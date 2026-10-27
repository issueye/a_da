//! `ApprovalGate` 的**生产实现**（执行侧）。
//!
//! 分工（AGENTS.md §14、`docs/plans/wiring.md` §5 W1-T6）：
//! - **策略**在适配器（[`agent_adapter::approval::policy`]）：档位解释、危险命令二次确认、
//!   免问白名单，纯函数、无 IO；
//! - **执行**在这里：读权威档位 → 注册等待通道 → 等前端 `approval.decide` → 超时/取消兜底。
//!
//! 为什么执行侧必须留在宿主：它要接前端的审批卡片通道（`ApprovalManager` 的 waiter 注册表），
//! 而 waiter 的解析点在 `dispatch.rs` 的 `approval.decide` 分支。
//!
//! 失败方向固定 [`FailDirection::Closed`]（INV-4）：超时、通道关闭、会话中止都**按拒绝处理**，
//! 绝不放行。

use std::sync::Arc;
use std::time::Duration;

use agent_base::domain::{Access, ApprovalPolicy, FailDirection};
use agent_base::ports::{
    AnsweredBy, ApprovalGate, ApprovalOutcome, ApprovalRequest, BoxFuture, CancelToken,
};
use agent_proto::ApprovalMode;

use crate::approval::manager::ApprovalManager;
use crate::node_config::NodeConfigSource;

/// 默认审批等待上限：超时按**拒绝**处理（fail-closed）。
/// 审批等待上限。
///
/// 30 分钟：等待期间界面会**全局提示**"有会话在等你批准/答复"（输入框上方的提示条 +
/// 标签页"等待审批"徽标），所以人离开一会儿也来得及。超过上限按**失败安全**方向拒绝
/// （不是放行）——这条底线不变。
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(1800);

/// 取消轮询间隔。
///
/// `CancelToken` 端口只有 `is_cancelled()`（轮询式），没有"等它发生"的入口——
/// 给它加方法属于端口变更，需单独排任务（见计划 §13.2 第 3 条）。在审批等待这种
/// 秒级场景里，50ms 的轮询延迟完全可以接受，且不引入新的契约。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct HostApprovalGate {
    /// 权威档位来源（S2）。
    ///
    /// **不再是 `AgentStore`**：节点层不得读 UI 投影。端口背后的生产实现是
    /// [`crate::server::node_config::StoreBackedNodeConfig`]，由组合根装配注入。
    config_source: Arc<dyn NodeConfigSource>,
    /// 前端答复通道
    manager: Arc<ApprovalManager>,
    /// 策略配置（免问白名单 / 危险命令清单）
    config: agent_adapter::approval::ApprovalGuardConfig,
    timeout: Duration,
    /// 安全下线（`agent_toolkit::security::deferred()`）：一律放行、不等用户。
    ///
    /// 默认 `false`：单测与恢复安全后的行为不变；组合根按总开关显式打开。
    defer_security: bool,
}

impl HostApprovalGate {
    pub fn new(config_source: Arc<dyn NodeConfigSource>, manager: Arc<ApprovalManager>) -> Self {
        Self {
            config_source,
            manager,
            config: agent_adapter::approval::ApprovalGuardConfig::default(),
            timeout: DEFAULT_APPROVAL_TIMEOUT,
            defer_security: false,
        }
    }

    /// 安全下线：`needs_approval` 恒 false，`decide` 直接放行。
    ///
    /// 只由组合根按 `agent_toolkit::security::deferred()` 调用。
    pub fn with_security_deferred(mut self, defer: bool) -> Self {
        self.defer_security = defer;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_config(mut self, config: agent_adapter::approval::ApprovalGuardConfig) -> Self {
        self.config = config;
        self
    }

    /// 读权威档位。
    ///
    /// S2：经 [`NodeConfigSource`] 端口读，不再直接读 `AgentStore`。
    async fn mode(&self) -> ApprovalMode {
        self.config_source.approval_mode()
    }
}

/// 等待取消发生（`None` 表示永不取消）。
async fn wait_cancel(cancel: Option<&dyn CancelToken>) {
    let Some(token) = cancel else {
        std::future::pending::<()>().await;
        return;
    };
    if token.is_cancelled() {
        return;
    }
    loop {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
        if token.is_cancelled() {
            return;
        }
    }
}

impl ApprovalGate for HostApprovalGate {
    fn direction(&self) -> FailDirection {
        FailDirection::Closed
    }

    fn needs_approval<'a>(
        &'a self,
        tool: &'a str,
        access: &'a Access,
        policy: &'a ApprovalPolicy,
        args: &'a serde_json::Value,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // 安全下线：不询问、不建卡、不等用户。
            if self.defer_security {
                return false;
            }
            match policy {
                // 引擎已短路 Never；这里再答一次，保证"从端口问"总有确定答案
                ApprovalPolicy::Never => false,
                ApprovalPolicy::Always => true,
                ApprovalPolicy::DangerScan { patterns } => {
                    let args_str = args.to_string();
                    patterns.iter().any(|p| args_str.contains(p))
                }
                // 命名策略：当前只有 `approval-guard`，其语义就是 `should_ask_approval`
                // （档位 + 危险命令二次确认 + 免问白名单）。**不再把名字当成"永远要问"**。
                ApprovalPolicy::Named(_) => {
                    let mode = self.mode().await;
                    // 读写性用**递进来的 `access`**（描述符真源），不再按工具名回查注册表：
                    // 注册表里没有插件工具与外部工具（MCP server / 远端）的名字。
                    agent_adapter::approval::policy::should_ask_approval_for(
                        mode,
                        tool,
                        Some(access),
                        args,
                        &self.config,
                    )
                }
            }
        })
    }

    fn decide<'a>(
        &'a self,
        req: ApprovalRequest,
        cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, ApprovalOutcome> {
        Box::pin(async move {
            // 安全下线：直接放行（`AnsweredBy::Policy` 如实标明"不是用户答的"）。
            if self.defer_security {
                return ApprovalOutcome::allowed(AnsweredBy::Policy);
            }
            // 档位可能在上游询问之后被用户改过，这里按**当前**档位复核一次：
            // 若策略说不用问了，就直接放行（AnsweredBy::Policy，如实标明不是用户答的）
            let mode = self.mode().await;
            // 复核用的是**描述符给的**读写性（`req.is_write`），不是按名字回查注册表：
            // 复核与 `needs_approval` 必须同源，否则"先问后不问"会随工具名漂移。
            if !agent_adapter::approval::should_ask_approval_with_write(
                mode,
                &req.tool,
                req.is_write,
                &req.args,
                &self.config,
            ) {
                return ApprovalOutcome::allowed(AnsweredBy::Policy);
            }

            let rx = self.manager.register_waiter(&req.call_id);
            let timeout = self.timeout;

            let outcome = tokio::select! {
                answered = rx => match answered {
                    Ok(true) => ApprovalOutcome::allowed(AnsweredBy::User),
                    Ok(false) => ApprovalOutcome::denied(AnsweredBy::User, "用户拒绝了该操作"),
                    // 发送端被丢弃（例如进程关闭）→ 按中止处理，不放行
                    Err(_) => ApprovalOutcome::denied(AnsweredBy::Aborted, "审批通道已关闭"),
                },
                _ = tokio::time::sleep(timeout) => {
                    ApprovalOutcome::denied(AnsweredBy::Timeout, "审批超时未答复，按安全方向拒绝")
                }
                _ = wait_cancel(cancel) => {
                    ApprovalOutcome::denied(AnsweredBy::Aborted, "会话已中止")
                }
            };

            // 无论走哪条分支都清理 waiter：超时/中止时 `resolve_approval` 不会替我们移除，
            // 不清理就会在 map 里留下永久泄漏（幂等，正常答复路径下是空操作）。
            self.manager.remove_waiter(&req.call_id);

            outcome
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalManager;
    use agent_adapter::cancel::CancelHandle;
    use serde_json::json;

    /// S2：闸门不再需要整个 `AgentStore`——测试直接给一个固定值端口替身。
    /// 这正是端口化的收益：**测试替身变小了**（原先要构造一整个 UI 投影）。
    fn gate_with_mode(mode: ApprovalMode) -> (HostApprovalGate, Arc<ApprovalManager>) {
        let cfg = crate::node_config::FixedNodeConfig::new(
            agent_base::model::ProviderConfig::default(),
            mode,
        );
        let manager = Arc::new(ApprovalManager::new());
        let gate = HostApprovalGate::new(Arc::new(cfg), manager.clone())
            .with_timeout(Duration::from_millis(80));
        (gate, manager)
    }

    fn req(tool: &str, args: serde_json::Value) -> ApprovalRequest {
        ApprovalRequest {
            call_id: "call_1".to_string(),
            thread_id: "t1".to_string(),
            tool: tool.to_string(),
            args,
            is_write: true,
            reason: None,
        }
    }

    #[test]
    fn test_direction_is_closed() {
        let (gate, _) = gate_with_mode(ApprovalMode::Auto);
        assert_eq!(gate.direction(), FailDirection::Closed, "失败方向必须闭合");
    }

    #[tokio::test]
    async fn test_named_policy_resolves_to_real_guard_logic() {
        // Auto 档位 + 普通工具 → 不需要问（这是修掉 `Named(_) => true` 的关键断言）
        let (gate, _) = gate_with_mode(ApprovalMode::Auto);
        assert!(
            !gate
                .needs_approval(
                    "write_file",
                    &Access::Mutates { paths: agent_base::domain::PathSelector::Single("path") },
                    &ApprovalPolicy::Named("approval-guard"),
                    &json!({"path": "a.rs"}),
                )
                .await,
            "Auto 档位下普通写工具不该被 Named 策略一律要求审批"
        );
        // Auto 档位 + 危险命令 → 也不问了（Auto 是 YOLO 档，规则 0 短路一切）
        assert!(
            !gate.needs_approval(
                "run_command",
                &Access::Executes { command_arg: "command" },
                &ApprovalPolicy::Named("approval-guard"),
                &json!({"command": "rm -rf /"})
            )
            .await,
            "自动批准档位下不该再有任何审批弹窗，危险命令也不例外"
        );
        // …但换成「每次询问」档，危险命令必须拦下（危险命中判定本身没被删）
        let (ask_gate, _) = gate_with_mode(ApprovalMode::Ask);
        assert!(
            ask_gate
                .needs_approval(
                    "run_command",
                    &Access::Executes { command_arg: "command" },
                    &ApprovalPolicy::Named("approval-guard"),
                    &json!({"command": "rm -rf /"})
                )
                .await,
            "每次询问档位下危险命令必须二次确认"
        );
        assert!(
            ask_gate
                .needs_approval(
                    "read_file",
                    &Access::ReadOnly,
                    &ApprovalPolicy::Named("approval-guard"),
                    &json!({}),
                )
                .await
        );
    }

    #[tokio::test]
    async fn test_explicit_policies() {
        let (gate, _) = gate_with_mode(ApprovalMode::Auto);
        assert!(!gate.needs_approval("x", &Access::External, &ApprovalPolicy::Never, &json!({})).await);
        assert!(gate.needs_approval("x", &Access::External, &ApprovalPolicy::Always, &json!({})).await);
        assert!(
            gate.needs_approval(
                "x",
                &Access::External,
                &ApprovalPolicy::DangerScan { patterns: vec!["rm -rf".into()] },
                &json!({"cmd": "rm -rf /"})
            )
            .await
        );
        assert!(
            !gate.needs_approval(
                "x",
                &Access::External,
                &ApprovalPolicy::DangerScan { patterns: vec!["rm -rf".into()] },
                &json!({"cmd": "ls"})
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_user_approval_is_reported_as_user() {
        let (gate, manager) = gate_with_mode(ApprovalMode::Ask);
        let m = manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            m.resolve_approval("call_1", true);
        });

        let outcome = gate.decide(req("write_file", json!({"path": "a.rs"})), None).await;
        assert!(outcome.approved, "用户批准后必须放行");
        assert_eq!(outcome.by, AnsweredBy::User, "必须如实标明是用户答的");
        assert_eq!(manager.pending_count(), 0, "答复后 waiter 必须被清理");
    }

    #[tokio::test]
    async fn test_user_rejection_is_reported_with_reason() {
        let (gate, manager) = gate_with_mode(ApprovalMode::Ask);
        let m = manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            m.resolve_approval("call_1", false);
        });

        let outcome = gate.decide(req("write_file", json!({"path": "a.rs"})), None).await;
        assert!(!outcome.approved);
        assert_eq!(outcome.by, AnsweredBy::User);
        assert!(outcome.reason.as_deref().unwrap_or("").contains("拒绝"));
    }

    #[tokio::test]
    async fn test_timeout_denies_and_cleans_up() {
        let (gate, manager) = gate_with_mode(ApprovalMode::Ask);
        let outcome = gate.decide(req("write_file", json!({"path": "a.rs"})), None).await;
        assert!(!outcome.approved, "超时必须按拒绝处理（fail-closed）");
        assert_eq!(outcome.by, AnsweredBy::Timeout, "必须如实标明是超时");
        assert_eq!(manager.pending_count(), 0, "超时后 waiter 不得泄漏");
    }

    #[tokio::test]
    async fn test_cancellation_denies_and_cleans_up() {
        let (gate, manager) = gate_with_mode(ApprovalMode::Ask);
        let cancel = CancelHandle::new();
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            c.cancel();
        });

        let outcome = gate
            .decide(req("write_file", json!({"path": "a.rs"})), Some(&cancel))
            .await;
        assert!(!outcome.approved, "中止必须按拒绝处理");
        assert_eq!(outcome.by, AnsweredBy::Aborted, "必须如实标明是中止");
        assert_eq!(manager.pending_count(), 0, "中止后 waiter 不得泄漏");
    }

    #[tokio::test]
    async fn test_already_cancelled_returns_immediately() {
        let (gate, _) = gate_with_mode(ApprovalMode::Ask);
        let cancel = CancelHandle::new();
        cancel.cancel();
        let outcome = gate
            .decide(req("write_file", json!({"path": "a.rs"})), Some(&cancel))
            .await;
        assert!(!outcome.approved);
        assert_eq!(outcome.by, AnsweredBy::Aborted);
    }

    #[tokio::test]
    async fn test_policy_says_no_means_allow_without_asking() {
        // Auto 档位 + 普通工具 → decide 不该挂起等用户，而应直接按策略放行
        let (gate, manager) = gate_with_mode(ApprovalMode::Auto);
        let outcome = gate.decide(req("read_file", json!({"path": "a.rs"})), None).await;
        assert!(outcome.approved);
        assert_eq!(outcome.by, AnsweredBy::Policy, "不是用户答的，必须标 Policy");
        assert_eq!(manager.pending_count(), 0, "不该留下 waiter");
    }

    #[tokio::test]
    async fn test_readonly_mode_asks_for_write_tool_and_policy_allows_read() {
        let (gate, _) = gate_with_mode(ApprovalMode::Readonly);
        assert!(
            gate.needs_approval(
                "write_file",
                &Access::Mutates { paths: agent_base::domain::PathSelector::Single("path") },
                &ApprovalPolicy::Named("approval-guard"),
                &json!({}),
            )
                .await,
            "严格只读档位下写工具必须问"
        );
        assert!(
            !gate
                .needs_approval(
                    "read_file",
                    &Access::ReadOnly,
                    &ApprovalPolicy::Named("approval-guard"),
                    &json!({}),
                )
                .await,
            "严格只读档位下只读工具不必问"
        );
    }
}
