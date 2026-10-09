//! 审批**策略**（纯函数，无 IO、无单例）。
//!
//! 设计口径（AGENTS.md §14：**策略**归插件 `approval-guard`、**执行**归核心 `askUser`；
//! `docs/agent-base-wiring-plan.md` §5 W1-T6）：
//! 档位解释、危险命令二次确认、免问白名单都是**纯判定**，因此放在适配器；
//! "问用户"的执行侧（注册 waiter、等 `approval.decide`、超时/取消）需要前端通道，留在宿主。
//!
//! 本文件是 W1-T6 从 `agent-core/src/approval/{guard,types}.rs` **搬运**而来，
//! 判定逻辑逐条保持不变（agent-core 侧保留 `pub use` shim，调用点零改动）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use agent_proto::ApprovalMode;

/// 审批策略的可配置项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalGuardConfig {
    pub auto_approve: Vec<String>,
    pub confirm_commands: Vec<String>,
    pub command_tools: Vec<String>,
}

impl Default for ApprovalGuardConfig {
    fn default() -> Self {
        Self {
            auto_approve: Vec::new(),
            // command_tools 默认交由 ToolDescriptor::access 动态判定（INV-3），不再维护硬编码名单
            command_tools: Vec::new(),
            confirm_commands: vec![
                "rm ".to_string(),
                "rmdir".to_string(),
                "del ".to_string(),
                "format".to_string(),
                "git push".to_string(),
                "git reset".to_string(),
                "git clean".to_string(),
                "npm publish".to_string(),
                "bun publish".to_string(),
                "shutdown".to_string(),
                "taskkill".to_string(),
            ],
        }
    }
}

/// 从工具参数中提取命令行文本。
pub fn extract_command(args: &Value) -> Option<String> {
    if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
        return Some(cmd.to_string());
    }
    if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
        return Some(cmd.to_string());
    }
    None
}

/// 判断工具是否属于命令执行类工具（真源：`ToolDescriptor::access`）。
pub fn is_command_tool(tool_name: &str, config: &ApprovalGuardConfig) -> bool {
    if config.command_tools.iter().any(|t| t == tool_name) {
        return true;
    }
    if let Some(desc) = agent_toolkit::find_tool_descriptor(tool_name) {
        matches!(desc.access, agent_base::domain::Access::Executes { .. })
    } else {
        false
    }
}

/// 检查是否包含破坏性危险命令。
pub fn is_destructive_command(cmd: &str, confirm_list: &[String]) -> bool {
    let lower = cmd.to_lowercase();
    for pattern in confirm_list {
        let pat_lower = pattern.to_lowercase();
        if lower.contains(&pat_lower) {
            return true;
        }
    }
    false
}

/// 审批策略决策判定核心（严格对齐 TS 端 approval-guard 规范）。
pub fn should_ask_approval(
    mode: ApprovalMode,
    tool_name: &str,
    args: &Value,
    config: &ApprovalGuardConfig,
) -> bool {
    // 规则 1：高危需二次确认。
    // 命令类工具（由 ToolDescriptor 或自定义配置驱动）命中危险模式时，即使用户开了自动批准也必须强制问用户。
    if is_command_tool(tool_name, config) {
        if let Some(cmd) = extract_command(args) {
            if is_destructive_command(&cmd, &config.confirm_commands) {
                return true;
            }
        }
    }

    // 规则 2：只读档位的硬约束。
    // ApprovalMode::Readonly 时写操作一律问用户，绝对不能自动放行。
    if mode == ApprovalMode::Readonly {
        return agent_toolkit::is_write_tool(tool_name);
    }

    // 规则 3：免问白名单。
    if config.auto_approve.iter().any(|t| t == tool_name) {
        return false;
    }

    // 规则 4：通用档位判断。
    match mode {
        ApprovalMode::Ask => true,
        ApprovalMode::Auto => false,
        ApprovalMode::Readonly => agent_toolkit::is_write_tool(tool_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> ApprovalGuardConfig {
        ApprovalGuardConfig::default()
    }

    #[test]
    fn test_auto_mode_does_not_ask_for_normal_tools() {
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "write_file",
            &json!({ "path": "a.rs" }),
            &config()
        ));
    }

    #[test]
    fn test_auto_mode_still_asks_for_destructive_command() {
        // 规则 1 优先于档位：自动批准也要为 `rm ` 二次确认
        assert!(should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &json!({ "command": "rm -rf /tmp/x" }),
            &config()
        ));
        assert!(should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &json!({ "cmd": "git push --force" }),
            &config()
        ));
    }

    #[test]
    fn test_auto_mode_allows_benign_command() {
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &json!({ "command": "cargo test" }),
            &config()
        ));
    }

    #[test]
    fn test_ask_mode_always_asks() {
        assert!(should_ask_approval(
            ApprovalMode::Ask,
            "read_file",
            &json!({ "path": "a.rs" }),
            &config()
        ));
    }

    #[test]
    fn test_readonly_mode_asks_only_for_write_tools() {
        assert!(should_ask_approval(
            ApprovalMode::Readonly,
            "write_file",
            &json!({ "path": "a.rs" }),
            &config()
        ));
        assert!(!should_ask_approval(
            ApprovalMode::Readonly,
            "read_file",
            &json!({ "path": "a.rs" }),
            &config()
        ));
    }

    #[test]
    fn test_auto_approve_whitelist_skips_asking() {
        let mut cfg = config();
        cfg.auto_approve.push("write_file".to_string());
        assert!(!should_ask_approval(
            ApprovalMode::Ask,
            "write_file",
            &json!({ "path": "a.rs" }),
            &cfg
        ));
    }

    #[test]
    fn test_unknown_tool_is_not_a_command_tool() {
        // 非命令工具：危险模式不参与判定（且未知工具不会被误判为命令工具）
        assert!(!is_command_tool("nonexistent_tool", &config()));
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "nonexistent_tool",
            &json!({ "command": "rm -rf /" }),
            &config()
        ));
    }

    #[test]
    fn test_extract_command_supports_both_field_names() {
        assert_eq!(
            extract_command(&json!({ "command": "ls" })).as_deref(),
            Some("ls")
        );
        assert_eq!(extract_command(&json!({ "cmd": "ls" })).as_deref(), Some("ls"));
        assert_eq!(extract_command(&json!({ "path": "a.rs" })), None);
    }
}
