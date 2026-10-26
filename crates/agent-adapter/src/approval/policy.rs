//! 审批**策略**（纯函数，无 IO、无单例）。
//!
//! 设计口径（AGENTS.md §14：**策略**归插件 `approval-guard`、**执行**归核心 `askUser`；
//! `docs/plans/wiring.md` §5 W1-T6）：
//! 档位解释、危险命令二次确认、免问白名单都是**纯判定**，因此放在适配器；
//! "问用户"的执行侧（注册 waiter、等 `approval.decide`、超时/取消）需要前端通道，留在宿主。
//!
//! 本文件是 W1-T6 从 `agent-core/src/approval/{guard,types}.rs` **搬运**而来，
//! 判定逻辑逐条保持不变（agent-core 侧保留 `pub use` shim，调用点零改动）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use agent_base::domain::Access;
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

/// 这次调用算不算“写”（只读档、只读子智能体、plan 模式共用这一个判据）。
///
/// 🔴 **真源是描述符的 `access`，不是工具名**（INV-3）。历史实现按名字回查注册表，
/// 而注册表里只有内置工具：插件工具、外部工具（MCP server / 远端）全都被当成“写”。
/// 那是一个双向的错：读类外部工具在只读档里被逐次审批（不可用），
/// 而将来“用户授权某个外部工具只读”也没有落地的地方。
///
/// `None` = 拿不到声明（未知工具）→ **按写处理**，失败安全（AGENTS.md §2）。
pub fn is_write_by_access(access: Option<&Access>) -> bool {
    match access {
        // 唯一能被当只读的情形：基座**自己**声明了 ReadOnly。
        Some(Access::ReadOnly) => false,
        // Mutates / Executes / External：都会造成副作用，或者压根无法描述其作用域。
        Some(_) => true,
        None => true,
    }
}

/// 兼容入口：调用方拿不到描述符时，按注册表回查内置工具的声明。
///
/// 非内置工具（插件 / 外部）在这里一律得到 `None` → 按写处理。
/// **权威路径（审批闸门）不走这里**：它把 `ToolDescriptor::access` 直接递给
/// [`should_ask_approval_for`]，不需要回查。
pub fn should_ask_approval(
    mode: ApprovalMode,
    tool_name: &str,
    args: &Value,
    config: &ApprovalGuardConfig,
) -> bool {
    let access = agent_toolkit::find_tool_descriptor(tool_name).map(|d| &d.access);
    should_ask_approval_for(mode, tool_name, access, args, config)
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
///
/// **档位优先级最高**：`Auto`（自动批准）是 YOLO 档——无条件放行，连危险命令的二次确认
/// 也一并免掉。这与界面文案一致（"读写文件和执行命令都不再询问"），也是用户显式选择
/// 这一档的语义。需要危险命令二次确认的场景请用 `Ask` / `Readonly`。
pub fn should_ask_approval_with_write(
    mode: ApprovalMode,
    tool_name: &str,
    is_write: bool,
    args: &Value,
    config: &ApprovalGuardConfig,
) -> bool {
    // 规则 0：`Auto` 与 `Full`（完全访问）= 不问，**先短路**。
    //
    // 必须放在规则 1 之前：规则 1（危险命令二次确认）原本是唯一能拦下自动批准的例外，
    // 而用户选了自动批准/完全访问就是不要任何弹窗——它不能再绕过档位。
    //
    // `Full` 与 `Auto` 在"问不问"这一层**完全一致**；它的增量在**权限层**
    // （工作区边界与脚本网络，见 `PermissionProfile` 与 `scope`）。
    if matches!(mode, ApprovalMode::Auto | ApprovalMode::Full) {
        return false;
    }

    // 规则 1：高危需二次确认。
    // 命令类工具（由 ToolDescriptor 或自定义配置驱动）命中危险模式时强制问用户。
    // 走到这里档位只可能是 `Ask` / `Readonly`（`Auto` 已在规则 0 短路）。
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
        return is_write;
    }

    // 规则 3：免问白名单。
    if config.auto_approve.iter().any(|t| t == tool_name) {
        return false;
    }

    // 规则 4：通用档位判断。
    match mode {
        ApprovalMode::Ask => true,
        // 规则 0 已短路，这里只为 match 穷尽；语义上"自动批准/完全访问不询问"
        ApprovalMode::Auto | ApprovalMode::Full => false,
        ApprovalMode::Readonly => is_write,
    }
}

/// 这次调用要不要问人（读写性取自**描述符的访问方式**，INV-3）。
///
/// 权威入口：调用方手里有 `ToolDescriptor::access` 时用它。
/// `access` 为 `None` = 拿不到声明 → 按写处理（失败安全）。
pub fn should_ask_approval_for(
    mode: ApprovalMode,
    tool_name: &str,
    access: Option<&Access>,
    args: &Value,
    config: &ApprovalGuardConfig,
) -> bool {
    should_ask_approval_with_write(mode, tool_name, is_write_by_access(access), args, config)
}

/// 权限档位（P1b 的第二层）：回答"能碰什么"，而不是"问不问"。
///
/// 与审批档位**共用一个真源**（[`ApprovalMode`]），但消费者不同：
/// 审批档位归 [`should_ask_approval`]（问不问），权限档位归 `scope`（能不能越出工作区）
/// 与脚本的 `Needs`（有没有网络）。
///
/// 为什么仍要显式分成两个枚举：将来会出现"问，但允许越界"这类组合
/// （设计文档 §6.2）；届时给审批层加档、给权限层加档各改一处，不必再动彼此。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionProfile {
    /// 只在工作区内活动（默认）
    Workspace,
    /// 放开工作区边界（`~/.a-da` 红线除外）
    Full,
}

/// 由审批档位推出权限档位（**唯一映射处**）。
pub fn permission_profile_of(mode: ApprovalMode) -> PermissionProfile {
    match mode {
        ApprovalMode::Full => PermissionProfile::Full,
        // 其余三档都只在工作区内活动（`Auto` 只是不再询问，不是放开边界）
        _ => PermissionProfile::Workspace,
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
    /// P1b：`Full`（完全访问）在"问不问"这一层与 `auto` **完全一致**；
    /// 它的增量在**权限层**（工作区边界与脚本网络），所以这里同时钉住两层。
    #[test]
    fn test_full_access_never_asks_and_maps_to_the_full_permission_profile() {
        // 层一：问不问 —— 连危险命令也不问（与 auto 同）
        assert!(!should_ask_approval(
            ApprovalMode::Full,
            "run_command",
            &json!({ "command": "rm -rf /tmp/x" }),
            &config()
        ));
        assert!(!should_ask_approval(
            ApprovalMode::Full,
            "write_file",
            &json!({ "path": "a.rs" }),
            &config()
        ));

        // 层二：权限档位 —— 只有 Full 放开边界，其余三档都在工作区内
        assert_eq!(permission_profile_of(ApprovalMode::Full), PermissionProfile::Full);
        for m in [ApprovalMode::Auto, ApprovalMode::Ask, ApprovalMode::Readonly] {
            assert_eq!(
                permission_profile_of(m),
                PermissionProfile::Workspace,
                "{m:?} 不是完全访问，不该放开工作区边界"
            );
        }
    }

    #[test]
    fn test_auto_mode_is_yolo_and_never_asks() {
        // 规则 0：自动批准 = YOLO，连危险命令也不再二次确认
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &json!({ "command": "rm -rf /tmp/x" }),
            &config()
        ));
        assert!(!should_ask_approval(
            ApprovalMode::Auto,
            "run_command",
            &json!({ "cmd": "git push --force" }),
            &config()
        ));
    }

    #[test]
    fn test_destructive_command_still_asks_outside_auto() {
        // 离开 Auto 档之后，"危险命令二次确认"依然生效（规则 0 只放行 Auto）
        assert!(should_ask_approval(
            ApprovalMode::Readonly,
            "run_command",
            &json!({ "command": "rm -rf /tmp/x" }),
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

    /// 只读档位 + **外部工具**（MCP server / 远端）：必须问。
    ///
    /// 这是 MCP 集成的关键一格：外部工具不在工作区坐标系里，`Access::External`
    /// 永远不是只读，于是只读档不会把它当“查询类”放过去。
    #[test]
    fn test_readonly_mode_asks_for_external_tools() {
        assert!(should_ask_approval_for(
            ApprovalMode::Readonly,
            "mcp__fs__read",
            Some(&Access::External),
            &json!({ "path": "a.rs" }),
            &config()
        ));
        // 每次询问档更不用说
        assert!(should_ask_approval_for(
            ApprovalMode::Ask,
            "mcp__fs__read",
            Some(&Access::External),
            &json!({}),
            &config()
        ));
        // 自动批准档是 YOLO：不问（既有语义，不为外部工具开例外）
        assert!(!should_ask_approval_for(
            ApprovalMode::Auto,
            "mcp__fs__read",
            Some(&Access::External),
            &json!({}),
            &config()
        ));
    }

    /// 读写性**来自描述符**，不来自名字。
    ///
    /// 旧实现按名字回查注册表：未注册的（插件 / 外部）一律判“写”。
    /// 这让“用户将来授权某个外部工具只读”无处落地——所以判据必须是 `access`。
    #[test]
    fn test_access_overrides_name_lookup() {
        // 未注册的名字 + ReadOnly 声明 → 只读档不问
        assert!(!should_ask_approval_for(
            ApprovalMode::Readonly,
            "not_in_registry_readonly",
            Some(&Access::ReadOnly),
            &json!({}),
            &config()
        ));
        // 同一个名字 + 没有声明 → 失败安全按写处理
        assert!(should_ask_approval_for(
            ApprovalMode::Readonly,
            "not_in_registry_readonly",
            None,
            &json!({}),
            &config()
        ));
    }

    /// 三种访问方式在“算不算写”上的统一口径（只读档、只读子智能体共用）。
    #[test]
    fn test_is_write_by_access_matrix() {
        assert!(!is_write_by_access(Some(&Access::ReadOnly)));
        assert!(is_write_by_access(Some(&Access::Mutates {
            paths: agent_base::domain::PathSelector::Single("path"),
        })));
        assert!(is_write_by_access(Some(&Access::Executes {
            command_arg: "command",
        })));
        assert!(is_write_by_access(Some(&Access::External)));
        assert!(is_write_by_access(None), "拿不到声明时失败安全：按写处理");
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
