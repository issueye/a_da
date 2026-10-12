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

/// 把命令行切成"词"：按空白与常见 shell 分隔符切。
///
/// 🔴 为什么要分词（而不是整串子串匹配）：命令里的**连字符必须留在词内**，
/// 这样 `Format-Table` 是一个词、不会等于 `format`；引号与 `|`/`;`/`&` 等分隔符
/// 则要从词里剔掉，避免 `"rm"` / `x|rm` 这类形式漏判。
fn tokenize(cmd: &str) -> Vec<String> {
    cmd.to_lowercase()
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    ';' | '|' | '&' | '(' | ')' | '"' | '\'' | ',' | '=' | '<' | '>' | '`'
                )
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 一个命令行词是否等于危险模式词。
///
/// 额外容忍路径与可执行后缀：`C:\tools\rm.exe` 仍应被 `rm` 命中。
fn token_matches(token: &str, pat: &str) -> bool {
    if token == pat {
        return true;
    }
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
    base == pat
        || base
            .strip_suffix(".exe")
            .or_else(|| base.strip_suffix(".cmd"))
            .or_else(|| base.strip_suffix(".bat"))
            .or_else(|| base.strip_suffix(".ps1"))
            .is_some_and(|stem| stem == pat)
}

/// 检查是否包含破坏性危险命令。
///
/// 🔴 判定是**按词**的：把模式也切成词，要求命令行里存在**连续的一段词**与之逐词相等。
///
/// 早先用的是 `cmd.to_lowercase().contains(pattern)`——**子串**匹配，于是清单里的
/// `"format"` 会被 PowerShell 的 `Format-Table` 命中，一条纯只读的列目录命令被当成
/// "高危"强制弹审批（实测把整轮卡死在审批闸门 30 分钟）；`"rm "` / `"del "` 同理，
/// 会命中任何恰好含这几个字符的文本（例如 `...-Recurse` 之后的 `--del-file`）。
pub fn is_destructive_command(cmd: &str, confirm_list: &[String]) -> bool {
    let tokens = tokenize(cmd);
    if tokens.is_empty() {
        return false;
    }
    confirm_list.iter().any(|pattern| {
        let pat_tokens = tokenize(pattern);
        if pat_tokens.is_empty() {
            return false;
        }
        tokens
            .windows(pat_tokens.len())
            .any(|window| {
                window
                    .iter()
                    .zip(&pat_tokens)
                    .all(|(token, pat)| token_matches(token, pat))
            })
    })
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

    /// 守门：危险判定必须**按词**，不能拿子串去比。
    ///
    /// 实测踩过：PowerShell 的表格输出命令 `Format-Table` 含子串 `format`，
    /// 于是一条 `Get-ChildItem ... | Format-Table` 的**纯只读**列目录命令被判定为
    /// 高危、强制弹审批，整轮卡死在审批闸门里（30 分钟超时后被自动拒绝）。
    #[test]
    fn test_destructive_scan_matches_tokens_not_substrings() {
        let list = ApprovalGuardConfig::default().confirm_commands;

        // 只读列表命令里的 Format-Table 不得被当成 `format`
        assert!(
            !is_destructive_command(
                "powershell -NoProfile -Command \"Get-ChildItem -Path crates -Recurse \
                 -Filter *.rs | Sort-Object L -Descending | Format-Table -AutoSize\"",
                &list
            ),
            "Format-Table 不是 format 命令"
        );
        // 其它"含危险词片段但不构成危险命令"的写法
        assert!(!is_destructive_command("cargo test --test reset_all", &list));
        assert!(!is_destructive_command("echo reformat", &list));

        // 保守侧（**刻意保留**）：危险词以完整词形出现就仍然问。
        // 宁可多问一次（用户点一下就好），也不放过藏在引号/包装器里的真危险命令。
        assert!(is_destructive_command("git log --grep 'rm -rf'", &list));
        assert!(is_destructive_command("powershell -Command \"rm -r x\"", &list));

        // 真危险命令仍必须命中（含两个词的 `git push` 与带路径/后缀的形式）
        assert!(is_destructive_command("rm -rf /tmp/x", &list));
        assert!(is_destructive_command("del build\\out.exe", &list));
        assert!(is_destructive_command("git push --force", &list));
        assert!(is_destructive_command("git reset --hard HEAD~1", &list));
        assert!(is_destructive_command("format D: /q", &list));
        assert!(is_destructive_command("taskkill /F /IM node.exe", &list));
        assert!(
            is_destructive_command("C:\\tools\\rmdir.exe /s /q out", &list),
            "带路径与 .exe 后缀的可执行名仍要认出来"
        );
        // 分隔符/引号不应挡住判定
        assert!(is_destructive_command("cd /tmp && rm -rf x", &list));
    }
}
