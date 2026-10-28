//! 改写后的**重新校验**（P3b · SP-61）：改写不是授权。
//!
//! 插件能改写工具参数之后，"改写 → 执行"这条链多了一个入口：如果核心直接相信插件给的参数，
//! 形状不对的调用就会绕过工具自己的契约。所以流程是固定的：
//!
//! ```text
//! 解析 tool_call
//!   → beforeToolCall（可改写 / 可拦下）
//!   → 重新校验：形状 / schema          ← 本模块
//!   → 执行（改写后的 args）
//! ```
//!
//! 三条刻意做窄的口径：
//!
//! 1. **描述符不属于可改写载荷**：插件改不了"这是哪个工具、它是不是写工具"
//!    （载荷里只有 `args`），所以"把写工具伪装成只读"在结构上就做不到。
//! 2. **schema 校验是"最低限度"的**：只查必需字段与顶层类型，不引 JSON Schema 实现。
//!    理由：多数内置工具的 schema 是**宽松**的（`properties` 常常是空的），
//!    严格校验会把本来能跑的调用拒掉。
//! 3. **不做路径裁决**：路径只解析、不拒绝；需要管控时由插件事件决定。

use serde_json::Value;

/// 改写被拒的理由（都是**结构化**的短标签，便于用例断言与面板过滤）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceRejection {
    /// 改写出来的不是对象（工具参数必须是对象）
    NotAnObject,
    /// schema 校验不过（缺必需字段 / 类型不符）
    Schema(String),
}

impl ReplaceRejection {
    /// 给人与模型看的一句话（**说清怎么改才对**，而不是只说"不合法"）。
    pub fn describe(&self) -> String {
        match self {
            Self::NotAnObject => "改写后的参数必须是 JSON 对象".to_string(),
            Self::Schema(msg) => format!("改写后的参数不符合工具契约：{msg}"),
        }
    }

    /// 稳定标签（诊断与用例断言用；与 `describe()` 分开，因为文案会改、标签不该改）。
    pub fn label(&self) -> String {
        match self {
            Self::NotAnObject => "not_an_object".to_string(),
            Self::Schema(_) => "schema_mismatch".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    /// 这里不假装自己是完整校验器）。
    #[test]
    fn test_valid_args_pass_and_unknown_fields_are_tolerated() {
        assert!(validate_args_against_schema(
            &serde_json::json!({ "path": "a.txt", "extra": 1 }),
            &schema()
        )
        .is_ok());
    }
}
