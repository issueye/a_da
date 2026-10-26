//! AGENT BASE 单一引擎（INV-1）：多轮流式驱动、工具执行、审批守门与收尾保证。
//!
//! 核心断言（合规契约）：
//! 1. `ApprovalGate::decide` 在每次受策略约束的调用前被调用一次；`Denied` 以工具结果回模型，不抛错；
//! 2. `ToolCallStarted` 必先于同 `CallId` 的 `ToolCallFinished`；每轮**恰好一个** `TurnFinished`；
//! 3. `CancelToken` 贯穿传进 `ModelClient::stream` 与 `Tool::execute`；
//! 4. 无工具调用即结束；无隐式步数上限，达到预算时自报 `TurnStopReason::BudgetExhausted`；
//! 5. 消费 `Termination::EndTurn`：当工具声明整轮终止时结束并发出恰好一个 `TurnFinished`；
//! 6. 零 IO、零产品名词（不依赖具体文件系统或产品词汇）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::{
    Access, AgentError, AgentEvent, AgentEventBody, AgentMessage, ApprovalPolicy, DiagFamily,
    DiagOutcome, DiagnosticEvent, FailDirection, Origin, POINT_APPROVAL_SKIPPED,
    POINT_APPROVAL_WAITED, POINT_HOOK_REPLACE_APPLIED, POINT_HOOK_REPLACE_REJECTED,
    POINT_PROMPT_COMPOSED, POINT_TURN_ABORTED, Termination, ToolCall, ToolCallBlock, ToolReceipt,
    ToolStatus, TraceCtx, TurnStopReason, make_turn_id,
};
use crate::engine::policy::RunPolicy;
use crate::engine::prompt_format::format_messages_for_model;
use crate::engine::turn::{TurnOutcome, TurnRequest};
use crate::model::{
    ChatCompletionTool, ChatCompletionToolFunction, ModelChatOptions, StreamDelta, TokenUsage,
};
use crate::ports::hooks::{
    AfterNote, AfterRequest, BeforeOutcome, BeforeRequest, HookCtx, HookObservation, HookRunner,
    points,
};
use crate::ports::{
    AnsweredBy, ApprovalGate, ApprovalOutcome, ApprovalRequest, CancelToken, Clock,
    CompletionRequest, EventSink, ModelClient, ModelError, PromptSource, Scope, SessionStore,
    ToolCatalog, ToolContext, ToolError,
};

pub struct AgentRuntime {
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<dyn ToolCatalog>,
    pub approval: Arc<dyn ApprovalGate>,
    pub store: Arc<dyn SessionStore>,
    pub prompt: Arc<dyn PromptSource>,
    pub scope: Arc<dyn Scope>,
    pub clock: Arc<dyn Clock>,
    pub policy: RunPolicy,
    seq: AtomicU64,
    /// 轮次序号（每轮唯一 turn_id 的一半，另一半是线程 id）。见 [`make_turn_id`]。
    turn_ordinal: AtomicU64,
    /// 钩子运行时（P3a · SP-51）。`None` = 这个产品没开钩子——那时**一条钩子诊断都不发**：
    /// 没开钩子的产品报"没人订阅这个点位"只会污染面板，还会掩盖真正的问题。
    hooks: Option<Arc<dyn HookRunner>>,
}

/// `AFTER_TURN` 钩子的 payload（**给插件钩子的公开契约**：字段增删都算破坏性变更）。
///
/// 为什么必须只有一处构造：`run_turn` 有两条退出路径（"无工具调用自然结束"与
/// "有工具调用后结束"），此前各自内联了一份同样的 `json!`。加一个字段时漏改一处
/// **不会有任何编译错误**，只会让钩子在部分轮次里拿不到字段——最难查的那类缺陷。
fn after_turn_payload(
    step: u32,
    thread_id: &str,
    status: &str,
    assistant_text_chars: usize,
    thinking_chars: usize,
    requested_tool_names: &[String],
    tool_results: &[serde_json::Value],
    usage: &serde_json::Value,
    llm_duration_ms: u64,
    tools_duration_ms: u64,
) -> serde_json::Value {
    serde_json::json!({
        "step": step,
        "threadId": thread_id,
        "status": status,
        "assistantTextChars": assistant_text_chars,
        "thinkingChars": thinking_chars,
        "requestedToolNames": requested_tool_names,
        "toolResults": tool_results,
        "usage": usage,
        "llmDurationMs": llm_duration_ms,
        "toolsDurationMs": tools_duration_ms,
    })
}

impl AgentRuntime {
    pub fn new(
        model: Arc<dyn ModelClient>,
        tools: Arc<dyn ToolCatalog>,
        approval: Arc<dyn ApprovalGate>,
        store: Arc<dyn SessionStore>,
        prompt: Arc<dyn PromptSource>,
        scope: Arc<dyn Scope>,
        clock: Arc<dyn Clock>,
        policy: RunPolicy,
    ) -> Self {
        Self {
            model,
            tools,
            approval,
            store,
            prompt,
            scope,
            clock,
            policy,
            seq: AtomicU64::new(1),
            turn_ordinal: AtomicU64::new(1),
            hooks: None,
        }
    }

    /// 装上钩子运行时（P3a · SP-51）。
    ///
    /// 用 builder 而不是给 `new` 加参数：`AgentRuntime::new` 的调用点绝大多数是
    /// "这个用例压根不关心钩子"的引擎用例，让它们一行不改才是对的。
    ///
    /// **预算不在这里**：单点位预算是运行时实现自己的配置（SP-53 的 `hookTimeoutMs`），
    /// 传两处就会不一致（表现是"界面显示 500ms、实际等了 5 秒"）。
    pub fn with_hooks(mut self, runner: Arc<dyn HookRunner>) -> Self {
        self.hooks = Some(runner);
        self
    }

    /// 是否装了钩子（宿主自检 / 测试断言用）。
    pub fn has_hooks(&self) -> bool {
        self.hooks.is_some()
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    fn emit(&self, sink: &dyn EventSink, thread_id: &str, trace: &TraceCtx, body: AgentEventBody) {
        let seq = self.next_seq();
        let at_ms = self.clock.now_ms();
        sink.emit(AgentEvent::with_trace(
            seq,
            at_ms,
            thread_id,
            trace.clone(),
            body,
        ));
    }

    /// 把一条钩子观测转成**成对诊断**（P3a · SP-52）。
    ///
    /// 把一条钩子观测转成成对诊断（P3a · SP-52）。
    ///
    /// 转换本身在 `ports::hooks::observation_to_diagnostic`（**唯一转换点**：宿主侧的
    /// 管理面点位用同一份），这里只负责套上本轮的事件信封（`seq` 与轮次上下文）。
    fn emit_hook_observation(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        obs: &HookObservation,
    ) {
        self.emit_diag(
            sink,
            thread_id,
            trace,
            crate::ports::hooks::observation_to_diagnostic(obs),
        );
    }
    /// 跑一个事前点位（P3a · SP-51）。
    ///
    /// 观测回调把**逐条**记录直接发成诊断——`HookEntered` 必须早于钩子实际结束，
    /// 事后补发会让"耗时"与"顺序"都变成编的。
    async fn run_before_hook(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        cancel: &dyn CancelToken,
    ) -> BeforeOutcome {
        let Some(runner) = &self.hooks else {
            return BeforeOutcome::Pass;
        };
        let on_event = |obs: HookObservation| {
            self.emit_hook_observation(sink, thread_id, trace, &obs);
        };
        let ctx = HookCtx {
            thread_id,
            turn_id: trace.turn_id.as_deref().unwrap_or(""),
            call_id: trace.call_id.as_deref(),
            cancel,
            on_event: &on_event,
        };
        runner
            .before(
                BeforeRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await
    }

    /// 跑一个事后点位并拿到旁注（P3a · SP-51）。
    #[allow(clippy::too_many_arguments)]
    async fn run_after_hook(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        cancel: &dyn CancelToken,
    ) -> Vec<AfterNote> {
        let Some(runner) = &self.hooks else {
            return Vec::new();
        };
        let on_event = |obs: HookObservation| {
            self.emit_hook_observation(sink, thread_id, trace, &obs);
        };
        let ctx = HookCtx {
            thread_id,
            turn_id: trace.turn_id.as_deref().unwrap_or(""),
            call_id: trace.call_id.as_deref(),
            cancel,
            on_event: &on_event,
        };
        runner
            .after(
                AfterRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await
    }

    async fn persist_message(
        &self,
        thread_id: &str,
        message: &AgentMessage,
        sink: &dyn EventSink,
        trace: &TraceCtx,
        cancel: &dyn CancelToken,
    ) -> Result<(), AgentError> {
        let _ = self
            .run_before_hook(
                points::BEFORE_PERSIST,
                Some(thread_id.to_string()),
                serde_json::json!({
                    "threadId": thread_id,
                    "message": serde_json::to_value(message).unwrap_or(serde_json::Value::Null),
                }),
                sink,
                thread_id,
                trace,
                cancel,
            )
            .await;
        self.store.append_message(thread_id, message).await
    }

    /// 问审批端口"这次调用需要问吗"（`Never` 短路——它与档位无关，声明就是声明）。
    ///
    /// 抽出来是为了**改写路径上的风险单调性判定**与真正执行前的那次判定
    /// 用的是**同一个**判据：两处各写一份，迟早出现"单调性说没降级、闸门却没问"。
    async fn needs_approval_for(
        &self,
        tool: &str,
        access: &Access,
        policy: &ApprovalPolicy,
        args: &serde_json::Value,
    ) -> bool {
        match policy {
            ApprovalPolicy::Never => false,
            policy => self.approval.needs_approval(tool, access, policy, args).await,
        }
    }

    /// 改写被重新校验拒了：**留痕**（这是"插件想做但没做成"的记录）。
    fn emit_replace_rejected(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        by: &str,
        reason: &str,
    ) {
        self.emit_diag(
            sink,
            thread_id,
            trace,
            DiagnosticEvent::new(
                DiagFamily::Hook,
                POINT_HOOK_REPLACE_REJECTED,
                Origin::Plugin { id: by.to_string() },
            )
            .with_outcome(DiagOutcome::Denied)
            .with_reason(reason),
        );
    }

    /// 把事后旁注**追加进模型读到的那段文本**（P3a · SP-51）。
    ///
    /// 为什么追在 `output` 里而不是另起一条消息：设计 §5.3 的口径是"复用模型已在读的
    /// 那段文本"——另起一条会让模型把它当成新的一步，而插件旁注本来只是"补充说明"。
    /// 前缀写明来源：模型必须知道这句话**不是工具自己说的**。
    fn append_hook_notes(receipt: &mut ToolReceipt, notes: &[AfterNote]) {
        let texts: Vec<&str> = notes
            .iter()
            .filter_map(|n| match n {
                AfterNote::Note { text } => Some(text.as_str()),
                AfterNote::Observe { .. } => None,
            })
            .collect();
        if texts.is_empty() {
            return;
        }
        receipt.output.push_str("\n\n[插件旁注] ");
        receipt.output.push_str(&texts.join("\n"));
    }

    /// 回执状态 → 给插件看的标签（与线上 DTO 的 snake_case 一致，别各写一套）。
    fn status_label(status: ToolStatus) -> &'static str {
        match status {
            ToolStatus::Success => "success",
            ToolStatus::Error => "error",
            ToolStatus::Denied => "denied",
            ToolStatus::Timeout => "timeout",
            ToolStatus::Aborted => "aborted",
        }
    }

    /// 发一条**诊断遥测**（P0 埋点地基）：只给人看，不进模型上下文、不进会话正文。
    ///
    /// `diag.origin` 是**产生方**（例如提示词组装是 `Engine`），与 `trace.origin`
    /// （当前正在执行谁，例如某个工具）刻意分开：一条"审批等待"是引擎测出来的，
    /// 但它测的是那个工具的等待。
    fn emit_diag(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        diag: DiagnosticEvent,
    ) {
        let seq = self.next_seq();
        let at_ms = self.clock.now_ms();
        // 显式构造变体（而不是走某个便利构造函数）：`verify-wiring` 的 check G 按
        // `AgentEventBody::<变体>` 的出现判"有没有生产发射者"。藏在便利函数里会让
        // 门禁看不见这条链路——那正是它要防的"声明了却没人发"。
        sink.emit(AgentEvent::with_trace(
            seq,
            at_ms,
            thread_id,
            trace.clone(),
            AgentEventBody::Diagnostic(diag),
        ));
    }

    /// 轮次被中止时的诊断遥测（**必须带原因**：只发一个 `Aborted` 事件、
    /// 不带"谁中止的"，界面与日志都查不出结论——这是 §1.2 记的缺口）。
    fn emit_aborted(&self, sink: &dyn EventSink, thread_id: &str, trace: &TraceCtx, source: &str) {
        self.emit_diag(
            sink,
            thread_id,
            trace,
            DiagnosticEvent::new(DiagFamily::Turn, POINT_TURN_ABORTED, Origin::Engine)
                .with_outcome(DiagOutcome::Aborted)
                .with_reason(source),
        );
    }

    /// 按 `FailDirection` 裁决**没有判定依据**的审批结果（W5-T5 / INV-4）。
    ///
    /// | `outcome.by` | 含义 | 处理 |
    /// |---|---|---|
    /// | `User` | 有人明确点了批准/拒绝 | **原样采信**（方向不得覆盖人的决定） |
    /// | `Policy` | 策略给出了确定判定 | **原样采信** |
    /// | `Timeout` | 等到超时也没人回答 | 按 `direction`：`Closed` → 拒绝，`Open` → 放行 |
    /// | `Aborted` | 等待期间会话被中止 | 同上 |
    ///
    /// 纯函数，便于直接断言（无需构造完整运行时）。
    pub(crate) fn apply_fail_direction(
        outcome: ApprovalOutcome,
        direction: FailDirection,
    ) -> ApprovalOutcome {
        match outcome.by {
            AnsweredBy::User | AnsweredBy::Policy => outcome,
            AnsweredBy::Timeout | AnsweredBy::Aborted => match direction {
                FailDirection::Closed => {
                    if outcome.approved {
                        // 安全网：闸门声明 Closed 却对"没答案"给了放行 → 引擎翻成拒绝
                        ApprovalOutcome::denied(
                            outcome.by,
                            outcome.reason.clone().unwrap_or_else(|| {
                                "无判定依据，按失败方向 Closed 拒绝".to_string()
                            }),
                        )
                    } else {
                        outcome
                    }
                }
                FailDirection::Open => {
                    if outcome.approved {
                        outcome
                    } else {
                        ApprovalOutcome::allowed(outcome.by)
                    }
                }
            },
        }
    }

    /// 运行一轮 Agent 多轮决策与执行循环。
    pub async fn run_turn(
        &self,
        req: TurnRequest,
        sink: &dyn EventSink,
        cancel: &dyn CancelToken,
    ) -> Result<TurnOutcome, AgentError> {
        let turn_start_ms = self.clock.now_ms();
        let thread_id = req.thread_id.clone();

        // 0. 埋点关联上下文（P0）：每轮唯一的 `turn_id` + 来源 `Engine`。
        //
        // 刻意**不存进 `self`**：运行时是共享的，不同会话的轮次会并发，
        // 把轮次状态放进结构体就等于引入共享全局态（INV-8 的反面）。
        let turn_id = make_turn_id(&thread_id, self.turn_ordinal.fetch_add(1, Ordering::SeqCst));
        let mut trace = TraceCtx::turn(turn_id, Origin::Engine);

        // 1. 发送 TurnStarted
        self.emit(sink, &thread_id, &trace, AgentEventBody::TurnStarted);

        match self
            .run_before_hook(
                points::BEFORE_AGENT_START,
                None,
                serde_json::json!({
                    "threadId": thread_id,
                    "hasUserPrompt": req.user_prompt.as_ref().is_some_and(|p| !p.trim().is_empty()),
                    "kind": "main",
                }),
                sink,
                &thread_id,
                &trace,
                cancel,
            )
            .await
        {
            BeforeOutcome::Pass | BeforeOutcome::Replace { .. } => {}
            BeforeOutcome::Deny { reason } => {
                let stop = TurnStopReason::Denied;
                let _ = self
                    .run_after_hook(
                        points::AFTER_AGENT_END,
                        None,
                        serde_json::json!({ "threadId": thread_id, "reason": reason }),
                        sink,
                        &thread_id,
                        &trace,
                        cancel,
                    )
                    .await;
                self.emit(
                    sink,
                    &thread_id,
                    &trace,
                    AgentEventBody::TurnFinished { stop: stop.clone() },
                );
                return Ok(TurnOutcome {
                    thread_id,
                    stop_reason: stop,
                    steps_taken: 0,
                    total_duration_ms: (self.clock.now_ms() - turn_start_ms).max(0) as u64,
                });
            }
            BeforeOutcome::AbortTurn { reason } => {
                let stop = TurnStopReason::Aborted;
                self.emit_aborted(sink, &thread_id, &trace, "hook:beforeAgentStart");
                let _ = self
                    .run_after_hook(
                        points::AFTER_AGENT_END,
                        None,
                        serde_json::json!({ "threadId": thread_id, "reason": reason }),
                        sink,
                        &thread_id,
                        &trace,
                        cancel,
                    )
                    .await;
                self.emit(
                    sink,
                    &thread_id,
                    &trace,
                    AgentEventBody::TurnFinished { stop: stop.clone() },
                );
                return Ok(TurnOutcome {
                    thread_id,
                    stop_reason: stop,
                    steps_taken: 0,
                    total_duration_ms: (self.clock.now_ms() - turn_start_ms).max(0) as u64,
                });
            }
        }

        // 2. 检查启动前是否已取消
        if cancel.is_cancelled() {
            self.emit_aborted(sink, &thread_id, &trace, "cancel_before_start");
            self.emit(
                sink,
                &thread_id,
                &trace,
                AgentEventBody::TurnFinished {
                    stop: TurnStopReason::Aborted,
                },
            );
            return Ok(TurnOutcome {
                thread_id,
                stop_reason: TurnStopReason::Aborted,
                steps_taken: 0,
                total_duration_ms: (self.clock.now_ms() - turn_start_ms).max(0) as u64,
            });
        }

        // 3. 用户输入记录并落库
        if let Some(ref prompt_text) = req.user_prompt {
            if !prompt_text.trim().is_empty() {
                let user_msg = AgentMessage::User {
                    content: prompt_text.clone(),
                    images: req.images.clone(),
                    timestamp: Some(self.clock.now_ms()),
                };
                self.persist_message(&thread_id, &user_msg, sink, &trace, cancel)
                    .await?;
            }
        }

        // 4. 加载会话历史与系统提示词
        let mut history = self.store.load_messages(&thread_id).await?;
        let system_prompt = self.prompt.system_prompt();
        let _ = self
            .run_before_hook(
                points::BEFORE_SYSTEM_PROMPT,
                None,
                serde_json::json!({
                    "threadId": thread_id,
                    "bytes": system_prompt.len(),
                }),
                sink,
                &thread_id,
                &trace,
                cancel,
            )
            .await;
        let _ = self
            .run_after_hook(
                points::AFTER_SYSTEM_PROMPT,
                None,
                serde_json::json!({
                    "threadId": thread_id,
                    "bytes": system_prompt.len(),
                }),
                sink,
                &thread_id,
                &trace,
                cancel,
            )
            .await;
        // 提示词组装的埋点（P2·SP-43）：除了总字节，还**按片段**报"这段是谁加的、多少字节"。
        //
        // 只有总长度时，"这轮多了 200 字节"是没法排查的——不知道是技能正文变长了、
        // 还是某个插件加了一段话。`prompt_segments()` 为空表示这一份不细分（例如测试替身），
        // 此时只报总字节，**不编造一个假的拆分**。
        let segments = self.prompt.prompt_segments();
        self.emit_diag(
            sink,
            &thread_id,
            &trace,
            DiagnosticEvent::new(DiagFamily::Prompt, POINT_PROMPT_COMPOSED, Origin::Engine)
                .with_outcome(DiagOutcome::Applied)
                .with_bytes(system_prompt.len() as u64)
                .with_data(serde_json::json!({
                    "totalBytes": system_prompt.len(),
                    "segments": segments
                        .iter()
                        .map(|s| serde_json::json!({
                            "id": s.id,
                            "source": s.source,
                            "bytes": s.bytes,
                        }))
                        .collect::<Vec<_>>(),
                })),
        );

        // 获取工具描述符并转为模型 ChatCompletionTool
        let tool_descriptors = self.tools.descriptors();
        let model_tools: Vec<ChatCompletionTool> = tool_descriptors
            .iter()
            .map(|d| ChatCompletionTool {
                tool_type: "function".to_string(),
                function: ChatCompletionToolFunction {
                    name: d.name.clone(),
                    description: d.summary.clone(),
                    parameters: d.schema.clone(),
                },
            })
            .collect();

        let mut step_index = 0u32;
        let mut final_stop_reason: TurnStopReason;
        // 上一轮模型**如实报回来**的用量：上下文治理拿它当"下一轮 prompt 会有多大"的依据
        let mut last_round_usage: Option<TokenUsage> = None;

        // 5. 多轮循环驱动
        'turn_loop: loop {
            // 步数预算（INV: 设了预算必须自报，无隐式步数上限）。
            //
            // ⚠️ 顺序很重要：**先判预算、后自增**。
            // 原实现是"先自增再判 `step_index > max_steps`"，于是被预算**拒绝的那次尝试**
            // 也被算进了 `steps_taken`——`maxSteps=1` 会报 `steps_taken=2`，而实际只跑了 1 步。
            // 这个 off-by-one 由 W3-T5 的 golden 夹具 `budget_exhausted` 首次照出
            // （既有单测只断言了 `stop_reason`，没断言步数，所以一直没被发现）。
            if let Some(max_steps) = self.policy.max_steps {
                if step_index >= max_steps {
                    final_stop_reason = TurnStopReason::BudgetExhausted {
                        limit_steps: max_steps,
                    };
                    break 'turn_loop;
                }
            }

            // 上下文治理：上一轮真实用量已经逼近窗口上限 → **提前失败**并如实自报。
            //
            // 不这么做会发生什么（实测）：445k tokens 的请求被网关在流式过程中掐断，
            // 报告只写了一半，界面看到的却是笼统的"模型调用失败"，只能靠翻会话文件才查得出。
            // 判定放在模型调用**之前**，且只看"上一轮如实报回来的数字"，不猜、不估算。
            if let (Some(window), Some(prev)) = (req.context_window, last_round_usage.as_ref()) {
                if window > 0 {
                    let projected = prev.prompt_tokens.saturating_add(prev.completion_tokens);
                    // 留 5% 余量：下一轮的 prompt 还要加上这一轮的工具输出
                    let limit = window.saturating_mul(95) / 100;
                    if projected >= limit {
                        // 只**如实收尾**，不额外发 `Failed`：撞顶由 `TurnStopReason` 承载，
                        // 桥接层会把它投影成专门的事件交给调度层做"自动压缩后续跑"。
                        // （早先发一条笼统的 Failed，界面只看到"模型调用失败"，查不出真因。）
                        final_stop_reason = TurnStopReason::ContextWindowExceeded {
                            prompt_tokens: projected,
                            window,
                        };
                        break 'turn_loop;
                    }
                }
            }

            step_index += 1;
            // 埋点步号与循环步号同步（一次模型调用 = 一步）
            trace.step_index = step_index;

            // 检查取消
            if cancel.is_cancelled() {
                self.emit_aborted(sink, &thread_id, &trace, "cancel_before_step");
                final_stop_reason = TurnStopReason::Aborted;
                break 'turn_loop;
            }

            // beforeTurn is called after per-turn budget/cancellation checks and
            // immediately before constructing the model request.
            let before_turn_outcome = self
                .run_before_hook(
                    points::BEFORE_TURN,
                    None,
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "messageCount": history.len(),
                        "systemPromptBytes": system_prompt.len(),
                        "availableTools": model_tools.iter().map(|t| t.function.name.as_str()).collect::<Vec<_>>(),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
            match before_turn_outcome {
                BeforeOutcome::Pass => {}
                BeforeOutcome::Deny { reason: _ } => {
                    final_stop_reason = TurnStopReason::Denied;
                    break 'turn_loop;
                }
                BeforeOutcome::AbortTurn { reason } => {
                    self.emit_aborted(
                        sink,
                        &thread_id,
                        &trace,
                        &format!("hook:{}:{reason}", points::BEFORE_TURN),
                    );
                    final_stop_reason = TurnStopReason::Aborted;
                    break 'turn_loop;
                }
                BeforeOutcome::Replace { .. } => {}
            }

            let step_start_ms = self.clock.now_ms();
            let mut step_usage: Option<TokenUsage> = None;
            let mut accumulated_usage = TokenUsage::default();

            let _ = self
                .run_before_hook(
                    points::BEFORE_CONTEXT,
                    None,
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "messageCount": history.len(),
                        "systemPromptBytes": system_prompt.len(),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
            let chat_messages = format_messages_for_model(&system_prompt, &history);
            let _ = self
                .run_after_hook(
                    points::AFTER_CONTEXT,
                    None,
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "messageCount": chat_messages.len(),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
            let mut options = req.options.clone().unwrap_or_else(|| ModelChatOptions {
                tools: None,
                system_prompt: None,
                temperature: Some(0.2),
                effort: None,
                max_retries: Some(crate::model::DEFAULT_MODEL_RETRY_COUNT),
            });
            // 重试次数由组合根按用户设置注入；**只覆盖这一项**，不碰温度等其它默认值
            if let Some(retries) = req.max_retries {
                options.max_retries = Some(retries);
            }
            if !model_tools.is_empty() {
                options.tools = Some(model_tools.clone());
            }

            let completion_req = CompletionRequest {
                config: req.provider_config.clone(),
                messages: chat_messages,
                options,
            };

            // 调用模型流式
            for point in [
                points::BEFORE_PROVIDER_REQUEST,
                points::BEFORE_PROVIDER_HEADERS,
            ] {
                match self
                    .run_before_hook(
                        point,
                        Some(req.provider_config.model.clone()),
                        serde_json::json!({
                            "step": step_index,
                            "threadId": thread_id,
                            "model": req.provider_config.model.clone(),
                            "messageCount": completion_req.messages.len(),
                        }),
                        sink,
                        &thread_id,
                        &trace,
                        cancel,
                    )
                    .await
                {
                    BeforeOutcome::Pass | BeforeOutcome::Replace { .. } => {}
                    BeforeOutcome::Deny { reason } => {
                        self.emit(
                            sink,
                            &thread_id,
                            &trace,
                            AgentEventBody::Failed { message: reason },
                        );
                        final_stop_reason = TurnStopReason::Denied;
                        break 'turn_loop;
                    }
                    BeforeOutcome::AbortTurn { reason } => {
                        self.emit_aborted(
                            sink,
                            &thread_id,
                            &trace,
                            &format!("hook:{point}:{reason}"),
                        );
                        final_stop_reason = TurnStopReason::Aborted;
                        break 'turn_loop;
                    }
                }
            }

            match self
                .run_before_hook(
                    points::BEFORE_LLM_REQUEST,
                    Some(req.provider_config.model.clone()),
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "model": req.provider_config.model.clone(),
                        "messageCount": completion_req.messages.len(),
                        "toolCount": completion_req.options.tools.as_ref().map(|t| t.len()).unwrap_or(0),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await
            {
                BeforeOutcome::Pass | BeforeOutcome::Replace { .. } => {}
                BeforeOutcome::Deny { reason } => {
                    self.emit(sink, &thread_id, &trace, AgentEventBody::Failed { message: reason });
                    final_stop_reason = TurnStopReason::Denied;
                    break 'turn_loop;
                }
                BeforeOutcome::AbortTurn { reason } => {
                    self.emit_aborted(sink, &thread_id, &trace, &format!("hook:{}:{reason}", points::BEFORE_LLM_REQUEST));
                    final_stop_reason = TurnStopReason::Aborted;
                    break 'turn_loop;
                }
            }

            let stream_res = self.model.stream(completion_req, Some(cancel)).await;
            let mut stream_rx = match stream_res {
                Ok(rx) => rx,
                Err(err) => {
                    self.emit(
                        sink,
                        &thread_id,
                        &trace,
                        AgentEventBody::Failed {
                            message: format!("{:?}", err),
                        },
                    );
                    final_stop_reason = if matches!(err, ModelError::Cancelled) {
                        TurnStopReason::Aborted
                    } else {
                        TurnStopReason::ModelError
                    };
                    break 'turn_loop;
                }
            };

            let mut accumulated_text = String::new();
            let mut accumulated_thinking = String::new();
            let mut tool_calls = Vec::new();
            let mut stream_stop_reason = "stop".to_string();
            let mut stream_error: Option<String> = None;

            while let Some(delta) = stream_rx.recv().await {
                if cancel.is_cancelled() {
                    final_stop_reason = TurnStopReason::Aborted;
                    break 'turn_loop;
                }

                match delta {
                    StreamDelta::Thinking { thinking } => {
                        accumulated_thinking.push_str(&thinking);
                        self.emit(
                            sink,
                            &thread_id,
                            &trace,
                            AgentEventBody::ThinkingDelta { text: thinking },
                        );
                    }
                    StreamDelta::Text { text } => {
                        accumulated_text.push_str(&text);
                        self.emit(sink, &thread_id, &trace, AgentEventBody::TextDelta { text });
                    }
                    StreamDelta::ToolCall { call } => {
                        tool_calls.push(call);
                    }
                    StreamDelta::Usage { usage } => {
                        accumulated_usage.prompt_tokens += usage.prompt_tokens;
                        accumulated_usage.completion_tokens += usage.completion_tokens;
                        accumulated_usage.total_tokens += usage.total_tokens;
                        if let Some(thinking) = usage.thinking_tokens {
                            accumulated_usage.thinking_tokens =
                                Some(accumulated_usage.thinking_tokens.unwrap_or(0) + thinking);
                        }
                        if usage.cached_tokens.is_some() {
                            accumulated_usage.cached_tokens = usage.cached_tokens;
                        }
                        step_usage = Some(usage);
                    }
                    StreamDelta::Done { stop_reason } => {
                        stream_stop_reason = stop_reason;
                    }
                    StreamDelta::Error { error } => {
                        stream_error = Some(error.clone());
                        self.emit(
                            sink,
                            &thread_id,
                            &trace,
                            AgentEventBody::Failed { message: error },
                        );
                        break;
                    }
                }
            }

            let _ = self
                .run_after_hook(
                    points::AFTER_LLM_RESPONSE,
                    Some(req.provider_config.model.clone()),
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "status": if stream_error.is_some() { "error" } else { "completed" },
                        "textChars": accumulated_text.chars().count(),
                        "thinkingChars": accumulated_thinking.chars().count(),
                        "toolCalls": tool_calls.len(),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
            let _ = self
                .run_after_hook(
                    points::AFTER_PROVIDER_RESPONSE,
                    Some(req.provider_config.model.clone()),
                    serde_json::json!({
                        "step": step_index,
                        "threadId": thread_id,
                        "status": if stream_error.is_some() { "error" } else { "completed" },
                        "textChars": accumulated_text.chars().count(),
                        "toolCalls": tool_calls.len(),
                    }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;

            if let Some(err) = stream_error {
                // 流到一半断了（网关掉线 / 超时 / 传输错误）：模型**已经产出的正文不能丢**。
                //
                // 早先这里直接 break——半截报告既不落盘、也不进会话，界面上只能看到上一轮的
                // 某句话，看着就像"莫名其妙地失败"（实测：子智能体把 400 行调研报告写到一半，
                // 流断了，会话里只剩它上一轮的一句"Let me capture the BI plan docs…"）。
                // 落盘失败也不能吞掉原始错误：如实上报失败原因优先。
                if !accumulated_text.trim().is_empty() || !accumulated_thinking.trim().is_empty() {
                    let now = self.clock.now_ms();
                    let partial = AgentMessage::Assistant {
                        content: accumulated_text.clone(),
                        thinking: if accumulated_thinking.is_empty() {
                            None
                        } else {
                            Some(accumulated_thinking.clone())
                        },
                        tool_calls: None,
                        stop_reason: Some("error".to_string()),
                        error_message: Some(err.clone()),
                        timestamp: Some(now),
                        usage: serde_json::to_value(&accumulated_usage).ok(),
                        duration_ms: Some((now - step_start_ms).max(0) as u64),
                        turn_duration_ms: Some((now - turn_start_ms).max(0) as u64),
                    };
                    let _ = self
                        .persist_message(&thread_id, &partial, sink, &trace, cancel)
                        .await;
                }
                final_stop_reason = TurnStopReason::ModelError;
                break 'turn_loop;
            }

            if stream_stop_reason == "aborted" || cancel.is_cancelled() {
                final_stop_reason = TurnStopReason::Aborted;
                break 'turn_loop;
            }

            let step_duration_ms = (self.clock.now_ms() - step_start_ms).max(0) as u64;
            let turn_duration_ms = (self.clock.now_ms() - turn_start_ms).max(0) as u64;

            let final_step_usage = step_usage.unwrap_or(accumulated_usage);
            // 记下来给下一轮的上下文治理用（如实数字，不做估算）
            last_round_usage = Some(final_step_usage.clone());
            let assistant_text_chars = accumulated_text.chars().count();
            let thinking_chars = accumulated_thinking.chars().count();
            let requested_tool_names: Vec<String> =
                tool_calls.iter().map(|c| c.name.clone()).collect();
            let usage_summary =
                serde_json::to_value(&final_step_usage).unwrap_or(serde_json::Value::Null);

            // 保存 Assistant 消息
            let tool_call_blocks: Vec<ToolCallBlock> = tool_calls
                .iter()
                .map(|c| ToolCallBlock {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: serde_json::from_str(&c.args).unwrap_or(serde_json::Value::Null),
                    raw_arguments: c.args.clone(),
                })
                .collect();

            let assistant_msg = AgentMessage::Assistant {
                content: accumulated_text,
                thinking: if accumulated_thinking.is_empty() {
                    None
                } else {
                    Some(accumulated_thinking)
                },
                tool_calls: if tool_call_blocks.is_empty() {
                    None
                } else {
                    Some(tool_call_blocks)
                },
                stop_reason: Some(stream_stop_reason.clone()),
                error_message: None,
                timestamp: Some(self.clock.now_ms()),
                usage: serde_json::to_value(&final_step_usage).ok(),
                duration_ms: Some(step_duration_ms),
                turn_duration_ms: Some(turn_duration_ms),
            };

            self.persist_message(&thread_id, &assistant_msg, sink, &trace, cancel)
                .await?;
            history.push(assistant_msg);
            let mut tool_results_summary: Vec<serde_json::Value> = Vec::new();
            let mut tools_duration_ms = 0u64;

            // 发送用量与统计事件
            self.emit(
                sink,
                &thread_id,
                &trace,
                AgentEventBody::UsageReported {
                    usage: final_step_usage,
                    duration_ms: step_duration_ms,
                },
            );

            // 如果无工具调用，本轮自然结束
            if tool_calls.is_empty() {
                self.run_after_hook(
                    points::AFTER_TURN,
                    None,
                    after_turn_payload(
                        step_index,
                        &thread_id,
                        "completed",
                        assistant_text_chars,
                        thinking_chars,
                        &requested_tool_names,
                        &tool_results_summary,
                        &usage_summary,
                        step_duration_ms,
                        tools_duration_ms,
                    ),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
                final_stop_reason = TurnStopReason::Completed;
                break 'turn_loop;
            }

            // 执行工具调用
            let mut should_terminate_turn = false;

            for raw_call in &tool_calls {
                if cancel.is_cancelled() {
                    final_stop_reason = TurnStopReason::Aborted;
                    break 'turn_loop;
                }

                let mut parsed_args =
                    serde_json::from_str(&raw_call.args).unwrap_or(serde_json::Value::Null);

                let tool_res = self.tools.resolve(&raw_call.name);
                match tool_res {
                    Err(ToolError::Unknown(name)) => {
                        let now = self.clock.now_ms();
                        let receipt = ToolReceipt::error(format!("未知的工具: {name}"), now, now);
                        let result_msg =
                            Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                        self.persist_message(&thread_id, &result_msg, sink, &trace, cancel)
                            .await?;
                        history.push(result_msg);
                    }
                    Err(ToolError::Unsupported(msg)) | Err(ToolError::Failed(msg)) => {
                        let now = self.clock.now_ms();
                        let receipt = ToolReceipt::error(msg, now, now);
                        let result_msg =
                            Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                        self.persist_message(&thread_id, &result_msg, sink, &trace, cancel)
                            .await?;
                        history.push(result_msg);
                    }
                    Ok(tool) => {
                        let desc = tool.descriptor();
                        let is_write = !desc.is_readonly();

                        // 埋点归因（P0）：这次调用属于哪个 call、来自谁。
                        // 内置工具按工具名标注；P1 起插件工具会带 `plugin:<id>`。
                        trace.call_id = Some(raw_call.id.clone());
                        trace.origin = Origin::Builtin {
                            name: raw_call.name.clone(),
                        };

                        let mut is_denied = false;
                        let mut denial_reason = None;
                        // 风险判定的**预计算结果**：改写路径上算过一次就复用，
                        // 免得同一次调用问端口两遍（两遍之间端口若给了不同答案，
                        // "风险单调性"这条安全性质就没法证明了）。
                        let mut approval_prechecked: Option<bool> = None;

                        // ── beforeToolCall（P3a·SP-51 拦下 / P3b·SP-60 改写）─────
                        //
                        // 位置是**安全序**（设计 §5.6）：在审批判定**之前**。
                        // 改写之后**必须**重跑 schema / 路径 / 风险单调性——改写不是授权，
                        // 它只是"换个参数再走同一套闸门"。
                        let before_outcome = self
                            .run_before_hook(
                                points::BEFORE_TOOL_CALL,
                                Some(raw_call.name.clone()),
                                serde_json::json!({
                                    "tool": raw_call.name,
                                    "args": parsed_args.clone(),
                                    "isWrite": is_write,
                                }),
                                sink,
                                &thread_id,
                                &trace,
                                cancel,
                            )
                            .await;
                        match before_outcome {
                            BeforeOutcome::Pass => {}
                            BeforeOutcome::Deny { reason } => {
                                is_denied = true;
                                denial_reason = Some(reason);
                            }
                            BeforeOutcome::AbortTurn { reason } => {
                                // AbortTurn 比 Deny 强：结束**整轮**（只有内置插件拿得到，
                                // 分级已在装配期挡掉第三方，见 `ports::hooks::allows`）
                                self.emit_aborted(
                                    sink,
                                    &thread_id,
                                    &trace,
                                    &format!("hook:{}:{reason}", points::BEFORE_TOOL_CALL),
                                );
                                final_stop_reason = TurnStopReason::Aborted;
                                trace.call_id = None;
                                trace.origin = Origin::Engine;
                                break 'turn_loop;
                            }
                            BeforeOutcome::Replace { args: new_args, by } => {
                                // ① 形状 / schema / 路径沙箱（纯函数，见 `hook_guard`）
                                let structural = crate::engine::validate_args_against_schema(
                                    &new_args,
                                    &desc.schema,
                                )
                                .and_then(|()| {
                                    crate::engine::check_paths(
                                        self.scope.as_ref(),
                                        &desc.access,
                                        &new_args,
                                    )
                                });

                                match structural {
                                    Err(rej) => {
                                        self.emit_replace_rejected(
                                            sink,
                                            &thread_id,
                                            &trace,
                                            &by,
                                            &rej.label(),
                                        );
                                    }
                                    Ok(()) => {
                                        // ② 风险单调性：改写**不得降低**风险等级。
                                        //
                                        // 判据就是审批端口对同一 (工具, 声明, 参数) 的判定：
                                        // 改之前要问、改之后不用问 = 降级 → 拒。理由很硬：
                                        // 否则插件可以把危险命令改写成无害形式骗过审批，
                                        // 而**实际执行的是改写后的 args**（事后点位没有副作用能力）。
                                        let before_risk = self
                                            .needs_approval_for(
                                                &raw_call.name,
                                                &desc.access,
                                                &desc.approval,
                                                &parsed_args,
                                            )
                                            .await;
                                        let after_risk = self
                                            .needs_approval_for(
                                                &raw_call.name,
                                                &desc.access,
                                                &desc.approval,
                                                &new_args,
                                            )
                                            .await;
                                        if before_risk && !after_risk {
                                            self.emit_replace_rejected(
                                                sink,
                                                &thread_id,
                                                &trace,
                                                &by,
                                                "risk_downgrade:改写不得把需要审批的调用变成免审批",
                                            );
                                        } else {
                                            // ③ 生效：之后**一切**（审批、执行、回执）都用新参数
                                            let before_hash =
                                                crate::ports::hooks::content_hash8(&parsed_args);
                                            let after_hash =
                                                crate::ports::hooks::content_hash8(&new_args);
                                            parsed_args = new_args;
                                            approval_prechecked = Some(after_risk);
                                            self.emit_diag(
                                                sink,
                                                &thread_id,
                                                &trace,
                                                DiagnosticEvent::new(
                                                    DiagFamily::Hook,
                                                    POINT_HOOK_REPLACE_APPLIED,
                                                    Origin::Plugin { id: by.clone() },
                                                )
                                                .with_outcome(DiagOutcome::Passed)
                                                .with_data(serde_json::json!({
                                                    "tool": raw_call.name,
                                                    "beforeHash": before_hash,
                                                    "afterHash": after_hash,
                                                })),
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // 审批检查（M1-T4 + W1-T6）：
                        // **策略归端口**——引擎不解释档位/危险命令/白名单/Named 策略，
                        // 只把工具名、声明与参数递过去（AGENTS.md §14）。
                        // `Never` 是唯一的例外：它连问都不必问，引擎直接短路。
                        //
                        // 🔴 读的是**改写后**的 `parsed_args`（安全序的要求）。
                        let needs_approval = match approval_prechecked {
                            Some(v) => v,
                            None => {
                                self.needs_approval_for(
                                    &raw_call.name,
                                    &desc.access,
                                    &desc.approval,
                                    &parsed_args,
                                )
                                .await
                            }
                        };

                        // P1b（SP-36）：**没问就放行**的审计留痕。
                        //
                        // 只记"声明侧要求审批（非 `Never`）而最终没问"的那一类——被档位
                        // （`auto` / `full`）或免问白名单放行的调用。`read_file` 这种本来就
                        // 免问的工具每次都记，只会把 60 条环形缓冲刷满、把真信号淹掉。
                        //
                        // 刻意**不记档位**：按 AGENTS §14 引擎不解释档位（策略归端口），
                        // 档位在界面的选择器上本来就看得见。
                        if !needs_approval && !matches!(desc.approval, ApprovalPolicy::Never) {
                            self.emit_diag(
                                sink,
                                &thread_id,
                                &trace,
                                DiagnosticEvent::new(
                                    DiagFamily::Approval,
                                    POINT_APPROVAL_SKIPPED,
                                    Origin::Engine,
                                )
                                .with_outcome(DiagOutcome::Passed)
                                .with_data(serde_json::json!({
                                    "tool": raw_call.name,
                                    "isWrite": is_write,
                                })),
                            );
                        }

                        let mut approval_hook_notes = Vec::new();
                        if needs_approval {
                            match self
                                .run_before_hook(
                                    points::BEFORE_APPROVAL,
                                    Some(raw_call.name.clone()),
                                    serde_json::json!({
                                        "callId": raw_call.id,
                                        "tool": raw_call.name,
                                        "args": parsed_args,
                                        "isWrite": is_write,
                                    }),
                                    sink,
                                    &thread_id,
                                    &trace,
                                    cancel,
                                )
                                .await
                            {
                                BeforeOutcome::Pass | BeforeOutcome::Replace { .. } => {}
                                BeforeOutcome::Deny { reason } => {
                                    is_denied = true;
                                    denial_reason = Some(format!("approval hook denied: {reason}"));
                                    approval_hook_notes = self
                                        .run_after_hook(
                                            points::AFTER_APPROVAL,
                                            Some(raw_call.name.clone()),
                                            serde_json::json!({
                                                "tool": raw_call.name,
                                                "approved": false,
                                                "by": "hook",
                                                "reason": reason,
                                            }),
                                            sink,
                                            &thread_id,
                                            &trace,
                                            cancel,
                                        )
                                        .await;
                                }
                                BeforeOutcome::AbortTurn { reason } => {
                                    self.emit_aborted(
                                        sink,
                                        &thread_id,
                                        &trace,
                                        &format!("hook:{}:{reason}", points::BEFORE_APPROVAL),
                                    );
                                    final_stop_reason = TurnStopReason::Aborted;
                                    break 'turn_loop;
                                }
                            }

                            if !is_denied {
                                // 审批事件**带上参数原文**：界面要能把"要批准的命令"完整摆出来，
                                // 否则用户只能对着一个工具名点"批准"（等于没有确认闸门）。
                                self.emit(
                                    sink,
                                    &thread_id,
                                    &trace,
                                    AgentEventBody::ApprovalRequested {
                                        call_id: raw_call.id.clone(),
                                        tool: raw_call.name.clone(),
                                        args: parsed_args.clone(),
                                    },
                                );

                                let app_req = ApprovalRequest {
                                    call_id: raw_call.id.clone(),
                                    thread_id: thread_id.clone(),
                                    tool: raw_call.name.clone(),
                                    args: parsed_args.clone(),
                                    is_write,
                                    reason: None,
                                };

                                let approval_started = self.clock.now_ms();
                                let outcome = self.approval.decide(app_req, Some(cancel)).await;
                                let waited_ms =
                                    (self.clock.now_ms() - approval_started).max(0) as u64;
                                // W5-T5 / INV-4：**无判定依据**时由端口声明的方向裁决。
                                //
                                // `User` / `Policy` 是**确定的答案**（有人点了、或策略判了），
                                // 方向不得覆盖它——否则"用户明确拒绝"会被 fail-open 翻成放行。
                                // `Timeout` / `Aborted` 是**没拿到答案**，正是方向该起作用的地方。
                                //
                                // 引擎是唯一裁决点：这样"声明了 `Open` 的闸门"才不会与自己的声明打架
                                // （此前每个闸门各自硬编码超时结果，`direction()` 形同虚设）。
                                let outcome =
                                    Self::apply_fail_direction(outcome, self.approval.direction());
                                // 审批等待的埋点（§1.2 记的缺口：等多久、谁给的答案、结果如何，
                                // 此前完全没有输出——只有前端自己打的四段耗时）。
                                let by = match outcome.by {
                                    AnsweredBy::User => "user",
                                    AnsweredBy::Policy => "policy",
                                    AnsweredBy::Timeout => "timeout",
                                    AnsweredBy::Aborted => "aborted",
                                };
                                self.emit_diag(
                                sink,
                                &thread_id,
                                &trace,
                                DiagnosticEvent::new(
                                    DiagFamily::Approval,
                                    POINT_APPROVAL_WAITED,
                                    Origin::Engine,
                                )
                                .with_duration_ms(waited_ms)
                                .with_outcome(if outcome.approved {
                                    DiagOutcome::Passed
                                } else {
                                    DiagOutcome::Denied
                                })
                                .with_data(serde_json::json!({ "by": by, "approved": outcome.approved })),
                            );
                                if !outcome.approved {
                                    is_denied = true;
                                    denial_reason =
                                        outcome.reason.or_else(|| Some("审批未通过".to_string()));
                                }
                                approval_hook_notes = self
                                    .run_after_hook(
                                        points::AFTER_APPROVAL,
                                        Some(raw_call.name.clone()),
                                        serde_json::json!({
                                            "tool": raw_call.name,
                                            "approved": outcome.approved,
                                            "by": by,
                                            "waitedMs": waited_ms,
                                        }),
                                        sink,
                                        &thread_id,
                                        &trace,
                                        cancel,
                                    )
                                    .await;
                            }
                        }

                        if is_denied {
                            // 合规断言：Denied 以工具结果回模型，不抛错
                            let now = self.clock.now_ms();
                            let mut receipt = ToolReceipt::denied(
                                denial_reason.unwrap_or_else(|| "操作被拒绝".to_string()),
                                now,
                                now,
                            );
                            tools_duration_ms =
                                tools_duration_ms.saturating_add(receipt.duration_ms());
                            tool_results_summary.push(serde_json::json!({
                                "tool": raw_call.name,
                                "status": "denied",
                                "outputChars": receipt.output.chars().count(),
                            }));
                            Self::append_hook_notes(&mut receipt, &approval_hook_notes);

                            // afterToolCall 对**被拒的调用**同样触发（P3a·SP-51）：
                            // 拒绝也产生一条模型会读到的结果，插件（例如一个审计插件）
                            // 最需要看见的恰是这一类。事后没有否决权，只追加旁注。
                            let notes = self
                                .run_after_hook(
                                    points::AFTER_TOOL_CALL,
                                    Some(raw_call.name.clone()),
                                    serde_json::json!({
                                        "tool": raw_call.name,
                                        "status": "denied",
                                        "reason": receipt.output,
                                    }),
                                    sink,
                                    &thread_id,
                                    &trace,
                                    cancel,
                                )
                                .await;
                            Self::append_hook_notes(&mut receipt, &notes);

                            self.emit(
                                sink,
                                &thread_id,
                                &trace,
                                AgentEventBody::ToolCallFinished {
                                    call_id: raw_call.id.clone(),
                                    name: raw_call.name.clone(),
                                    receipt: receipt.clone(),
                                },
                            );

                            let result_msg = Self::receipt_to_agent_message(
                                &raw_call.id,
                                &raw_call.name,
                                &receipt,
                            );
                            self.persist_message(&thread_id, &result_msg, sink, &trace, cancel)
                                .await?;
                            history.push(result_msg);

                            // 归因复位：这一手的 call_id 不再适用于后续事件
                            trace.call_id = None;
                            trace.origin = Origin::Engine;
                        } else {
                            // 审批通过，执行工具。
                            // 事件里报的参数与真正执行的是**同一份**（改写后的那份）：
                            // 界面看到的、模型之后读到的、工具收到的三者必须一致。
                            self.emit(
                                sink,
                                &thread_id,
                                &trace,
                                AgentEventBody::ToolCallStarted {
                                    call_id: raw_call.id.clone(),
                                    name: raw_call.name.clone(),
                                    args: parsed_args.clone(),
                                },
                            );

                            // P3b·SP-63：**工具内部点位**的入口。出口仍由引擎给
                            // （观测 → 诊断信封要 `seq` 与轮次上下文，只有这里拼得出来），
                            // 所以工具只报事实，"事件流单一真源"没有被破坏。
                            let hook_outlet = |obs: HookObservation| {
                                self.emit_hook_observation(sink, &thread_id, &trace, &obs);
                            };
                            let tool_hooks =
                                self.hooks
                                    .as_ref()
                                    .map(|runner| crate::ports::hooks::ToolHooks {
                                        runner: runner.as_ref(),
                                        turn_id: trace.turn_id.as_deref().unwrap_or(""),
                                        call_id: trace.call_id.as_deref(),
                                        on_event: &hook_outlet,
                                    });

                            let ctx = ToolContext {
                                scope: self.scope.as_ref(),
                                cancel,
                                events: sink,
                                thread_id: &thread_id,
                                hooks: tool_hooks,
                            };

                            let exec_started = self.clock.now_ms();
                            // 🔴 策略超时（`RunPolicy::tool_timeout`，产品声明里是 `toolTimeoutSec`）。
                            //
                            // 没有这一段，一个卡住的工具会把整轮**永远挂住**：界面上是
                            // "工具卡片卡着 → 智能体停止"，用户完全看不出发生了什么。
                            // 超时不杀工具内部的东西（那是各工具自己的事），而是**引擎不再等**
                            // ——如实回一条 `Timeout` 回执，让模型换路子，整轮继续。
                            //
                            // `long_running()` 的工具（等用户/派活/命令执行）不受它约束：
                            // 拿 120 秒去掐"等审批"和"跑长命令"只会制造假失败。
                            // 🔴 执行用的参数**在这一点**才组装：`parsed_args` 可能已被
                            // `beforeToolCall` 的改写换掉（P3b）。早一点绑定就会变成
                            // "事件里报的是新参数、真正执行的是旧参数"——两本账，最难查。
                            let tool_call = ToolCall {
                                id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                args: parsed_args.clone(),
                            };

                            let mut receipt = match self.policy.tool_timeout {
                                Some(limit) if !tool.long_running() => {
                                    match tokio::time::timeout(
                                        limit,
                                        tool.execute(&tool_call, &ctx),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            let now = self.clock.now_ms();
                                            ToolReceipt::timeout(
                                                format!(
                                                    "工具 `{}` 超过 {} 秒没有返回，已停止等待。\
                                                     不要原样重试同一调用：缩小范围、改成分步/后台执行，\
                                                     或换一个工具。",
                                                    raw_call.name,
                                                    limit.as_secs()
                                                ),
                                                exec_started,
                                                now,
                                            )
                                        }
                                    }
                                }
                                _ => tool.execute(&tool_call, &ctx).await,
                            };

                            // ── afterToolCall（P3a · SP-51）─────────────────────
                            //
                            // 事后只做**观察 + 追加旁注**：旁注追进 `output`（模型正在读的那段
                            // 文本，前缀写明来自插件），观察只进诊断面。**不得**改写
                            // `status`——"工具到底成没成"是已经发生的事实（设计 §5.3）。
                            tools_duration_ms =
                                tools_duration_ms.saturating_add(receipt.duration_ms());
                            tool_results_summary.push(serde_json::json!({
                                "tool": raw_call.name,
                                "status": Self::status_label(receipt.status),
                                "outputChars": receipt.output.chars().count(),
                            }));
                            let notes = self
                                .run_after_hook(
                                    points::AFTER_TOOL_CALL,
                                    Some(raw_call.name.clone()),
                                    serde_json::json!({
                                        "tool": raw_call.name,
                                        "status": Self::status_label(receipt.status),
                                        "output": receipt.output,
                                        "durationMs": receipt.duration_ms(),
                                    }),
                                    sink,
                                    &thread_id,
                                    &trace,
                                    cancel,
                                )
                                .await;
                            Self::append_hook_notes(&mut receipt, &notes);

                            self.emit(
                                sink,
                                &thread_id,
                                &trace,
                                AgentEventBody::ToolCallFinished {
                                    call_id: raw_call.id.clone(),
                                    name: raw_call.name.clone(),
                                    receipt: receipt.clone(),
                                },
                            );

                            let result_msg = Self::receipt_to_agent_message(
                                &raw_call.id,
                                &raw_call.name,
                                &receipt,
                            );
                            self.persist_message(&thread_id, &result_msg, sink, &trace, cancel)
                                .await?;
                            history.push(result_msg);

                            // 归因复位（同拒绝分支）
                            trace.call_id = None;
                            trace.origin = Origin::Engine;

                            // 消费 Termination（M1-T6）
                            if desc.termination == Termination::EndTurn {
                                should_terminate_turn = true;
                            }
                        }
                    }
                }
            }

            self.run_after_hook(
                points::AFTER_TURN,
                None,
                after_turn_payload(
                    step_index,
                    &thread_id,
                    "completed",
                    assistant_text_chars,
                    thinking_chars,
                    &requested_tool_names,
                    &tool_results_summary,
                    &usage_summary,
                    step_duration_ms,
                    tools_duration_ms,
                ),
                sink,
                &thread_id,
                &trace,
                cancel,
            )
            .await;

            if should_terminate_turn {
                final_stop_reason = TurnStopReason::Completed;
                break 'turn_loop;
            }
        }

        // 6. 收尾保证：每轮恰好发出一个 TurnFinished！
        let _ = self
            .run_after_hook(
                points::AFTER_AGENT_END,
                None,
                serde_json::json!({
                    "threadId": thread_id,
                    "reason": format!("{:?}", final_stop_reason),
                    "steps": step_index,
                }),
                sink,
                &thread_id,
                &trace,
                cancel,
            )
            .await;
        self.emit(
            sink,
            &thread_id,
            &trace,
            AgentEventBody::TurnFinished {
                stop: final_stop_reason.clone(),
            },
        );

        let total_duration_ms = (self.clock.now_ms() - turn_start_ms).max(0) as u64;

        Ok(TurnOutcome {
            thread_id,
            stop_reason: final_stop_reason,
            steps_taken: step_index,
            total_duration_ms,
        })
    }

    fn receipt_to_agent_message(
        call_id: &str,
        tool_name: &str,
        receipt: &ToolReceipt,
    ) -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: receipt.output.clone(),
            is_error: Some(receipt.status != ToolStatus::Success),
            details: receipt.details.clone(),
            patch: None,
            checkpoint_id: None,
            timestamp: Some(receipt.finished_at),
            status: Some(format!("{:?}", receipt.status).to_lowercase()),
            duration_ms: Some(receipt.duration_ms()),
            started_at: Some(receipt.started_at),
            finished_at: Some(receipt.finished_at),
        }
    }
}

#[cfg(test)]
mod tests;
