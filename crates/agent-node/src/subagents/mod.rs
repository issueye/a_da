pub mod builtins;
pub mod local_bus;
pub mod manager;
pub mod ports;
pub mod runner;
pub mod tool;
pub mod types;

pub use builtins::builtin_subagents;
pub use local_bus::LocalAgentBus;
pub use manager::SubagentManager;
pub use ports::{EphemeralSessionStore, ProfilePrompt, ReadonlyEnforcingGate};
pub use runner::{
    filter_subagent_tools, run_subagent, RunSubagentOptions, LEGACY_ONLY_TOOLS,
    NEVER_FOR_SUBAGENT,
};
pub use tool::InvokeSubagentTool;
pub use types::{
    SubagentMode, SubagentProfile, SubagentRunResult, SubagentScope, SubagentStepUpdate,
    SubagentToolArgs,
};

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{
        Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor,
    };
    use agent_base::ports::Tool;
    use agent_base::testing::MockTool;
    use std::sync::Arc;

    /// 造一个**描述符驱动**的测试工具（W4-T6：裁切只看描述符，不再看 ChatCompletionTool）。
    ///
    /// 只读性由注册表决定——名字在注册表里就用它的真实 `access`，
    /// 这样测试测的是**真实判定**而不是我在这里手写的布尔值。
    fn desc_tool(name: &str) -> Arc<dyn Tool> {
        let descriptor = agent_toolkit::registry::find_tool_descriptor(name)
            .cloned()
            .unwrap_or(ToolDescriptor {
                name: name.to_string(),
                summary: format!("tool {name}"),
                schema: serde_json::json!({ "type": "object" }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            });
        Arc::new(MockTool::new(
            descriptor,
            agent_base::domain::ToolReceipt::success("ok", 1, 2),
        ))
    }

    #[test]
    fn test_builtin_subagents_completeness() {
        let builtins = builtin_subagents();
        assert_eq!(builtins.len(), 4);

        let ids: Vec<&str> = builtins.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"general_purpose"));
        assert!(ids.contains(&"researcher"));
        assert!(ids.contains(&"code_reviewer"));
        assert!(ids.contains(&"tester"));

        let researcher = builtins.iter().find(|p| p.id == "researcher").unwrap();
        assert_eq!(researcher.mode, SubagentMode::Readonly);
        assert!(researcher.allowed_tools.contains(&"read_file".to_string()));
    }

    /// W4-T6：裁切改在**描述符**上做（`filter_subagent_tools`）。
    /// 只读档位 + 递归黑名单都必须生效。
    #[test]
    fn test_filter_subagent_tools_readonly_safety() {
        let builtins = builtin_subagents();
        let researcher = builtins.iter().find(|p| p.id == "researcher").unwrap();

        let all_tools = vec![
            desc_tool("read_file"),
            desc_tool("write_file"),
            desc_tool("edit_file"),
            desc_tool("run_command"),
            desc_tool("search_files"),
            desc_tool("invoke_subagent"),
        ];

        let filtered = filter_subagent_tools(researcher, &all_tools);
        let filtered_names: Vec<String> = filtered
            .iter()
            .map(|t| t.descriptor().name.clone())
            .collect();

        // 验证只读智能体绝对拿不到任何写工具和套娃工具 (AGENTS.md §2)
        assert!(filtered_names.contains(&"read_file".to_string()));
        assert!(filtered_names.contains(&"search_files".to_string()));
        assert!(!filtered_names.contains(&"write_file".to_string()));
        assert!(!filtered_names.contains(&"edit_file".to_string()));
        assert!(!filtered_names.contains(&"run_command".to_string()));
        assert!(!filtered_names.contains(&"invoke_subagent".to_string()));
    }

    #[test]
    fn test_filter_subagent_tools_never_for_subagent() {
        let builtins = builtin_subagents();
        let general = builtins.iter().find(|p| p.id == "general_purpose").unwrap();

        // W2-T6：只造 **真实存在** 的工具。原先这里 mock 了 4 个本仓没有实现的名字
        // （`check_subagent` 等），于是"断言过滤掉了它们"是**空转断言**——
        // 那些工具根本不可能出现在工具表里。
        let all_tools = vec![
            desc_tool("read_file"),
            desc_tool("write_file"),
            desc_tool("invoke_subagent"),
        ];

        let filtered = filter_subagent_tools(general, &all_tools);
        let filtered_names: Vec<String> = filtered
            .iter()
            .map(|t| t.descriptor().name.clone())
            .collect();

        assert!(filtered_names.contains(&"read_file".to_string()));
        assert!(filtered_names.contains(&"write_file".to_string()));
        // 递归工具被通配符 '*' 也严格排除
        assert!(!filtered_names.contains(&"invoke_subagent".to_string()));
    }

    /// W2-T6 守门：白名单/黑名单里的名字必须**真实存在**——要么在 `ToolDescriptor`
    /// 注册表里，要么在显式的 legacy 名单里。
    ///
    /// 幽灵名字（`find_symbol`/`Skill`/`get_outline`/`read_files`/`design_decision`/
    /// `run_test_focused`/`edit_files`/`read_url_content`）让子智能体**以为自己有某个能力**，
    /// 实际永远拿不到——而且过滤逻辑"成功过滤掉了不存在的东西"，没有任何红灯。
    /// 这条断言就是防止它们回来。
    #[test]
    fn test_subagent_tool_lists_have_no_ghost_names() {
        use crate::subagents::runner::LEGACY_ONLY_TOOLS;
        use agent_toolkit::standard_tool_descriptors;
        use std::collections::HashSet;

        let registry: HashSet<String> = standard_tool_descriptors()
            .iter()
            .map(|d| d.name.clone())
            .collect();
        let legacy: HashSet<&str> = LEGACY_ONLY_TOOLS.iter().copied().collect();
        let known = |name: &str| registry.contains(name) || legacy.contains(name);

        for p in builtin_subagents() {
            for name in &p.allowed_tools {
                if name == "*" {
                    continue;
                }
                assert!(
                    known(name),
                    "profile `{}` 的白名单里有不存在的工具 `{name}`（幽灵名字）",
                    p.id
                );
            }
            for name in p.disallowed_tools.iter().flatten() {
                assert!(
                    known(name),
                    "profile `{}` 的黑名单里有不存在的工具 `{name}`（幽灵名字）",
                    p.id
                );
            }
        }

        for name in crate::subagents::runner::NEVER_FOR_SUBAGENT {
            assert!(known(name), "NEVER_FOR_SUBAGENT 里有不存在的工具 `{name}`");
        }
        for name in LEGACY_ONLY_TOOLS {
            assert!(
                !registry.contains(*name),
                "`{name}` 已进注册表，应从 LEGACY_ONLY_TOOLS 移除（豁免必须随实现收敛）"
            );
        }
    }

    #[test]
    fn test_subagent_manager_memory_flow() {
        let mgr = SubagentManager::new();
        let list = mgr.list_profiles(None);
        assert!(list.len() >= 4);

        // 测试启用/禁用内置智能体
        let res = mgr.set_enabled("tester", false);
        assert!(res.is_ok());

        let list_after = mgr.list_profiles(None);
        let tester = list_after.iter().find(|p| p.id == "tester").unwrap();
        assert!(!tester.enabled);

        // 还原
        let _ = mgr.set_enabled("tester", true);
    }
}
