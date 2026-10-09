//! `AgentBus` 的**本地实现**（S3）：进程内、一次性、临时上下文。
//!
//! 这不是新写的逻辑——它是 `invoke_subagent` 原先那段派活代码**整体搬进来**的，
//! 目的是让"本地委派"与"经网关委派"成为**同一语义的两个实现**，而不是两套机制。
//! 搬移的判据是：**行为逐字不变**（原有工具测试全绿，且 `details` 的 JSON 形状不变）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_base::model::ProviderConfig;
use agent_base::ports::BoxFuture;

use super::{RunSubagentOptions, SubagentManager};
use crate::agent_bus::{AgentBus, AgentHandle, DispatchOutcome, DispatchRequest};
use crate::checkpoint::CheckpointManager;
use crate::node_config::NodeConfigSource;

/// 父会话取消的轮询间隔（W4-T3）。与 `cmd_tools::CANCEL_POLL_INTERVAL` 同口径。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 进程内委派：跑一个**临时上下文**的子智能体（`EphemeralSessionStore`），跑完即散。
pub struct LocalAgentBus {
    workspace: PathBuf,
    subagent_mgr: Arc<SubagentManager>,
    /// 父会话的模型配置：子智能体用**同一个**模型（S2 之后经端口读，不读 UI 投影）
    config_source: Arc<dyn NodeConfigSource>,
    checkpoint_mgr: Option<Arc<CheckpointManager>>,
    /// 产品的工具包声明：子智能体按**同一处装配**取工具（W4-T6）
    toolkits: Vec<String>,
}

impl LocalAgentBus {
    pub fn new(
        workspace: impl Into<PathBuf>,
        subagent_mgr: Arc<SubagentManager>,
        config_source: Arc<dyn NodeConfigSource>,
        checkpoint_mgr: Option<Arc<CheckpointManager>>,
        toolkits: Vec<String>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            subagent_mgr,
            config_source,
            checkpoint_mgr,
            toolkits,
        }
    }
}

impl AgentBus for LocalAgentBus {
    fn list_agents(&self) -> BoxFuture<'_, Vec<AgentHandle>> {
        Box::pin(async move {
            self.subagent_mgr
                .list_profiles(Some(&self.workspace))
                .into_iter()
                .map(|p| AgentHandle {
                    id: p.id,
                    name: p.name,
                    description: p.description,
                    enabled: p.enabled,
                })
                .collect()
        })
    }

    fn dispatch<'a>(&'a self, req: DispatchRequest<'a>) -> BoxFuture<'a, DispatchOutcome> {
        Box::pin(async move {
            // 1. 目标必须存在——**不编造**、不"随便挑一个继续跑"
            let Some(profile) = self
                .subagent_mgr
                .get_profile(req.agent_id, Some(&self.workspace))
            else {
                return DispatchOutcome::rejected(format!(
                    "找不到指定的子智能体配置: {}",
                    req.agent_id
                ));
            };
            if !profile.enabled {
                return DispatchOutcome::rejected(format!("子智能体 [{}] 已被禁用", profile.name));
            }

            // 2. 父会话的 provider 配置：子智能体用**同一个**模型。
            //    读不到就如实失败——legacy 会在这里编造一个 gemini 配置继续跑，那更糟。
            let parent_config: ProviderConfig = self.config_source.provider();
            if parent_config.base_url.trim().is_empty() {
                return DispatchOutcome::rejected(
                    "父会话没有可用的 provider 配置，无法委派子智能体（请先在设置里配置模型）",
                );
            }

            // 3. 跑。父会话的取消必须**真的**传到子智能体（W4-T3）。
            //
            // `req.cancel` 是 `&'a dyn CancelToken`（轮询式），不能 move 进
            // `tokio::spawn`（生命周期不够），所以用 `select!` 在**同一个 future** 里
            // 轮询转发到 `run_subagent` 的 `abort_rx`——与 W4-T1 修 `run_command`
            // 假接线用的是同一个手法。
            let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
            let fut = super::run_subagent(RunSubagentOptions {
                profile,
                task: req.task.to_string(),
                additional_context: req.additional_context.clone(),
                workspace: self.workspace.clone(),
                parent_config,
                checkpoint_mgr: self.checkpoint_mgr.clone(),
                abort_rx: Some(abort_rx),
                // S3 保持与原实现一致：进度通道仍未接（P1-17 登记在案，S6 一并解决）
                update_tx: None,
                toolkits: self.toolkits.clone(),
                model: None,
            });
            tokio::pin!(fut);
            let res = loop {
                tokio::select! {
                    r = &mut fut => break r,
                    _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                        if req.cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
                            let _ = abort_tx.send(true);
                        }
                    }
                }
            };

            // 4. 如实回执：`details` 与重构前**逐字节一致**（同一个 `SubagentRunResult`）
            DispatchOutcome {
                ok: res.ok,
                summary: res.summary.clone(),
                error_message: res.error_message.clone(),
                details: serde_json::to_value(&res).ok(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_config::FixedNodeConfig;
    use agent_proto::ApprovalMode;

    fn bus_with(provider: ProviderConfig) -> LocalAgentBus {
        let cfg = FixedNodeConfig::new(provider, ApprovalMode::Auto);
        LocalAgentBus::new(
            std::env::current_dir().unwrap(),
            Arc::new(SubagentManager::new()),
            Arc::new(cfg),
            None,
            vec!["core".to_string()],
        )
    }

    fn ok_provider() -> ProviderConfig {
        let mut p = ProviderConfig::default();
        p.base_url = "https://example.invalid/v1".to_string();
        p
    }

    /// 发现：内置 profile 必须在列表里，且带 enabled 状态。
    #[tokio::test]
    async fn test_list_agents_exposes_builtin_profiles() {
        let bus = bus_with(ok_provider());
        let agents = bus.list_agents().await;
        assert!(!agents.is_empty(), "内置子智能体 profile 不该为空");
        assert!(
            agents.iter().any(|a| a.id == "general_purpose"),
            "应包含 general_purpose：{:?}",
            agents.iter().map(|a| &a.id).collect::<Vec<_>>()
        );
        assert!(
            agents.iter().all(|a| !a.name.is_empty()),
            "每个 handle 都要有展示名（错误信息用它）"
        );
    }

    /// **不编造**：未知 agent → `details: None`（未进入执行）+ 精确原因。
    #[tokio::test]
    async fn test_dispatch_unknown_agent_is_rejected_without_executing() {
        let bus = bus_with(ok_provider());
        let out = bus
            .dispatch(DispatchRequest {
                agent_id: "no_such_agent",
                task: "做点事",
                additional_context: None,
                cancel: None,
                depth: 0,
            })
            .await;

        assert!(!out.ok);
        assert_eq!(out.details, None, "未进入执行 → details 必须是 None");
        assert_eq!(out.error_message, None);
        assert!(
            out.summary.contains("找不到指定的子智能体配置"),
            "{}",
            out.summary
        );
    }

    /// **不编造**：没有可用模型 → 如实拒绝，而不是挑个默认配置继续跑。
    #[tokio::test]
    async fn test_dispatch_without_provider_is_rejected_without_executing() {
        let mut empty = ProviderConfig::default();
        empty.base_url = String::new();
        let bus = bus_with(empty);

        let out = bus
            .dispatch(DispatchRequest {
                agent_id: "general_purpose",
                task: "做点事",
                additional_context: None,
                cancel: None,
                depth: 0,
            })
            .await;

        assert!(!out.ok);
        assert_eq!(out.details, None, "未进入执行 → details 必须是 None");
        assert!(
            out.summary.contains("provider"),
            "错误信息应指出是 provider 问题：{}",
            out.summary
        );
    }

    /// 端口语义：`rejected` 的构造必须与"未进入执行"一致（details 为 None）。
    #[test]
    fn test_rejected_outcome_never_carries_details() {
        let o = DispatchOutcome::rejected("x");
        assert!(!o.ok);
        assert_eq!(o.details, None);
        assert_eq!(o.error_message, None);
    }
}
