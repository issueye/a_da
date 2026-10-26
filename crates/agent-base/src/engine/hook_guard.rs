//! 改写后的**重新校验**（P3b · SP-61）：改写不是授权。
//!
//! # 为什么必须有这一层
//!
//! 插件能改写工具参数之后，"审批 → 执行"这条链就多了一个入口：如果核心直接相信插件给的参数，
//! 那么把 `rm -rf /workspace` 改写成别的、或者把越界路径混进参数里，就都能绕开闸门。
//! 所以安全序（设计 §5.6）是**固定的**：
//!
//! ```text
//! 解析 tool_call
//!   → beforeToolCall（可改写 / 可拦下）
//!   → 重新校验：形状 / schema / 路径沙箱      ← 本模块
//!   → 风险单调性 + needs_approval(改写后的 args)  ← 引擎
//!   → 用户决定 → 执行（改写后的 args）
//! ```
//!
//! # 三条刻意做窄的口径
//!
//! 1. **描述符不属于可改写载荷**：插件改不了"这是哪个工具、它是不是写工具"
//!    （载荷里只有 `args`），所以"把写工具伪装成只读"在结构上就做不到。
//! 2. **schema 校验是"最低限度"的**：只查必需字段与顶层类型，不引 JSON Schema 实现。
//!    理由：多数内置工具的 schema 是**宽松**的（`properties` 常常是空的），
//!    严格校验会把本来能跑的调用拒掉；而真正要挡的东西（越界路径、绕过审批）
//!    由下面两条与审批端口负责。**这里不假装自己是完整校验器**。
//! 3. **路径校验用工具声明的路径字段**（`Access::Mutates { paths }`），
//!    走的是与外层工具**同一个** `Scope`——档位语义因此自动一致
//!    （`full` 档下越界本来就允许，但代码路径与留痕一致；见 SP-33/SP-36）。

use serde_json::Value;

use crate::domain::{Access, DenialKind, PathSelector};
use crate::ports::Scope;

/// 改写被拒的理由（都是**结构化**的短标签，便于用例断言与面板过滤）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceRejection {
    /// 改写出来的不是对象（工具参数必须是对象）
    NotAnObject,
    /// schema 校验不过（缺必需字段 / 类型不符）
    Schema(String),
    /// 路径参数越界（沙箱）
    Path { field: String, kind: DenialKind },
}

impl ReplaceRejection {
    /// 给人与模型看的一句话（**说清怎么改才对**，而不是只说"不合法"）。
    pub fn describe(&self) -> String {
        match self {
            Self::NotAnObject => "改写后的参数必须是 JSON 对象".to_string(),
            Self::Schema(msg) => format!("改写后的参数不符合工具契约：{msg}"),
            Self::Path { field, kind } => format!(
                "改写后的 `{field}` 越出作用域（{:?}）：改写不能把路径挪到沙箱外",
                kind
            ),
        }
    }

    /// 稳定标签（诊断与用例断言用；与 `describe()` 分开，因为文案会改、标签不该改）。
    pub fn label(&self) -> String {
        match self {
            Self::NotAnObject => "not_an_object".to_string(),
            Self::Schema(_) => "schema_mismatch".to_string(),
            Self::Path { field, .. } => format!("path_out_of_scope:{field}"),
        }
    }
}

/// 最低限度的 schema 校验：必需字段在不在、顶层类型对不对。
pub fn validate_args_against_schema(args: &Value, schema: &Value) -> Result<(), ReplaceRejection> {
    let Some(obj) = args.as_object() else {
        return Err(ReplaceRejection::NotAnObject);
    };
    // 顶层类型声明为 object（内置工具一律如此）才做下面的检查；其它类型如实放行，
    // 不在这里发明规则。
    if let Some(t) = schema.get("type").and_then(|v| v.as_str()) {
        if t != "object" {
            return Ok(());
        }
    }
    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        for key in required.iter().filter_map(|v| v.as_str()) {
            match obj.get(key) {
                None | Some(Value::Null) => {
                    return Err(ReplaceRejection::Schema(format!("缺少必需字段 `{key}`")))
                }
                Some(_) => {}
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (key, spec) in props {
            let Some(declared) = spec.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(value) = obj.get(key) else { continue };
            // null 一律放过：它表示"没给这个字段"，由工具自己决定怎么解释
            if value.is_null() {
                continue;
            }
            let matches = match declared {
                "string" => value.is_string(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                _ => true,
            };
            if !matches {
                return Err(ReplaceRejection::Schema(format!(
                    "字段 `{key}` 期望 {declared}，实际是 {}",
                    type_name(value)
                )));
            }
        }
    }
    Ok(())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 工具声明的**路径字段**（`Access::Mutates { paths }`）。
///
/// `Executes { command_arg }` 没有路径字段：命令本身**不是路径**，它的安全性由
/// 审批端口的危险模式判定负责（改写要过风险单调性，见引擎里的那一段）——
/// 拿路径沙箱去判命令字符串只会得到"不是合法路径"这种没用的结论。
///
/// `External` 也没有：外部工具的参数不是工作区路径（它的副作用在别的进程/别的机器上），
/// 拿工作区沙箱去判它只会误伤——那只证明了"这个字符串不是工作区里的路径"。
pub fn path_fields_of(access: &Access) -> Vec<(&'static str, bool)> {
    match access {
        Access::ReadOnly | Access::Executes { .. } | Access::External => Vec::new(),
        Access::Mutates {
            paths: PathSelector::Single(field),
        } => vec![(*field, false)],
        Access::Mutates {
            paths: PathSelector::Batch(field),
        } => vec![(*field, true)],
    }
}

/// 把改写后的路径参数逐个走一遍**外层同一个** `Scope`。
///
/// 单目标：字段存在就得解析得出来；批量：数组里每个字符串都要解析得出来。
/// 解析不出来 = 越界（`Scope::resolve_path` 的失败方向固定为拒绝，不做成可配置）。
pub fn check_paths(
    scope: &dyn Scope,
    access: &Access,
    args: &Value,
) -> Result<(), ReplaceRejection> {
    let Some(obj) = args.as_object() else {
        return Err(ReplaceRejection::NotAnObject);
    };
    for (field, is_batch) in path_fields_of(access) {
        let Some(value) = obj.get(field) else { continue };
        if value.is_null() {
            continue;
        }
        let candidates: Vec<&str> = if is_batch {
            match value.as_array() {
                Some(items) => items.iter().filter_map(|v| v.as_str()).collect(),
                None => {
                    return Err(ReplaceRejection::Schema(format!(
                        "字段 `{field}` 期望路径数组"
                    )))
                }
            }
        } else {
            match value.as_str() {
                Some(s) => vec![s],
                None => {
                    return Err(ReplaceRejection::Schema(format!("字段 `{field}` 期望路径字符串")))
                }
            }
        };
        for raw in candidates {
            if let Err(kind) = scope.resolve_path(raw) {
                return Err(ReplaceRejection::Path {
                    field: field.to_string(),
                    kind,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockScope;

    fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
                "count": { "type": "integer" }
            },
            "required": ["path"]
        })
    }

    /// 必需字段缺失 → 拒（"改写成 schema 之外的东西"必须挡在审批之前）。
    #[test]
    fn test_missing_required_field_is_rejected() {
        let err = validate_args_against_schema(&serde_json::json!({ "content": "x" }), &schema())
            .expect_err("缺 path 必须被拒");
        assert_eq!(err.label(), "schema_mismatch");
        assert!(err.describe().contains("path"), "{err:?}");
    }

    /// 顶层类型不符 → 拒（说清期望什么、实际是什么）。
    #[test]
    fn test_type_mismatch_is_rejected_with_both_sides() {
        let err = validate_args_against_schema(
            &serde_json::json!({ "path": "a", "count": "不是数字" }),
            &schema(),
        )
        .expect_err("类型不符必须被拒");
        let d = err.describe();
        assert!(d.contains("count") && d.contains("integer"), "{d}");
    }

    #[test]
    fn test_non_object_args_are_rejected() {
        let err = validate_args_against_schema(&serde_json::json!([1, 2]), &schema())
            .expect_err("数组不是合法参数");
        assert_eq!(err.label(), "not_an_object");
    }

    /// 合法的改写放行；未声明的额外字段也放行（内置工具 schema 常常是宽松的，
    /// 这里不假装自己是完整校验器——真正要挡的越界路径由 `check_paths` 负责）。
    #[test]
    fn test_valid_args_pass_and_unknown_fields_are_tolerated() {
        assert!(validate_args_against_schema(
            &serde_json::json!({ "path": "a.txt", "extra": 1 }),
            &schema()
        )
        .is_ok());
    }

    /// 路径越界 → 拒（且指出是哪个字段）。
    #[test]
    fn test_path_out_of_scope_is_rejected() {
        let scope = MockScope::new("ws");
        let access = Access::Mutates {
            paths: PathSelector::Single("path"),
        };
        // MockScope 的实现口径：以 workspace 开头的路径算在范围内（见其定义）
        let ok = check_paths(&scope, &access, &serde_json::json!({ "path": "ws/a.txt" }));
        assert!(ok.is_ok(), "{ok:?}");

        let err = check_paths(&scope, &access, &serde_json::json!({ "path": "/etc/passwd" }))
            .expect_err("越界必须被拒");
        assert!(err.label().starts_with("path_out_of_scope"), "{err:?}");
    }

    /// 批量路径：数组里**任意一个**越界就要拒（不能只看第一个）。
    #[test]
    fn test_batch_paths_check_every_element() {
        let scope = MockScope::new("ws");
        let access = Access::Mutates {
            paths: PathSelector::Batch("files"),
        };
        let err = check_paths(
            &scope,
            &access,
            &serde_json::json!({ "files": ["ws/a.txt", "/etc/passwd"] }),
        )
        .expect_err("批量里有一个越界就必须拒");
        assert!(err.label().contains("files"), "{err:?}");
    }

    /// 只读工具没有路径字段可查（描述符决定，不靠猜参数名）。
    #[test]
    fn test_readonly_tool_has_no_path_fields() {
        assert!(path_fields_of(&Access::ReadOnly).is_empty());
    }
}
