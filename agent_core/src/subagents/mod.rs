pub mod builtins;
pub mod manager;
pub mod runner;
pub mod types;

pub use builtins::builtin_subagents;
pub use manager::SubagentManager;
pub use runner::{resolve_subagent_tools, run_subagent, RunSubagentOptions, NEVER_FOR_SUBAGENT};
pub use types::{
    SubagentMode, SubagentProfile, SubagentRunResult, SubagentScope, SubagentStepUpdate,
    SubagentToolArgs,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{ChatCompletionTool, ChatCompletionToolFunction};
    use serde_json::json;

    fn mock_tool(name: &str) -> ChatCompletionTool {
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: name.to_string(),
                description: format!("tool {}", name),
                parameters: json!({}),
            },
        }
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

    #[test]
    fn test_resolve_subagent_tools_readonly_safety() {
        let builtins = builtin_subagents();
        let researcher = builtins.iter().find(|p| p.id == "researcher").unwrap();

        let all_tools = vec![
            mock_tool("read_file"),
            mock_tool("write_file"),
            mock_tool("edit_file"),
            mock_tool("run_command"),
            mock_tool("search_files"),
            mock_tool("invoke_subagent"),
        ];

        let filtered = resolve_subagent_tools(researcher, &all_tools);
        let filtered_names: Vec<&str> = filtered.iter().map(|t| t.function.name.as_str()).collect();

        // 验证只读智能体绝对拿不到任何写工具和套娃工具 (AGENTS.md §2)
        assert!(filtered_names.contains(&"read_file"));
        assert!(filtered_names.contains(&"search_files"));
        assert!(!filtered_names.contains(&"write_file"));
        assert!(!filtered_names.contains(&"edit_file"));
        assert!(!filtered_names.contains(&"run_command"));
        assert!(!filtered_names.contains(&"invoke_subagent"));
    }

    #[test]
    fn test_resolve_subagent_tools_never_for_subagent() {
        let builtins = builtin_subagents();
        let general = builtins.iter().find(|p| p.id == "general_purpose").unwrap();

        let all_tools = vec![
            mock_tool("read_file"),
            mock_tool("write_file"),
            mock_tool("invoke_subagent"),
            mock_tool("check_subagent"),
            mock_tool("send_subagent_message"),
            mock_tool("resume_subagent"),
            mock_tool("await_subagents"),
        ];

        let filtered = resolve_subagent_tools(general, &all_tools);
        let filtered_names: Vec<&str> = filtered.iter().map(|t| t.function.name.as_str()).collect();

        assert!(filtered_names.contains(&"read_file"));
        assert!(filtered_names.contains(&"write_file"));
        // 递归工具被通配符 '*' 也严格排除
        assert!(!filtered_names.contains(&"invoke_subagent"));
        assert!(!filtered_names.contains(&"check_subagent"));
        assert!(!filtered_names.contains(&"send_subagent_message"));
        assert!(!filtered_names.contains(&"resume_subagent"));
        assert!(!filtered_names.contains(&"await_subagents"));
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
