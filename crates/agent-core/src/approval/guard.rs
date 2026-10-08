use serde_json::Value;

use super::types::ApprovalGuardConfig;
use crate::protocol::ApprovalMode;
use crate::tools::is_write_tool;

/// 从工具参数中提取命令行文本（优先依据 ToolDescriptor 声明的字段）
pub fn extract_command(args: &Value) -> Option<String> {
    if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
        return Some(cmd.to_string());
    }
    if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
        return Some(cmd.to_string());
    }
    None
}

/// 判断工具是否属于命令执行类工具（真源：ToolDescriptor::access）
pub fn is_command_tool(tool_name: &str, config: &ApprovalGuardConfig) -> bool {
    if config.command_tools.iter().any(|t| t == tool_name) {
        return true;
    }
    if let Some(desc) = crate::tools::find_tool_descriptor(tool_name) {
        matches!(desc.access, agent_base::domain::Access::Executes { .. })
    } else {
        false
    }
}

/// 检查是否包含破坏性危险命令
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

/// 审批策略决策判定核心（严格对齐 TS 端 approval-guard 规范）
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
        return is_write_tool(tool_name);
    }

    // 规则 3：免问白名单。
    if config.auto_approve.iter().any(|t| t == tool_name) {
        return false;
    }

    // 规则 4：通用档位判断。
    match mode {
        ApprovalMode::Ask => true,
        ApprovalMode::Auto => false,
        ApprovalMode::Readonly => is_write_tool(tool_name),
    }
}
