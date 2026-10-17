//! 引擎桥（W3-T2 引入，W3-T4 收敛为**唯一**路径）。
//!
//! # 它现在是什么
//!
//! legacy 主循环 `run_agent_loop` 已在 W3-T4 删除：仓库里只剩 `agent-base` 一份多轮循环
//! （`AgentRuntime::run_turn`，INV-1）。这个模块做两件事：
//!
//! 1. [`LoopEventBridge`]：把领域事件 `AgentEvent` 投影成界面事件 [`AgentLoopEvent`]，
//!    推给 `dispatch.rs` 里那套**不该动的** UI 投影代码；
//! 2. [`run_agent_turn`]：3 个调用点的统一入口——**没有引擎就直接失败**，
//!    不再有"悄悄退回 legacy"这种把配置错误藏起来的行为。
//!
//! ```text
//! AgentRuntime::run_turn ──emit──▶ AgentEvent ──LoopEventBridge──▶ AgentLoopEvent ──▶ AgentStore ──▶ 界面
//! ```
//!
//! # 历史
//!
//! W3-T2 时期这里还有一个 `A_DA_ENGINE` 开关（默认 legacy、可回滚）。W3-T4 把 legacy
//! 删掉之后，开关与"降级"语义一并移除——**"没注入引擎"是装配错误，不是运行时可选项**。

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::engine::{AgentRuntime, TurnRequest};
use agent_base::ports::EventSink;
use agent_adapter::cancel::WatchedCancel;
use thiserror::Error;

use crate::seq_guard::SeqGuard;
use tokio::sync::{mpsc, watch};
use tracing::warn;

use super::ui_events::AgentLoopEvent;
use agent_base::model::ProviderConfig;

/// 宿主未装配引擎时的**硬失败**（不再静默降级）。
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(
        "宿主没有注入 AgentRuntime：legacy 主循环已在 W3-T4 删除，必须由宿主装配真引擎\
         （用 agent_host::build_engine_injection(...) 后传给 WsHostServer::bind_with_engine）"
    )]
    EngineNotInjected,
}

/// 领域事件 → legacy 事件（UI 投影路径不变的关键）。
///
/// 返回 `None` 表示"这个领域事件在 legacy 事件集里没有对应物"——**不是错误**，
/// 而是两代事件集的差集（例如 `TurnStarted` / 子智能体事件 / 审批事件）。
/// 刻意显式列出，免得将来有人以为漏了分支。
pub fn project_to_loop_event(event: &AgentEvent) -> Option<AgentLoopEvent> {
    match &event.body {
        AgentEventBody::ThinkingDelta { text } => Some(AgentLoopEvent::Thinking { text: text.clone() }),
        AgentEventBody::TextDelta { text } => Some(AgentLoopEvent::TextDelta { text: text.clone() }),
        AgentEventBody::ToolCallStarted { call_id, name, args } => {
            Some(AgentLoopEvent::ToolCallStarted {
                name: name.clone(),
                id: call_id.clone(),
                args: args.to_string(),
            })
        }
        AgentEventBody::ToolCallFinished { call_id, name, receipt } => {
            Some(AgentLoopEvent::ToolCallFinished {
                name: name.clone(),
                id: call_id.clone(),
                ok: receipt.ok(),
                output: Some(receipt.output.clone()),
                duration_ms: Some(receipt.duration_ms()),
                started_at: Some(receipt.started_at),
                finished_at: Some(receipt.finished_at),
                status: Some(tool_status_name(receipt.status)),
                details: receipt.details.clone(),
            })
        }
        AgentEventBody::QuestionAsked { call_id, question } => {
            Some(AgentLoopEvent::ToolAwaitingQuestion {
                id: call_id.clone(),
                question: question.clone(),
            })
        }
        // W3-T3：审批请求必须有界面通道，否则处于"询问档"时工具会静默等满超时（P1-14）
        AgentEventBody::ApprovalRequested { call_id, tool, args } => {
            Some(AgentLoopEvent::ApprovalRequested {
                id: call_id.clone(),
                tool: tool.clone(),
                // 参数原文照搬：审批卡要摆出"要批准什么"，不能只剩一个工具名
                args: args.clone(),
            })
        }
        AgentEventBody::TurnFinished { stop } => Some(AgentLoopEvent::TurnFinished {
            stop_reason: stop_reason_name(stop),
        }),
        AgentEventBody::Failed { message } => Some(AgentLoopEvent::Error { message: message.clone() }),
        // 无对应物（两代事件集的差集）
        AgentEventBody::TurnStarted
        | AgentEventBody::SubagentStarted { .. }
        | AgentEventBody::SubagentFinished { .. }
        | AgentEventBody::UsageReported { .. }
        // P0 埋点：诊断遥测**刻意不进 legacy UI 事件集**——它只给人看（诊断面板/日志），
        // 不进会话流。留在领域事件层由 dispatch 的投影按需消费。
        | AgentEventBody::Diagnostic(_) => None,
    }
}

fn tool_status_name(status: agent_base::domain::ToolStatus) -> String {
    use agent_base::domain::ToolStatus;
    match status {
        ToolStatus::Success => "success",
        ToolStatus::Error => "error",
        ToolStatus::Denied => "denied",
        ToolStatus::Timeout => "timeout",
        ToolStatus::Aborted => "aborted",
    }
    .to_string()
}

/// 停止原因 → 界面文案（与 legacy 的 `stop_reason` 字符串保持一致口径）。
pub fn stop_reason_name(stop: &TurnStopReason) -> String {
    match stop {
        TurnStopReason::Completed => "stop",
        TurnStopReason::Aborted => "aborted",
        TurnStopReason::ModelError => "error",
        TurnStopReason::BudgetExhausted { .. } => "length",
        // 上下文撞顶同样是"没跑完"：界面按 length 处理（真正的理由在 Failed 事件与回执里）
        TurnStopReason::ContextWindowExceeded { .. } => "length",
        TurnStopReason::Denied => "denied",
    }
    .to_string()
}

/// 把真引擎的领域事件桥接回 legacy 事件通道（供既有 UI 投影消费）。
///
/// 另外做两件 legacy 路径也做的事：
/// 1. **seq 违约记账**（INV-6）：不丢事件，但如实记录；
/// 2. **用量事件补发** `AssistantStats`：真引擎的 `UsageReported` 在 legacy 事件集里
///    没有对应物，但界面遥测条依赖 `AssistantStats`，所以在这里补出来。
pub struct LoopEventBridge {
    tx: mpsc::Sender<AgentLoopEvent>,
    /// INV-6 的 `seq` 单调性审计（一处实现，三个出口共用）
    seq: SeqGuard,
    started: Instant,
}

impl LoopEventBridge {
    pub fn new(tx: mpsc::Sender<AgentLoopEvent>) -> Self {
        Self {
            tx,
            seq: SeqGuard::new(),
            started: Instant::now(),
        }
    }

    pub fn seq_violations(&self) -> Vec<String> {
        self.seq.violations()
    }
}

impl EventSink for LoopEventBridge {
    fn emit(&self, event: AgentEvent) {
        // INV-6：seq 必须严格递增；违约如实记账但**仍然转发**（丢事件会让界面少显示东西）
        self.seq.check(&event);

        // 用量事件在 legacy 事件集里没有对应物，单独补成 AssistantStats
        if let AgentEventBody::UsageReported { usage, duration_ms } = &event.body {
            let _ = self.tx.try_send(AgentLoopEvent::AssistantStats {
                usage: Some(usage.clone()),
                duration_ms: *duration_ms,
                // 已知缺口：本类型没有时钟端口，用桥创建时刻起算的单调耗时。
                // 与 `ToolContext` 缺时钟是同一个缺口，随端口补齐一并收敛。
                turn_duration_ms: self.started.elapsed().as_millis() as u64,
            });
            return;
        }

        if let Some(loop_event) = project_to_loop_event(&event) {
            // `try_send`：容量满时丢弃而不是阻塞引擎。
            // 阻塞引擎会让"事件消费慢"变成"模型停摆"，代价远大于丢一条增量。
            let _ = self.tx.try_send(loop_event);
        }
    }
}

/// 直接通道事件出口（保持 INV-6 的 seq 递增审计）。
pub struct DirectEventSink {
    tx: mpsc::Sender<AgentEvent>,
    /// INV-6 的 `seq` 单调性审计（一处实现，三个出口共用）
    seq: SeqGuard,
}

/// **关键事件**：这些事件被丢掉，界面就会"不知道有人在等你/这一轮已经结束"，
/// 表现为静默卡死（用户看不到审批卡、也等不到收尾）。
///
/// 增量类事件（`TextDelta`/`ThinkingDelta`）丢一条无所谓——下一帧会补上；
/// 关键事件是**状态跃迁**，丢一次就永久对不上。
fn is_critical_event(body: &AgentEventBody) -> bool {
    matches!(
        body,
        AgentEventBody::ApprovalRequested { .. }
            | AgentEventBody::QuestionAsked { .. }
            | AgentEventBody::ToolCallFinished { .. }
            | AgentEventBody::TurnFinished { .. }
            | AgentEventBody::Failed { .. }
    )
}

/// 关键事件在通道满时的退避重试：最多等约 1 秒（100 × 10ms）。
const CRITICAL_SEND_RETRIES: usize = 100;
const CRITICAL_SEND_BACKOFF: Duration = Duration::from_millis(10);

impl DirectEventSink {
    pub fn new(tx: mpsc::Sender<AgentEvent>) -> Self {
        Self {
            tx,
            seq: SeqGuard::new(),
        }
    }

    pub fn seq_violations(&self) -> Vec<String> {
        self.seq.violations()
    }
}

impl EventSink for DirectEventSink {
    fn emit(&self, event: AgentEvent) {
        // INV-6：seq 必须严格递增；违约如实记账但**仍然转发**（丢事件会让界面少显示东西）
        self.seq.check(&event);
        // 🔴 关键事件**绝不静默丢弃**：通道满时做**有界**退避重试（上限约 1 秒）。
        //
        // 丢一条增量只是少刷一帧文字；丢一次 `ApprovalRequested` / `TurnFinished`，
        // 界面就永远不知道"有人在等你"或"这轮已经结束"——那是静默卡死，用户没有任何线索。
        if is_critical_event(&event.body) {
            let mut pending = event;
            for _ in 0..CRITICAL_SEND_RETRIES {
                match self.tx.try_send(pending) {
                    Ok(()) => return,
                    Err(mpsc::error::TrySendError::Full(back)) => {
                        pending = back;
                        std::thread::sleep(CRITICAL_SEND_BACKOFF);
                    }
                    // 接收端已关闭（本轮已收摊）——没有消费者了，重试没有意义
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
            warn!(
                "关键事件在通道持续拥塞下仍未送达（kind={}）：界面可能看不到这次状态跃迁",
                pending.body.kind()
            );
            return;
        }
        let _ = self.tx.try_send(event);
    }
}

/// 用真引擎跑一轮，并把领域事件送入通道。
///
/// `max_retries` = 模型请求重试次数（设置项「请求重试次数」，默认 5）。
pub async fn run_turn_with_engine(
    engine: Arc<AgentRuntime>,
    thread_id: &str,
    user_prompt: Option<&str>,
    // 本轮用户附带的图片（**data URI**）。`agent-base` 是零 IO 层，路径转 base64 已在
    // `dispatch` 入口做完（见那里的 `normalize_images`）。
    images: Option<Vec<String>>,
    provider_config: ProviderConfig,
    context_window: Option<u64>,
    max_retries: Option<usize>,
    event_tx: mpsc::Sender<AgentEvent>,
    abort_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<()> {
    let sink = DirectEventSink::new(event_tx);
    let cancel = match abort_rx {
        Some(rx) => WatchedCancel::new(rx),
        None => WatchedCancel::never(),
    };

    let mut req = TurnRequest::new(thread_id, provider_config)
        .with_context_window(context_window)
        .with_max_retries(max_retries);
    if let Some(p) = user_prompt {
        if !p.trim().is_empty() {
            req = req.with_user_prompt(p);
        }
    }
    // 只在真有图片时才登记：`with_images(None)` 与不调用等价，但显式过滤掉空数组
    // 能避免 "用户发了张空图" 这种状态在引擎里留下 `Some(vec![])`。
    if let Some(imgs) = images {
        if !imgs.is_empty() {
            req = req.with_images(imgs);
        }
    }

    let outcome = engine.run_turn(req, &sink, &cancel).await;

    // seq 违约不吞：如实打日志（INV-6 的诊断口径）
    for v in sink.seq_violations() {
        warn!("{}", v);
    }

    match outcome {
        Ok(_) => Ok(()),
        Err(e) => {
            // 引擎错误也要变成通道可见的事件，而不是静默消失
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let _ = sink.tx.try_send(AgentEvent::new(
                0,
                now,
                thread_id,
                AgentEventBody::Failed { message: e.to_string() },
            ));
            Err(anyhow::anyhow!("引擎执行失败：{e}"))
        }
    }
}

/// **3 个调用点的统一入口**（W3-T4 后不再有第二条路径）。
///
/// `engine` 为 `None` → [`EngineError::EngineNotInjected`] **硬失败**：
/// legacy 主循环已删除，"没装配引擎"是配置错误，必须显式暴露而不是悄悄降级。
pub async fn run_agent_turn(
    engine: Option<Arc<AgentRuntime>>,
    thread_id: &str,
    user_prompt: Option<&str>,
    images: Option<Vec<String>>,
    provider_config: ProviderConfig,
    context_window: Option<u64>,
    max_retries: Option<usize>,
    event_tx: mpsc::Sender<AgentEvent>,
    abort_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<()> {
    let engine = engine.ok_or(EngineError::EngineNotInjected)?;
    run_turn_with_engine(
        engine,
        thread_id,
        user_prompt,
        images,
        provider_config,
        context_window,
        max_retries,
        event_tx,
        abort_rx,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{AgentEventBody, ToolReceipt, ToolStatus};
    use agent_base::model::TokenUsage;

    fn ev(seq: u64, body: AgentEventBody) -> AgentEvent {
        AgentEvent::new(seq, 1000 + seq as i64, "t1", body)
    }

    /// 投影必须覆盖"两代事件集的交集"，且差集**显式**返回 None。
    #[test]
    fn test_projection_covers_intersection_and_declares_the_gap() {
        // 交集
        assert!(matches!(
            project_to_loop_event(&ev(1, AgentEventBody::ThinkingDelta { text: "x".into() })),
            Some(AgentLoopEvent::Thinking { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(2, AgentEventBody::TextDelta { text: "x".into() })),
            Some(AgentLoopEvent::TextDelta { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(
                3,
                AgentEventBody::ToolCallStarted {
                    call_id: "c1".into(),
                    name: "todo".into(),
                    args: serde_json::json!({"a":1}),
                }
            )),
            Some(AgentLoopEvent::ToolCallStarted { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(
                4,
                AgentEventBody::ToolCallFinished {
                    call_id: "c1".into(),
                    name: "todo".into(),
                    receipt: ToolReceipt::success("ok", 10, 30),
                }
            )),
            Some(AgentLoopEvent::ToolCallFinished { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(
                5,
                AgentEventBody::QuestionAsked { call_id: "c1".into(), question: serde_json::json!({}) }
            )),
            Some(AgentLoopEvent::ToolAwaitingQuestion { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(
                6,
                AgentEventBody::TurnFinished { stop: TurnStopReason::Completed }
            )),
            Some(AgentLoopEvent::TurnFinished { .. })
        ));
        assert!(matches!(
            project_to_loop_event(&ev(7, AgentEventBody::Failed { message: "boom".into() })),
            Some(AgentLoopEvent::Error { .. })
        ));

        // 差集：显式 None（不是漏了分支）
        assert!(project_to_loop_event(&ev(8, AgentEventBody::TurnStarted)).is_none());
        assert!(project_to_loop_event(&ev(
            9,
            AgentEventBody::UsageReported { usage: TokenUsage::default(), duration_ms: 1 }
        ))
        .is_none());
    }

    /// 工具回执必须**结构化**穿过桥（AGENTS.md §18：裸文本会让界面耗时/状态徽章静默变空）。
    #[test]
    fn test_tool_receipt_survives_projection_with_structure() {
        let receipt = ToolReceipt {
            status: ToolStatus::Error,
            output: "失败了".into(),
            data: None,
            details: None,
            started_at: 1_000,
            finished_at: 1_250,
        };
        let projected = project_to_loop_event(&ev(
            1,
            AgentEventBody::ToolCallFinished {
                call_id: "c9".into(),
                name: "run_command".into(),
                receipt,
            },
        ))
        .expect("必须投影");

        match projected {
            AgentLoopEvent::ToolCallFinished {
                id,
                ok,
                duration_ms,
                started_at,
                finished_at,
                status,
                ..
            } => {
                assert_eq!(id, "c9");
                assert!(!ok, "Error 回执必须 ok=false");
                assert_eq!(duration_ms, Some(250), "耗时必须由起止时间派生");
                assert_eq!(started_at, Some(1_000));
                assert_eq!(finished_at, Some(1_250));
                assert_eq!(status.as_deref(), Some("error"));
            }
            other => panic!("投影类型不对: {other:?}"),
        }
    }

    /// 用量事件要补成 `AssistantStats`（界面遥测条依赖它），其余事件按投影走。
    #[tokio::test]
    async fn test_bridge_forwards_events_and_reports_seq_violations() {
        let (tx, mut rx) = mpsc::channel::<AgentLoopEvent>(16);
        let bridge = LoopEventBridge::new(tx);

        bridge.emit(ev(1, AgentEventBody::TextDelta { text: "a".into() }));
        bridge.emit(ev(2, AgentEventBody::UsageReported {
            usage: TokenUsage::default(),
            duration_ms: 42,
        }));
        // seq 违约：重复 2
        bridge.emit(ev(2, AgentEventBody::TextDelta { text: "b".into() }));

        assert_eq!(bridge.seq_violations().len(), 1, "重复 seq 必须被记账");

        // 三条都转发出来了（违约也转发，只是记账）
        let first = rx.recv().await.expect("第 1 条");
        assert!(matches!(first, AgentLoopEvent::TextDelta { .. }));
        let second = rx.recv().await.expect("第 2 条");
        match second {
            AgentLoopEvent::AssistantStats { duration_ms, usage, .. } => {
                assert_eq!(duration_ms, 42);
                assert!(usage.is_some(), "用量必须带过来，否则遥测条为空");
            }
            other => panic!("用量事件应补成 AssistantStats，实际 {other:?}"),
        }
        let third = rx.recv().await.expect("第 3 条");
        assert!(matches!(third, AgentLoopEvent::TextDelta { .. }));
    }

    /// W3-T4 守门：**没注入引擎 = 硬失败**，不许再悄悄降级到 legacy（那条路已经删了）。
    ///
    /// 这条钉住的是"配置错误必须可见"：如果 `run_agent_turn` 在无引擎时返回 `Ok`，
    /// 界面就会永远停在"运行中"而没有任何报错——比直接失败难查得多。
    #[tokio::test]
    async fn test_missing_engine_is_a_hard_error() {
        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        let err = run_agent_turn(
            None,
            "t_no_engine",
            Some("你好"),
            None,
            ProviderConfig {
                id: "p".into(),
                name: "p".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
            None,
            None,
            tx,
            None,
        )
        .await
        .expect_err("无引擎必须硬失败");

        let msg = err.to_string();
        assert!(
            msg.contains("legacy 主循环已在 W3-T4 删除"),
            "错误信息应说清原因与补救办法：{msg}"
        );
    }

    /// W3-T4 出口判据：**真引擎实跑一轮**，领域事件直接进入通道。
    ///
    /// （W3-T2 时期这条测试用 `run_agent_turn_with(EngineChoice::Runtime, ...)`；
    /// 开关删除后直接走唯一路径 `run_turn_with_engine`。）
    #[tokio::test]
    async fn test_runtime_path_actually_runs_the_real_engine() {
        use agent_base::model::{ProviderConfig, StreamDelta};
        use agent_base::testing::{
            FixedClock, FixedPrompt, InMemorySessionStore, MockScope, RecordingApprovalGate,
            ScriptedModelClient,
        };
        use agent_runtime::ProductBuilder;

        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "真引擎在跑。".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));
        let runtime = Arc::new(
            ProductBuilder::new(switch_spec())
                .with_tools(
                    agent_toolkit::tools_for_toolkits(&["core".to_string()], std::path::Path::new("."))
                        .expect("core 工具包可装配"),
                )
                .with_model(model)
                .with_approval(Arc::new(RecordingApprovalGate::new(true)))
                .with_store(Arc::new(InMemorySessionStore::new()))
                .with_prompt(Arc::new(FixedPrompt::new("测试人格")))
                .with_scope(Arc::new(MockScope::new("s")))
                .with_clock(Arc::new(FixedClock::new(1_700_000_000_000)))
                .build()
                .expect("装配真引擎"),
        );

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
        run_turn_with_engine(
            runtime,
            "t_switch",
            Some("你好"),
            None,
            ProviderConfig {
                id: "p".into(),
                name: "p".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
            None,
            Some(7),
            tx,
            None,
        )
        .await
        .expect("真引擎轮次必须成功");

        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(ev.body.kind().to_string());
        }
        assert!(kinds.iter().any(|k| k == "text.delta"), "脚本化模型的文本必须进入事件流：{kinds:?}");
        assert!(kinds.iter().any(|k| k == "turn.finished"), "收尾事件必须进入事件流：{kinds:?}");
    }

    /// **多模态端到端**：桥这一层收到的图片必须出现在**真正发给模型的那份 messages** 里。
    ///
    /// 这条链一共四段：`thread.send` → `spawn_thread_loop` → 本函数（`TurnRequest::with_images`）
    /// → `format_messages_for_model`。**断任何一段的表现都一样**——界面卡片上有图、
    /// 模型却看不见，而且不报错。这里从桥的出口钉死，另外三段各有自己的单测。
    #[tokio::test]
    async fn test_images_reach_the_model_request() {
        use agent_base::model::{ProviderConfig, StreamDelta};
        use agent_base::testing::{
            FixedClock, FixedPrompt, InMemorySessionStore, MockScope, RecordingApprovalGate,
            ScriptedModelClient,
        };
        use agent_runtime::ProductBuilder;

        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "看到了。".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));
        let runtime = Arc::new(
            ProductBuilder::new(switch_spec())
                .with_tools(
                    agent_toolkit::tools_for_toolkits(&["core".to_string()], std::path::Path::new("."))
                        .expect("core 工具包可装配"),
                )
                .with_model(model.clone())
                .with_approval(Arc::new(RecordingApprovalGate::new(true)))
                .with_store(Arc::new(InMemorySessionStore::new()))
                .with_prompt(Arc::new(FixedPrompt::new("测试人格")))
                .with_scope(Arc::new(MockScope::new("s")))
                .with_clock(Arc::new(FixedClock::new(1_700_000_000_000)))
                .build()
                .expect("装配真引擎"),
        );

        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        run_turn_with_engine(
            runtime,
            "t_images",
            Some("看看这张图"),
            Some(vec!["data:image/png;base64,AAA".into()]),
            ProviderConfig {
                id: "p".into(),
                name: "p".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
            None,
            Some(0),
            tx,
            None,
        )
        .await
        .expect("带图片的轮次必须成功");

        let requests = model.requests();
        assert_eq!(requests.len(), 1, "这一轮只该调用一次模型");

        let user = requests[0]
            .messages
            .iter()
            .find(|m| m.role == "user")
            .expect("必须有一条用户消息");
        assert_eq!(user.content.as_deref(), Some("看看这张图"));
        assert_eq!(
            user.images.as_deref(),
            Some(["data:image/png;base64,AAA".to_string()].as_slice()),
            "图片必须出现在发给模型的消息上"
        );

        // 反向：图片只挂在**那条**用户消息上，不许顺着历史漏到别的消息
        let carriers = requests[0].messages.iter().filter(|m| m.images.is_some()).count();
        assert_eq!(carriers, 1, "只有那条用户消息该带图");
    }

    /// W3-T3 守门：投影表必须**覆盖全部 12 种领域事件**，且每种只出现一次。
    #[test]
    fn test_projection_table_is_total_over_all_event_variants() {
        use agent_base::model::TokenUsage;

        let cases: Vec<(AgentEventBody, bool)> = vec![
            (AgentEventBody::TurnStarted, false),
            (AgentEventBody::ThinkingDelta { text: "t".into() }, true),
            (AgentEventBody::TextDelta { text: "t".into() }, true),
            (
                AgentEventBody::ToolCallStarted {
                    call_id: "c".into(),
                    name: "n".into(),
                    args: serde_json::json!({}),
                },
                true,
            ),
            (
                AgentEventBody::ToolCallFinished {
                    call_id: "c".into(),
                    name: "n".into(),
                    receipt: ToolReceipt::success("o", 1, 2),
                },
                true,
            ),
            (
                AgentEventBody::ApprovalRequested {
                    call_id: "c".into(),
                    tool: "n".into(),
                    args: serde_json::json!({ "command": "rm -rf build" }),
                },
                true,
            ),
            (
                AgentEventBody::QuestionAsked { call_id: "c".into(), question: serde_json::json!({}) },
                true,
            ),
            (
                AgentEventBody::SubagentStarted { thread_id: "s".into(), subagent: "x".into() },
                false,
            ),
            (
                AgentEventBody::SubagentFinished { thread_id: "s".into(), ok: true, summary: "s".into() },
                false,
            ),
            (
                AgentEventBody::UsageReported { usage: TokenUsage::default(), duration_ms: 1 },
                false,
            ),
            (AgentEventBody::TurnFinished { stop: TurnStopReason::Completed }, true),
            (AgentEventBody::Failed { message: "m".into() }, true),
            (
                AgentEventBody::Diagnostic(agent_base::domain::DiagnosticEvent::new(
                    agent_base::domain::DiagFamily::Prompt,
                    agent_base::domain::POINT_PROMPT_COMPOSED,
                    agent_base::domain::Origin::Engine,
                )),
                false,
            ),
        ];

        // 1. 编译期穷尽性
        for (body, _) in &cases {
            let _kind: &'static str = match body {
                AgentEventBody::TurnStarted => "turn.started",
                AgentEventBody::ThinkingDelta { .. } => "thinking.delta",
                AgentEventBody::TextDelta { .. } => "text.delta",
                AgentEventBody::ToolCallStarted { .. } => "tool.started",
                AgentEventBody::ToolCallFinished { .. } => "tool.finished",
                AgentEventBody::ApprovalRequested { .. } => "approval.requested",
                AgentEventBody::QuestionAsked { .. } => "question.asked",
                AgentEventBody::SubagentStarted { .. } => "subagent.started",
                AgentEventBody::SubagentFinished { .. } => "subagent.finished",
                AgentEventBody::UsageReported { .. } => "usage",
                AgentEventBody::TurnFinished { .. } => "turn.finished",
                AgentEventBody::Failed { .. } => "failed",
                AgentEventBody::Diagnostic(_) => "diagnostic",
            };
        }

        // 2. 完备性：13 种 kind 每种恰好一次
        let mut kinds: Vec<&str> = cases.iter().map(|(b, _)| b.kind()).collect();
        kinds.sort_unstable();
        let mut unique = kinds.clone();
        unique.dedup();
        assert_eq!(unique.len(), cases.len(), "投影表里同一事件种类出现了多次：{kinds:?}");
        assert_eq!(unique.len(), 13, "领域事件共 13 种，投影表必须全覆盖：{unique:?}");

        // 3. 结论与表一致
        for (body, expect_mapped) in &cases {
            let got = project_to_loop_event(&ev(1, body.clone()));
            if *expect_mapped {
                assert!(got.is_some(), "`{}` 必须有投影（界面需要它）", body.kind());
            } else {
                assert!(
                    got.is_none(),
                    "`{}` 声明为无对应物，却投影出了 {got:?}",
                    body.kind()
                );
            }
        }
    }

    /// W3-T3 出口判据（修 P1-14）：**审批请求真的能走到界面并等回答复**。
    ///
    /// 完整链路：引擎发 `ApprovalRequested` → 桥投影成 `AgentLoopEvent::ApprovalRequested`
    /// → 界面按 `waiting_approval` 渲染批准/拒绝按钮 → `approval.decide` →
    /// `ApprovalManager::resolve_approval` → 唤醒闸门 → 工具继续执行 → 正常收尾。
    ///
    /// 若这条链路断了（正是 P1-14 的症状），本用例会**超时失败**而不是悄悄变绿。
    #[tokio::test]
    async fn test_approval_request_reaches_ui_channel_and_unblocks_the_gate() {
        use std::time::Duration;

        use agent_base::domain::{
            Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor,
        };
        use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, MockScope, ScriptedModelClient};
        use agent_base::model::{ProviderConfig, StreamDelta, ToolCallInfo};
        use agent_proto::ApprovalMode;
        use agent_runtime::ProductBuilder;

        // 受审批约束的工具：只读 + Named 策略 → 在 Ask 档下会被问
        let gated_desc = ToolDescriptor {
            name: "gated_tool".to_string(),
            summary: "受审批约束的工具".to_string(),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Named("approval-guard"),
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };

        let store = Arc::new(tokio::sync::RwLock::new(crate::state::AgentStore::new(
            "E:/codes/gate_test_ws".to_string(),
        )));
        // 权威档位：Ask（一律问）
        store.write().await.config.approval = ApprovalMode::Ask;

        let approval_mgr = Arc::new(agent_node::approval::ApprovalManager::new());
        // S2：这里用**真实**的桥接实现（store → 端口 → 闸门），
        // 于是这条测试同时证明了"界面改档位，闸门立刻看到"这条链路。
        let node_config: Arc<dyn agent_node::node_config::NodeConfigSource> =
            Arc::new(crate::server::StoreBackedNodeConfig::new(store.clone()));
        let gate = Arc::new(
            agent_node::approval::HostApprovalGate::new(node_config, approval_mgr.clone())
                .with_timeout(Duration::from_secs(5)),
        );

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "call_gate".into(),
                        name: "gated_tool".into(),
                        // 用**真实形状**的参数（不是 `{}`）：审批事件必须把它带到界面，
                        // 否则界面上只有工具名，用户没法判断该不该批准。
                        args: r#"{"command":"rm -rf build"}"#.into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "批准后完成。".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = Arc::new(
            ProductBuilder::new(switch_spec())
                .with_tool(Arc::new(agent_base::testing::MockTool::new(
                    gated_desc,
                    ToolReceipt::success("工具已执行", 1, 2),
                )))
                .with_model(model)
                .with_approval(gate)
                .with_store(Arc::new(InMemorySessionStore::new()))
                .with_prompt(Arc::new(FixedPrompt::new("测试人格")))
                .with_scope(Arc::new(MockScope::new("s")))
                .with_clock(Arc::new(FixedClock::new(1_700_000_000_000)))
                .build()
                .expect("装配真引擎"),
        );

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);

        let handle = tokio::spawn(async move {
            run_turn_with_engine(
                runtime,
                "t_gate",
                Some("执行受约束的工具"),
                None,
                ProviderConfig {
                    id: "p".into(),
                    name: "p".into(),
                    protocol: Default::default(),
                    base_url: "http://localhost".into(),
                    api_key: "k".into(),
                    model: "m".into(),
                    max_output_tokens: None,
                    custom_headers: None,
                    proxy_url: None,
                },
                None,
                Some(5),
                tx,
                None,
            )
            .await
        });

        // 1. 审批请求必须到达通道
        let call_id = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match rx.recv().await {
                    Some(AgentEvent {
                        body: AgentEventBody::ApprovalRequested { call_id, tool, args },
                        ..
                    }) => {
                        assert_eq!(tool, "gated_tool", "审批事件必须带工具名");
                        // 🔴 只带工具名的审批请求等于没有确认闸门：用户看不到要批准什么。
                        // 这条断言就是钉住"审批卡能显示命令原文"。
                        assert_eq!(
                            args.get("command").and_then(|v| v.as_str()),
                            Some("rm -rf build"),
                            "审批事件必须带上参数原文（界面要显示待批准的命令），实际：{args}"
                        );
                        return call_id;
                    }
                    Some(_) => continue,
                    None => panic!("事件通道关闭，但从未收到审批请求（P1-14 症状）"),
                }
            }
        })
        .await
        .expect("审批请求必须到达通道——这正是 P1-14 要修的");

        // 2. 界面点"批准"（等价于 `approval.decide` 落到同一个 manager 上）
        assert!(
            approval_mgr.resolve_approval(&call_id, true),
            "批准必须命中等待中的 waiter（否则 UI 的答复送不到闸门）"
        );

        // 3. 轮次必须**继续并收尾**，而不是等满超时
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("批准后轮次必须继续（不得等满超时）")
            .expect("任务不应 panic");
        assert!(result.is_ok(), "轮次应成功：{result:?}");

        let mut saw_finished = false;
        let mut saw_tool_finish = false;
        while let Ok(ev) = rx.try_recv() {
            match ev.body {
                AgentEventBody::TurnFinished { .. } => saw_finished = true,
                AgentEventBody::ToolCallFinished { .. } => saw_tool_finish = true,
                _ => {}
            }
        }
        assert!(saw_tool_finish, "批准后工具应真的执行完");
        assert!(saw_finished, "批准后应正常收尾");
    }

    #[tokio::test]
    async fn test_direct_sink_forwards_events_and_reports_seq_violations() {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(16);
        let sink = DirectEventSink::new(tx);

        sink.emit(ev(1, AgentEventBody::TextDelta { text: "a".into() }));
        sink.emit(ev(2, AgentEventBody::TextDelta { text: "b".into() }));
        // seq 违约：重复 2
        sink.emit(ev(2, AgentEventBody::TextDelta { text: "c".into() }));

        assert_eq!(sink.seq_violations().len(), 1, "重复 seq 必须被记账");

        let first = rx.recv().await.expect("第 1 条");
        assert_eq!(first.seq, 1);
        let second = rx.recv().await.expect("第 2 条");
        assert_eq!(second.seq, 2);
        let third = rx.recv().await.expect("第 3 条");
        assert_eq!(third.seq, 2);
    }

    fn switch_spec() -> agent_runtime::AgentSpec {
        agent_runtime::AgentSpec::from_json_str(
            r#"{
            "id": "ada-engine-switch-test",
            "archetype": "coding",
            "identity": { "name": "切换测试", "persona": "system.md", "locale": "zh-CN" },
            "toolkits": ["core"],
            "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
            "policies": { "maxSteps": 4, "parallelTools": 1, "toolTimeoutSec": 30 }
        }"#,
        )
        .expect("spec 合法")
    }
}
