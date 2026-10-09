//! `invoke_subagent` 的一等 `Tool` 实现（W4-T5）。
//!
//! # 为什么它不在 `agent-toolkit` 的工具包里
//!
//! 工具包工厂是 `(workspace) -> Vec<Arc<dyn Tool>>` 这种**纯函数**形态，
//! 而子智能体委派天然是**宿主耦合**的：它需要
//!
//! - `SubagentManager`（读子智能体配置：内置 4 个 + 用户自定义）；
//! - **节点配置端口** `NodeConfigSource`（拿父会话的 provider——子智能体要用同一个模型。
//!   S2 之前这里读的是 `AgentStore`，属分层倒置，已修）；
//! - `CheckpointManager`（子智能体的写操作要能回滚）。
//!
//! 这三样都无法从一个工作区路径构造出来。所以分工是：
//!
//! | 层 | 职责 |
//! |---|---|
//! | `agent-toolkit::registry` | **描述符**（元数据单一真源，INV-3） |
//! | 本文件（`agent-core`） | **实现**（宿主耦合的那部分） |
//! | `agent-host` | **装配**（把它 `with_tool` 进 catalog） |
//!
//! # 与 legacy 的区别
//!
//! legacy 版本在 `parent_config` 缺失时会**编造一个 gemini 配置**（`gemini-2.5-flash`
//! + 空 api_key）继续跑——那会静默用一个用户没配过的模型。这里改为**直接读父会话的
//! provider 配置**，读不到就如实失败。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use super::types::SubagentProfile;
use super::{RunSubagentOptions, SubagentManager};
use crate::checkpoint::CheckpointManager;
use crate::node_config::NodeConfigSource;

/// 委派给子智能体的工具。
pub struct InvokeSubagentTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
    /// 节点配置端口（S2）。
    ///
    /// 原先直接读 `AgentStore` 拿父会话的 provider——节点层读 UI 投影是分层倒置。
    /// 现在只认端口，生产实现由组合根注入（`server::StoreBackedNodeConfig`）。
    config_source: Arc<dyn NodeConfigSource>,
    subagent_mgr: Arc<SubagentManager>,
    checkpoint_mgr: Option<Arc<CheckpointManager>>,
    /// 产品的工具包声明（`spec.toolkits`）：子智能体按**同一处装配**取工具（W4-T6）
    toolkits: Vec<String>,
}

impl InvokeSubagentTool {
    /// 描述符取自注册表——实现了却没声明会**立刻 panic**，而不是造出一个没有描述符的工具。
    pub fn new(
        workspace: impl Into<PathBuf>,
        config_source: Arc<dyn NodeConfigSource>,
        subagent_mgr: Arc<SubagentManager>,
        checkpoint_mgr: Option<Arc<CheckpointManager>>,
        toolkits: Vec<String>,
    ) -> Self {
        let descriptor = crate::tools::find_tool_descriptor("invoke_subagent")
            .expect("`invoke_subagent` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();
        Self {
            descriptor,
            workspace: workspace.into(),
            config_source,
            subagent_mgr,
            checkpoint_mgr,
            toolkits,
        }
    }

    fn resolve_profile(&self, subagent_id: &str) -> Option<SubagentProfile> {
        self.subagent_mgr
            .get_profile(subagent_id, Some(&self.workspace))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 父会话取消的轮询间隔（W4-T3）。与 `cmd_tools::CANCEL_POLL_INTERVAL` 同口径。
const CANCEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

impl Tool for InvokeSubagentTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        let _ctx = ctx;
        Box::pin(async move {
            let started_at = now_ms();

            let subagent_id = call
                .args
                .get("subagent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let task = call.args.get("task").and_then(|v| v.as_str()).unwrap_or("");
            if subagent_id.trim().is_empty() || task.trim().is_empty() {
                return ToolReceipt::error(
                    "缺少必填参数 `subagent_id` / `task`",
                    started_at,
                    now_ms(),
                );
            }

            let Some(profile) = self.resolve_profile(subagent_id) else {
                return ToolReceipt::error(
                    format!("找不到指定的子智能体配置: {subagent_id}"),
                    started_at,
                    now_ms(),
                );
            };
            if !profile.enabled {
                return ToolReceipt::error(
                    format!("子智能体 [{}] 已被禁用", profile.name),
                    started_at,
                    now_ms(),
                );
            }

            // 父会话的 provider 配置：子智能体用**同一个**模型。
            // 读不到就如实失败——legacy 会在这里编造一个 gemini 配置继续跑，那更糟。
            //
            // S2：经端口读，不直接读 `AgentStore`。
            let parent_config = self.config_source.provider();
            if parent_config.base_url.trim().is_empty() {
                return ToolReceipt::error(
                    "父会话没有可用的 provider 配置，无法委派子智能体（请先在设置里配置模型）",
                    started_at,
                    now_ms(),
                );
            }

            let additional_context = call
                .args
                .get("additional_context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let res = {
                // W4-T3：父会话的取消必须**真的**传到子智能体。
                //
                // `ToolContext.cancel` 是 `&'a dyn CancelToken`（轮询式），不能 move 进
                // `tokio::spawn`（生命周期不够），所以用 `select!` 在**同一个 future** 里
                // 轮询转发到 `run_subagent` 的 `abort_rx`——与 W4-T1 修 `run_command`
                // 假接线用的是同一个手法。
                let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
                let fut = super::run_subagent(RunSubagentOptions {
                    profile,
                    task: task.to_string(),
                    additional_context,
                    workspace: self.workspace.clone(),
                    parent_config,
                    checkpoint_mgr: self.checkpoint_mgr.clone(),
                    abort_rx: Some(abort_rx),
                    update_tx: None,
                    toolkits: self.toolkits.clone(),
                    model: None,
                });
                tokio::pin!(fut);
                loop {
                    tokio::select! {
                        r = &mut fut => break r,
                        _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                            if _ctx.cancel.is_cancelled() {
                                let _ = abort_tx.send(true);
                            }
                        }
                    }
                }
            };

            let finished_at = now_ms();
            let status = if res.ok { ToolStatus::Success } else { ToolStatus::Error };
            let mut receipt = ToolReceipt::new(status, res.summary.clone(), started_at, finished_at);
            // 结构化细节：步数/耗时/工具调用数/输出文件，供界面与诊断使用
            receipt.details = serde_json::to_value(&res).ok();
            if !res.ok {
                if let Some(err) = &res.error_message {
                    receipt.output = format!("{}\n{}", res.summary, err);
                }
            }
            receipt
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{AgentEvent, DenialKind};
    use agent_base::ports::{CancelToken, EventSink, Scope};
    use serde_json::json;

    struct NoopSink;
    impl EventSink for NoopSink {
        fn emit(&self, _event: AgentEvent) {}
    }
    struct NoCancel;
    impl CancelToken for NoCancel {
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    struct RootScope(PathBuf);
    impl Scope for RootScope {
        fn id(&self) -> &str {
            "test_root"
        }
        fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
            Ok(self.0.join(raw))
        }
    }

    fn ctx_parts(ws: &std::path::Path) -> (RootScope, NoCancel, NoopSink) {
        (RootScope(ws.to_path_buf()), NoCancel, NoopSink)
    }

    /// S2：测试替身从"一整个 `AgentStore`"缩小成"一个固定值配置端口"。
    fn tool(ws: &std::path::Path, provider: agent_base::model::ProviderConfig) -> InvokeSubagentTool {
        let cfg = crate::node_config::FixedNodeConfig::new(provider, agent_proto::ApprovalMode::Auto);
        InvokeSubagentTool::new(
            ws,
            Arc::new(cfg),
            Arc::new(SubagentManager::new()),
            None,
            vec!["core".to_string(), "fs".to_string()],
        )
    }

    fn default_provider() -> agent_base::model::ProviderConfig {
        let mut p = agent_base::model::ProviderConfig::default();
        p.base_url = "https://example.invalid/v1".to_string();
        p
    }

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: "invoke_subagent".into(), args }
    }

    /// 描述符必须来自注册表（INV-3），且**不能**被标成只读。
    #[test]
    fn test_descriptor_comes_from_registry_and_is_not_readonly() {
        let ws = std::env::current_dir().unwrap();
        let t = tool(&ws, default_provider());
        let from_registry = crate::tools::find_tool_descriptor("invoke_subagent").expect("注册表里必须有");
        assert_eq!(t.descriptor(), from_registry, "描述符必须与注册表逐字段一致");
        assert!(
            !t.descriptor().is_readonly(),
            "委派会驱动能写文件的子智能体，绝不能标成只读（AGENTS.md §2 失败安全）"
        );
    }

    #[tokio::test]
    async fn test_missing_args_is_an_error() {
        let ws = std::env::current_dir().unwrap();
        let t = tool(&ws, default_provider());
        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = t.execute(&call(json!({ "subagent_id": "general_purpose" })), &ctx).await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(r.output.contains("task"), "{}", r.output);
    }

    #[tokio::test]
    async fn test_unknown_profile_is_an_error_not_a_silent_success() {
        let ws = std::env::current_dir().unwrap();
        let t = tool(&ws, default_provider());
        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = t
            .execute(&call(json!({ "subagent_id": "no_such_profile", "task": "x" })), &ctx)
            .await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(r.output.contains("找不到"), "{}", r.output);
    }

    /// 内置 profile 存在但父会话没配 provider → 如实失败（**不编造** gemini 配置）。
    #[tokio::test]
    async fn test_missing_parent_provider_fails_instead_of_fabricating_one() {
        let ws = std::env::current_dir().unwrap();
        let mut empty = agent_base::model::ProviderConfig::default();
        empty.base_url = String::new();
        empty.api_key = String::new();
        let t = tool(&ws, empty);
        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = t
            .execute(&call(json!({ "subagent_id": "general_purpose", "task": "做点事" })), &ctx)
            .await;
        assert_eq!(r.status, ToolStatus::Error, "{}", r.output);
        assert!(
            r.output.contains("provider"),
            "错误信息应指出是 provider 配置问题：{}",
            r.output
        );
    }
}
