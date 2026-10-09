//! 工具包真源：`agent.spec.json` 的 `toolkits` 字段就是查这张表。
//!
//! 为什么需要它：在 W2-T1 之前，`spec.toolkits` 被解析出来却**没有任何消费者**——
//! `ada-coding` 甚至声明了一个不存在的 `patch` 工具包（能力实际藏在 `fs` 的
//! edit/write 里），而 `decision` 有真实模块却没人声明。两侧都不一致，
//! 且没有任何机械检查能发现（`verify-spec` 当时只能靠"目录是否存在"猜）。
//!
//! 现在**名字 → 工具实例**只有这一处映射：
//! - 产品声明（`spec.toolkits`）→ [`tools_for_toolkits`]；
//! - 声明校验（`cargo xtask verify-spec`）→ [`TOOLKIT_NAMES`]；
//! - 接线审计（`cargo xtask verify-wiring`）→ 用本表判定"工具是否有执行路径"。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use agent_base::ports::Tool;

/// 已知工具包名（**唯一真源**）。
pub const TOOLKIT_NAMES: &[&str] = &["core", "fs", "command", "decision", "git", "project"];

/// 该名字是否是已知工具包。
pub fn is_known_toolkit(name: &str) -> bool {
    TOOLKIT_NAMES.contains(&name)
}

/// 取一个工具包的全部工具实例。未知名返回 `None`。
pub fn tools_for_toolkit(name: &str, workspace: &Path) -> Option<Vec<Arc<dyn Tool>>> {
    match name {
        "core" => Some(crate::core::core_tools()),
        "fs" => Some(crate::fs::fs_tools(workspace)),
        "command" => Some(crate::command::command_tools(workspace)),
        "decision" => Some(crate::decision::decision_tools(workspace)),
        // W2-T2：原先只在 `execute_builtin_plugin_tool` 字符串分派里的插件工具
        "git" => Some(crate::plugin_tools::git_tools(workspace)),
        "project" => Some(crate::plugin_tools::project_tools(workspace)),
        _ => None,
    }
}

/// 按产品声明装配多个工具包。
///
/// 两条**失败安全**约束（不许静默忽略）：
/// 1. 未知名 → `Err`（声明了一个不存在的工具包，就该在装配期炸掉，而不是少装几个工具）；
/// 2. 重名 → `Err`（两个工具包提供同名工具会让 catalog 出现两份同名描述符，
///    而"注册表即真源"要求名字唯一）。
pub fn tools_for_toolkits(
    names: &[String],
    workspace: &Path,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    for name in names {
        let tools = tools_for_toolkit(name, workspace).ok_or_else(|| {
            format!(
                "未知工具包 `{name}`（已知：{}）",
                TOOLKIT_NAMES.join(", ")
            )
        })?;
        out.extend(tools);
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for t in &out {
        let n = t.descriptor().name.clone();
        if !seen.insert(n.clone()) {
            return Err(format!("工具 `{n}` 被多个工具包重复提供"));
        }
    }
    Ok(out)
}

/// 所有工具包提供的工具名（用于接线审计与"声明 ↔ 实现"对账）。
pub fn all_toolkit_tool_names(workspace: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in TOOLKIT_NAMES {
        if let Some(tools) = tools_for_toolkit(name, workspace) {
            for t in tools {
                out.insert(t.descriptor().name.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> &'static Path {
        Path::new("E:/toolkit_test_ws")
    }

    #[test]
    fn test_every_known_toolkit_yields_tools() {
        for name in TOOLKIT_NAMES {
            let tools = tools_for_toolkit(name, ws())
                .unwrap_or_else(|| panic!("工具包 `{name}` 必须存在"));
            assert!(!tools.is_empty(), "工具包 `{name}` 不能是空的");
        }
    }

    #[test]
    fn test_unknown_toolkit_is_an_error_not_silently_skipped() {
        // `patch` 就是曾经被 `ada-coding` 误声明的那一个
        assert!(tools_for_toolkit("patch", ws()).is_none());
        let err = match tools_for_toolkits(&["patch".to_string()], ws()) {
            Ok(_) => panic!("未知名必须报错，而不是少装几个工具"),
            Err(e) => e,
        };
        assert!(err.contains("patch"), "错误信息要能定位：{err}");
    }

    #[test]
    fn test_declared_toolkits_assemble_without_duplicates() {
        let names: Vec<String> = TOOLKIT_NAMES.iter().map(|s| s.to_string()).collect();
        let tools = tools_for_toolkits(&names, ws()).expect("全部工具包必须能装配");
        assert_eq!(
            tools.len(),
            all_toolkit_tool_names(ws()).len(),
            "装配出的工具数必须等于去重后的名字数（说明没有重名）"
        );
    }

    #[test]
    fn test_all_toolkit_tool_names_covers_the_expected_core_tools() {
        let names = all_toolkit_tool_names(ws());
        for expect in ["read_file", "write_file", "run_command", "ask_user", "todo", "finish", "decide", "check_gate"] {
            assert!(names.contains(expect), "`{expect}` 应来自某个工具包：{names:?}");
        }
    }

    #[test]
    fn test_duplicate_tool_names_across_toolkits_are_rejected() {
        // 同一工具包声明两次 → 重名，必须报错
        let names = vec!["core".to_string(), "core".to_string()];
        let err = match tools_for_toolkits(&names, ws()) {
            Ok(_) => panic!("重名必须报错"),
            Err(e) => e,
        };
        assert!(err.contains("重复提供"), "错误信息要说明原因：{err}");
    }
}
