pub mod guard;
pub mod manager;
pub mod question_manager;
pub mod types;

pub use guard::{extract_command, is_destructive_command, should_ask_approval};
pub use manager::ApprovalManager;
pub use question_manager::{global_question_manager, QuestionAnswer, QuestionManager};
pub use types::ApprovalGuardConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ApprovalMode;
    use serde_json::json;

    #[test]
    fn test_destructive_command_interception_even_in_auto_mode() {
        let config = ApprovalGuardConfig::default();

        // 即使在 Auto 模式下，包含 rm 或 git reset 的危险命令也必须拦截二次确认
        let destructive_args = json!({ "command": "rm -rf /some/dir" });
        assert!(should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &destructive_args,
            &config
        ));

        let git_reset_args = json!({ "command": "git reset --hard HEAD~1" });
        assert!(should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &git_reset_args,
            &config
        ));

        // 安全的只读命令直接放行
        let safe_args = json!({ "command": "cargo check" });
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &safe_args,
            &config
        ));
    }

    #[test]
    fn test_readonly_mode_safety_constraint() {
        let config = ApprovalGuardConfig::default();

        // 只读模式下，写工具一律拦截
        let write_args = json!({ "path": "test.txt", "content": "hello" });
        assert!(should_ask_approval(
            ApprovalMode::Readonly,
            "write_file",
            &write_args,
            &config
        ));
        assert!(should_ask_approval(
            ApprovalMode::Readonly,
            "edit_file",
            &json!({ "path": "a.ts" }),
            &config
        ));

        // 只读模式下，读工具放行
        let read_args = json!({ "path": "test.txt" });
        assert!(!should_ask_approval(
            ApprovalMode::Readonly,
            "read_file",
            &read_args,
            &config
        ));
    }

    #[test]
    fn test_ask_mode_with_auto_approve_whitelist() {
        let mut config = ApprovalGuardConfig::default();
        config.auto_approve.push("read_file".to_string());

        // Ask 模式下，白名单里的 read_file 免问
        assert!(!should_ask_approval(
            ApprovalMode::Ask,
            "read_file",
            &json!({}),
            &config
        ));

        // 其余未在白名单的工具需询问
        assert!(should_ask_approval(
            ApprovalMode::Ask,
            "list_files",
            &json!({}),
            &config
        ));
    }

    #[tokio::test]
    async fn test_approval_manager_wait_and_resolve() {
        let mgr = ApprovalManager::new();
        let item_id = "tool_item_999";

        let rx = mgr.register_waiter(item_id);
        assert!(mgr.has_pending(item_id));

        // 模拟前端用户点击“批准”
        let resolved = mgr.resolve_approval(item_id, true);
        assert!(resolved);
        assert!(!mgr.has_pending(item_id));

        let user_decision = rx.await.expect("接收决策失败");
        assert!(user_decision);
    }
}
