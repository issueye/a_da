//! `invoke_subagent` 的一等 `Tool` 实现（W4-T5；S3 改为 `AgentBus` 端口上的薄适配器）。
//!
//! # 它现在做什么
//!
//! 只做两件事：**参数校验**（`subagent_id` / `task` 必填）与**回执映射**
//! （`DispatchOutcome` → `ToolReceipt`）。派活语义——目标是否存在、用哪个模型、
//! 怎么跑、怎么取消——全在 [`crate::agent_bus::AgentBus`] 的实现里。
//!
//! ```text
//! invoke_subagent (本文件) ──► AgentBus 端口 ──┬─ LocalAgentBus（进程内，今天的行为）
//!                                             └─ GatewayAgentBus（S6，远端 agent 实例）
//! ```
//!
//! 这样"本地委派"与"经网关委派"是**同一语义的两个实现**，而不是两套机制——
//! 本仓已经为"两份实现"付过代价（INV-1、`verify-archive` 的第二份引擎断言）。
//!
//! # 为什么它不在 `agent-toolkit` 的工具包里
//!
//! 工具包工厂是 `(workspace) -> Vec<Arc<dyn Tool>>` 这种**纯函数**形态，
//! 而委派总线天然是**宿主耦合**的（要 `SubagentManager` / 节点配置 / `CheckpointManager`），
//! 这些都无法从一个工作区路径构造出来。所以分工是：
//!
//! | 层 | 职责 |
//! |---|---|
//! | `agent-toolkit::registry` | **描述符**（元数据单一真源，INV-3） |
//! | 本文件（`agent-core`） | **工具适配**（参数校验 + 回执映射） |
//! | `subagents::local_bus` | **派活实现**（宿主耦合的那部分） |
//! | `agent-host` | **装配**（构造总线 + 把它 `with_tool` 进 catalog） |
//!
//! # 与 legacy 的区别
//!
//! legacy 版本在 `parent_config` 缺失时会**编造一个 gemini 配置**（`gemini-2.5-flash`
//! + 空 api_key）继续跑——那会静默用一个用户没配过的模型。
//! 现在由总线实现**如实拒绝**（`DispatchOutcome::rejected`），`details` 为 `None`
//! 表示"未进入执行"，与"跑了但失败"可区分。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::agent_bus::{AgentBus, DispatchRequest};

/// 委派给子智能体的工具。
///
/// S3：本工具是 `AgentBus` 端口上的**薄适配器**——它只做参数校验与回执映射，
/// 派活语义（目标是否存在、用哪个模型、怎么跑、怎么取消）全在总线实现里。
///
/// 这样"本地委派"与"经网关委派"是**同一语义的两个实现**，而不是两套机制。
pub struct InvokeSubagentTool {
    descriptor: ToolDescriptor,
    /// 委派总线（S3）。生产实现 = `subagents::local_bus::LocalAgentBus`；
    /// 网关实现（S6）会替换它，而本文件**不需要改**。
    bus: Arc<dyn AgentBus>,
    /// 本节点的委派深度（S6）：由组合根按**产品声明**给出。
    ///
    /// 本地总线不使用它（进程内递归由 `NEVER_FOR_SUBAGENT` 拦）；
    /// 网关总线把它带进 `gateway.delegate`，由网关强制上限。
    /// 装配期给值而不是运行时猜——深度是"这个实例在委派链上的位置"，属于装配事实。
    delegation_depth: u32,
}

impl InvokeSubagentTool {
    /// 描述符取自注册表——实现了却没声明会**立刻 panic**，而不是造出一个没有描述符的工具。
    pub fn new(bus: Arc<dyn AgentBus>, delegation_depth: u32) -> Self {
        let descriptor = crate::tools::find_tool_descriptor("invoke_subagent")
            .expect("`invoke_subagent` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();
        Self { descriptor, bus, delegation_depth }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Tool for InvokeSubagentTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
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

            // ── S3：派活语义全在 `AgentBus` 实现里，这里只做参数校验与回执映射 ──
            let additional_context = call
                .args
                .get("additional_context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let out = self
                .bus
                .dispatch(DispatchRequest {
                    agent_id: subagent_id,
                    task,
                    additional_context,
                    // 父会话取消令牌：取消必须**真的**传到 agent 内部（W4-T3）
                    cancel: Some(ctx.cancel),
                    // 委派深度：跨网关时由网关强制上限（S6）
                    depth: self.delegation_depth,
                })
                .await;

            let finished_at = now_ms();

            // 未进入执行（目标不存在 / 已禁用 / 没有可用模型）→ 与重构前逐字一致：
            // `ToolReceipt::error(原因)`，`details` 为 None。
            let Some(details) = out.details else {
                return ToolReceipt::error(out.summary, started_at, finished_at);
            };

            let status = if out.ok {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };
            let mut receipt = ToolReceipt::new(status, out.summary.clone(), started_at, finished_at);
            // 结构化细节：步数/耗时/工具调用数/输出文件，供界面与诊断使用
            receipt.details = Some(details);
            if !out.ok {
                if let Some(err) = &out.error_message {
                    receipt.output = format!("{}\n{}", out.summary, err);
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
    use std::path::PathBuf;

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

    /// S3：工具经 `AgentBus` 端口派活。测试替身因此缩成"一个固定值配置端口 + 本地总线"。
    fn tool(ws: &std::path::Path, provider: agent_base::model::ProviderConfig) -> InvokeSubagentTool {
        let cfg = crate::node_config::FixedNodeConfig::new(provider, agent_proto::ApprovalMode::Auto);
        let bus = crate::subagents::local_bus::LocalAgentBus::new(
            ws,
            Arc::new(crate::subagents::SubagentManager::new()),
            Arc::new(cfg),
            None,
            vec!["core".to_string(), "fs".to_string()],
        );
        InvokeSubagentTool::new(Arc::new(bus), 0)
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
