//! AGENT BASE 单一引擎（INV-1）：多轮流式驱动、工具执行、审批守门与收尾保证。
//!
//! 核心断言（合规契约）：
//! 1. `ApprovalGate::decide` 在每次受策略约束的调用前被调用一次；`Denied` 以工具结果回模型，不抛错；
//! 2. `ToolCallStarted` 必先于同 `CallId` 的 `ToolCallFinished`；每轮**恰好一个** `TurnFinished`；
//! 3. `CancelToken` 贯穿传进 `ModelClient::stream` 与 `Tool::execute`；
//! 4. 无工具调用即结束；无隐式步数上限，达到预算时自报 `TurnStopReason::BudgetExhausted`；
//! 5. 消费 `Termination::EndTurn`：当工具声明整轮终止时结束并发出恰好一个 `TurnFinished`；
//! 6. 零 IO、零产品名词（不依赖具体文件系统或产品词汇）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::domain::{
    make_turn_id, AgentEvent, AgentEventBody, AgentError, AgentMessage, ApprovalPolicy,
    DiagFamily, DiagOutcome, DiagnosticEvent, FailDirection, Origin, Termination, ToolCall,
    ToolCallBlock, ToolReceipt, ToolStatus, TraceCtx, TurnStopReason, POINT_APPROVAL_SKIPPED,
    POINT_APPROVAL_WAITED,
    POINT_HOOK_REPLACE_APPLIED, POINT_HOOK_REPLACE_REJECTED,
    POINT_PROMPT_COMPOSED, POINT_TURN_ABORTED,
};
use crate::engine::policy::RunPolicy;
use crate::engine::prompt_format::format_messages_for_model;
use crate::engine::turn::{TurnOutcome, TurnRequest};
use crate::model::{
    ChatCompletionTool, ChatCompletionToolFunction, ModelChatOptions,
    StreamDelta, TokenUsage,
};
use crate::ports::hooks::{
    points, AfterNote, AfterRequest, BeforeOutcome, BeforeRequest, HookCtx,
    HookObservation, HookRunner,
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
        sink.emit(AgentEvent::with_trace(seq, at_ms, thread_id, trace.clone(), body));
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

    /// 问审批端口"这次调用需要问吗"（`Never` 短路——它与档位无关，声明就是声明）。
    ///
    /// 抽出来是为了**改写路径上的风险单调性判定**与真正执行前的那次判定
    /// 用的是**同一个**判据：两处各写一份，迟早出现"单调性说没降级、闸门却没问"。
    async fn needs_approval_for(
        &self,
        tool: &str,
        policy: &ApprovalPolicy,
        args: &serde_json::Value,
    ) -> bool {
        match policy {
            ApprovalPolicy::Never => false,
            policy => self.approval.needs_approval(tool, policy, args).await,
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
    fn emit_aborted(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        trace: &TraceCtx,
        source: &str,
    ) {
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

        // 2. 检查启动前是否已取消
        if cancel.is_cancelled() {
            self.emit_aborted(sink, &thread_id, &trace, "cancel_before_start");
            self.emit(sink, &thread_id, &trace, AgentEventBody::TurnFinished { stop: TurnStopReason::Aborted });
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
                self.store.append_message(&thread_id, &user_msg).await?;
            }
        }

        // 4. 加载会话历史与系统提示词
        let mut history = self.store.load_messages(&thread_id).await?;
        let system_prompt = self.prompt.system_prompt();
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
                    final_stop_reason = TurnStopReason::BudgetExhausted { limit_steps: max_steps };
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

            // beforeTurn 钩子 (P3a): 点位已开放，支持 Deny/AbortTurn。
            let before_turn_outcome = self
                .run_before_hook(
                    points::BEFORE_TURN,
                    None,
                    serde_json::json!({ "step": step_index }),
                    sink,
                    &thread_id,
                    &trace,
                    cancel,
                )
                .await;
            match before_turn_outcome {
                BeforeOutcome::Pass => {}
                BeforeOutcome::Deny { reason } => {
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

                break 'turn_loop;
            }

            let step_start_ms = self.clock.now_ms();
            let mut step_usage: Option<TokenUsage> = None;
            let mut accumulated_usage = TokenUsage::default();

            let chat_messages = format_messages_for_model(&system_prompt, &history);
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
            let stream_res = self.model.stream(completion_req, Some(cancel)).await;
            let mut stream_rx = match stream_res {
                Ok(rx) => rx,
                Err(err) => {
                    self.emit(sink, &thread_id, &trace, AgentEventBody::Failed { message: format!("{:?}", err) });
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
                        self.emit(sink, &thread_id, &trace, AgentEventBody::ThinkingDelta { text: thinking });
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
                        self.emit(sink, &thread_id, &trace, AgentEventBody::Failed { message: error });
                        break;
                    }
                }
            }

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
                    let _ = self.store.append_message(&thread_id, &partial).await;
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
                thinking: if accumulated_thinking.is_empty() { None } else { Some(accumulated_thinking) },
                tool_calls: if tool_call_blocks.is_empty() { None } else { Some(tool_call_blocks) },
                stop_reason: Some(stream_stop_reason.clone()),
                error_message: None,
                timestamp: Some(self.clock.now_ms()),
                usage: serde_json::to_value(&final_step_usage).ok(),
                duration_ms: Some(step_duration_ms),
                turn_duration_ms: Some(turn_duration_ms),
            };

            self.store.append_message(&thread_id, &assistant_msg).await?;
            history.push(assistant_msg);

            // 发送用量与统计事件
            self.emit(sink, &thread_id, &trace, AgentEventBody::UsageReported {
                usage: final_step_usage,
                duration_ms: step_duration_ms,
            });

            // 如果无工具调用，本轮自然结束
            if tool_calls.is_empty() {
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

                let mut parsed_args = serde_json::from_str(&raw_call.args).unwrap_or(serde_json::Value::Null);

                let tool_res = self.tools.resolve(&raw_call.name);
                match tool_res {
                    Err(ToolError::Unknown(name)) => {
                        let now = self.clock.now_ms();
                        let receipt = ToolReceipt::error(format!("未知的工具: {name}"), now, now);
                        let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                        self.store.append_message(&thread_id, &result_msg).await?;
                        history.push(result_msg);
                    }
                    Err(ToolError::Unsupported(msg)) | Err(ToolError::Failed(msg)) => {
                        let now = self.clock.now_ms();
                        let receipt = ToolReceipt::error(msg, now, now);
                        let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                        self.store.append_message(&thread_id, &result_msg).await?;
                        history.push(result_msg);
                    }
                    Ok(tool) => {
                        let desc = tool.descriptor();
                        let is_write = !desc.is_readonly();

                        // 埋点归因（P0）：这次调用属于哪个 call、来自谁。
                        // 内置工具按工具名标注；P1 起插件工具会带 `plugin:<id>`。
                        trace.call_id = Some(raw_call.id.clone());
                        trace.origin = Origin::Builtin { name: raw_call.name.clone() };

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
                                            sink, &thread_id, &trace, &by, &rej.label(),
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
                                            .needs_approval_for(&raw_call.name, &desc.approval, &parsed_args)
                                            .await;
                                        let after_risk = self
                                            .needs_approval_for(&raw_call.name, &desc.approval, &new_args)
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
                                self.needs_approval_for(&raw_call.name, &desc.approval, &parsed_args)
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

                        if needs_approval {
                            // 审批事件**带上参数原文**：界面要能把"要批准的命令"完整摆出来，
                            // 否则用户只能对着一个工具名点"批准"（等于没有确认闸门）。
                            self.emit(sink, &thread_id, &trace, AgentEventBody::ApprovalRequested {
                                call_id: raw_call.id.clone(),
                                tool: raw_call.name.clone(),
                                args: parsed_args.clone(),
                            });

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
                            let waited_ms = (self.clock.now_ms() - approval_started).max(0) as u64;
                            // W5-T5 / INV-4：**无判定依据**时由端口声明的方向裁决。
                            //
                            // `User` / `Policy` 是**确定的答案**（有人点了、或策略判了），
                            // 方向不得覆盖它——否则"用户明确拒绝"会被 fail-open 翻成放行。
                            // `Timeout` / `Aborted` 是**没拿到答案**，正是方向该起作用的地方。
                            //
                            // 引擎是唯一裁决点：这样"声明了 `Open` 的闸门"才不会与自己的声明打架
                            // （此前每个闸门各自硬编码超时结果，`direction()` 形同虚设）。
                            let outcome = Self::apply_fail_direction(
                                outcome,
                                self.approval.direction(),
                            );
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
                                denial_reason = outcome.reason.or_else(|| Some("审批未通过".to_string()));
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

                            self.emit(sink, &thread_id, &trace, AgentEventBody::ToolCallFinished {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                receipt: receipt.clone(),
                            });

                            let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                            self.store.append_message(&thread_id, &result_msg).await?;
                            history.push(result_msg);

                            // 归因复位：这一手的 call_id 不再适用于后续事件
                            trace.call_id = None;
                            trace.origin = Origin::Engine;
                        } else {
                            // 审批通过，执行工具。
                            // 事件里报的参数与真正执行的是**同一份**（改写后的那份）：
                            // 界面看到的、模型之后读到的、工具收到的三者必须一致。
                            self.emit(sink, &thread_id, &trace, AgentEventBody::ToolCallStarted {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                args: parsed_args.clone(),
                            });

                            // P3b·SP-63：**工具内部点位**的入口。出口仍由引擎给
                            // （观测 → 诊断信封要 `seq` 与轮次上下文，只有这里拼得出来），
                            // 所以工具只报事实，"事件流单一真源"没有被破坏。
                            let hook_outlet = |obs: HookObservation| {
                                self.emit_hook_observation(sink, &thread_id, &trace, &obs);
                            };
                            let tool_hooks = self.hooks.as_ref().map(|runner| {
                                crate::ports::hooks::ToolHooks {
                                    runner: runner.as_ref(),
                                    turn_id: trace.turn_id.as_deref().unwrap_or(""),
                                    call_id: trace.call_id.as_deref(),
                                    on_event: &hook_outlet,
                                }
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
                                    match tokio::time::timeout(limit, tool.execute(&tool_call, &ctx)).await {
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

                            self.emit(sink, &thread_id, &trace, AgentEventBody::ToolCallFinished {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                receipt: receipt.clone(),
                            });

                            let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                            self.store.append_message(&thread_id, &result_msg).await?;
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
                serde_json::json!({ "step": step_index }),
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
        self.emit(sink, &thread_id, &trace, AgentEventBody::TurnFinished { stop: final_stop_reason.clone() });

        let total_duration_ms = (self.clock.now_ms() - turn_start_ms).max(0) as u64;

        Ok(TurnOutcome {
            thread_id,
            stop_reason: final_stop_reason,
            steps_taken: step_index,
            total_duration_ms,
        })
    }

    fn receipt_to_agent_message(call_id: &str, tool_name: &str, receipt: &ToolReceipt) -> AgentMessage {
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
mod tests {
    use super::*;
    use crate::domain::{
        Access, Execution, PathSelector, RollbackPolicy, ToolCall, ToolDescriptor, POINT_HOOK_ENTERED,
        POINT_HOOK_FINISHED, POINT_HOOK_SKIPPED, POINT_TURN_ABORTED,
    };
    use crate::ports::{BoxFuture, Tool, ToolContext};
    use crate::model::{ProviderConfig, ToolCallInfo};
    use crate::testing::*;

    fn mock_provider_config() -> ProviderConfig {
        ProviderConfig {
            id: "test".to_string(),
            name: "test".to_string(),
            protocol: Default::default(),
            base_url: "https://api.test.com".to_string(),
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        }
    }

    fn make_test_descriptor(name: &str, approval: ApprovalPolicy, termination: Termination) -> ToolDescriptor {
        ToolDescriptor {
            name: name.to_string(),
            summary: format!("test tool {name}"),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "arg": { "type": "string" } }
            }),
            access: Access::ReadOnly,
            approval,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination,
        }
    }

    // ── W5-T5：`FailDirection` 真的参与裁决（INV-4）──────────────────────────

    /// 确定答案（`User` / `Policy`）**不受方向影响**——否则"用户明确拒绝"会被 fail-open 翻成放行。
    #[test]
    fn test_direction_never_overrides_a_definite_answer() {
        for direction in [FailDirection::Closed, FailDirection::Open] {
            let user_denied = ApprovalOutcome::denied(AnsweredBy::User, "用户点了拒绝");
            assert!(
                !AgentRuntime::apply_fail_direction(user_denied, direction).approved,
                "用户明确拒绝不得被 {direction:?} 覆盖"
            );

            let policy_allowed = ApprovalOutcome::allowed(AnsweredBy::Policy);
            assert!(
                AgentRuntime::apply_fail_direction(policy_allowed, direction).approved,
                "策略明确放行不得被 {direction:?} 覆盖"
            );
        }
    }

    /// 无判定依据（`Timeout` / `Aborted`）**由方向裁决**。
    #[test]
    fn test_direction_arbitrates_when_there_is_no_answer() {
        // Closed（默认，安全方向）→ 拒绝
        let denied = AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
            FailDirection::Closed,
        );
        assert!(!denied.approved);
        assert_eq!(denied.by, AnsweredBy::Timeout, "来路必须保留，事后要能分辨");
        assert!(denied.reason.as_deref().unwrap_or("").contains("超时"));

        // Open（显式配置的 fail-open）→ 放行：**这是 direction() 存在的意义**
        let allowed = AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
            FailDirection::Open,
        );
        assert!(allowed.approved, "声明 Open 的闸门在超时时必须放行");

        // Aborted 同口径
        assert!(!AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Aborted, "会话中止"),
            FailDirection::Closed
        )
        .approved);
        assert!(AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Aborted, "会话中止"),
            FailDirection::Open
        )
        .approved);
    }

    /// **安全网**：闸门声明 `Closed` 却对"没答案"给了放行 → 引擎必须翻成拒绝。
    ///
    /// 这条是"声明与行为打架时以声明为准"的兜底——否则一个写错的闸门会静默 fail-open。
    #[test]
    fn test_closed_direction_is_a_safety_net_over_a_wrong_gate() {
        let wrong = ApprovalOutcome::allowed(AnsweredBy::Timeout);
        let fixed = AgentRuntime::apply_fail_direction(wrong, FailDirection::Closed);
        assert!(!fixed.approved, "声明 Closed 时不得放行无判定依据的调用");
        assert_eq!(fixed.by, AnsweredBy::Timeout);
        assert!(
            fixed.reason.as_deref().unwrap_or("").contains("Closed"),
            "理由应说明是方向裁决：{:?}",
            fixed.reason
        );
    }

    /// 端到端：声明 `Open` 的闸门在超时时，工具**真的被执行**（不只是纯函数层面对）。
    #[tokio::test]
    async fn test_open_direction_lets_the_tool_run_after_timeout() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));

        let tool = Arc::new(MockTool::new(
            make_test_descriptor("gated_tool", ApprovalPolicy::Always, Termination::ContinueTurn),
            ToolReceipt::success("工具真的跑了", 0, 1),
        ));
        let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "gated_tool".into(),
                        args: "{}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        // 闸门：等待超时（没人回答），但声明 fail-open
        let gate = Arc::new(
            RecordingApprovalGate::with_outcomes(vec![ApprovalOutcome::denied(
                AnsweredBy::Timeout,
                "等待超时",
            )])
            .with_direction(FailDirection::Open),
        );

        let rt = AgentRuntime::new(
            model,
            catalog,
            gate,
            store,
            prompt,
            scope,
            clock,
            RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
        );

        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_open", mock_provider_config()).with_user_prompt("go");
        let outcome = rt
            .run_turn(req, &sink, &NeverCancel)
            .await
            .expect("轮次应正常结束");

        assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
        let executed = sink.snapshot().iter().any(|e| {
            matches!(
                &e.body,
                AgentEventBody::ToolCallFinished { receipt, .. }
                    if receipt.status == ToolStatus::Success
            )
        });
        assert!(
            executed,
            "声明 Open 时超时必须放行并真的执行工具：{:?}",
            sink.snapshot()
        );
    }

    /// 端到端对照：同场景下声明 `Closed`（默认）→ 工具**不被执行**，以拒绝回执回模型。
    #[tokio::test]
    async fn test_closed_direction_denies_after_timeout() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));

        let tool = Arc::new(MockTool::new(
            make_test_descriptor("gated_tool", ApprovalPolicy::Always, Termination::ContinueTurn),
            ToolReceipt::success("工具不该跑", 0, 1),
        ));
        let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "gated_tool".into(),
                        args: "{}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let gate = Arc::new(RecordingApprovalGate::with_outcomes(vec![
            ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
        ]));
        assert_eq!(
            ApprovalGate::direction(gate.as_ref()),
            FailDirection::Closed,
            "默认方向必须是 Closed"
        );

        let rt = AgentRuntime::new(
            model,
            catalog,
            gate,
            store,
            prompt,
            scope,
            clock,
            RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
        );

        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_closed", mock_provider_config()).with_user_prompt("go");
        rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");

        let receipts: Vec<ToolStatus> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.status.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(receipts.len(), 1, "应恰好一个工具回执：{receipts:?}");
        assert_eq!(
            receipts[0],
            ToolStatus::Denied,
            "默认 Closed 时超时必须拒绝，而不是执行工具"
        );
    }

    // ── P3a 钩子（SP-51 / SP-52）：引擎侧的行为与成对埋点 ────────────────────

    /// 造一个"模型调一次工具、然后收尾"的运行时（钩子由调用方装）。
    fn runtime_with_one_tool(
        runner: Option<Arc<dyn HookRunner>>,
        tool_receipt: ToolReceipt,
    ) -> (AgentRuntime, Arc<InMemoryToolCatalog>) {
        let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![Arc::new(MockTool::new(
            make_test_descriptor("run_command", ApprovalPolicy::Never, Termination::ContinueTurn),
            tool_receipt,
        ))]));
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "run_command".into(),
                        args: r#"{"command":"rm -rf /"}"#.into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));
        let mut rt = AgentRuntime::new(
            model,
            catalog.clone(),
            Arc::new(RecordingApprovalGate::new(true)),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(FixedPrompt::new("system")),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
        );
        if let Some(r) = runner {
            rt = rt.with_hooks(r);
        }
        (rt, catalog)
    }

    /// 从回执里取出工具结果的**正文**（模型读到的就是它）。
    fn tool_output_from_store(store: &InMemorySessionStore, thread_id: &str) -> String {
        store
            .get(thread_id)
            .into_iter()
            .filter_map(|m| match m {
                AgentMessage::ToolResult { content, .. } => Some(content),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// SP-51 的核心：插件在 `beforeToolCall` 拦下 → **理由以普通工具结果回模型**，
    /// 且工具**确实没有执行**。
    ///
    /// 这条守的是"可纠正"：模型要读到"为什么被拦"，而不是一句"执行失败"。
    #[tokio::test]
    async fn test_before_tool_call_hook_denies_with_a_normal_tool_result() {
        let runner = Arc::new(ScriptedHookRunner::new().deny_at(
            "beforeToolCall",
            "这条命令会删掉整个磁盘，请改用更精确的路径",
        ));
        let (rt, catalog) = runtime_with_one_tool(Some(runner.clone()), ToolReceipt::success("真的跑了", 0, 5));
        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_hook_deny", mock_provider_config()).with_user_prompt("go");
        rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");

        // ① 工具没被执行（回执不是那件工具自己产出的）
        let receipts: Vec<ToolReceipt> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(receipts.len(), 1, "{receipts:?}");
        assert_eq!(receipts[0].status, ToolStatus::Denied, "必须是'被拒'而不是'执行失败'");
        assert!(
            !receipts[0].output.contains("真的跑了"),
            "工具绝不能被执行：{}",
            receipts[0].output
        );
        assert!(
            receipts[0].output.contains("会删掉整个磁盘"),
            "插件给的理由必须原样回给模型（否则模型无从纠正）：{}",
            receipts[0].output
        );

        // ② 也没有 `ToolCallStarted`（没执行就没有开始）
        assert!(
            !sink.snapshot().iter().any(|e| matches!(&e.body, AgentEventBody::ToolCallStarted { .. })),
            "被拦下的调用不该发 ToolCallStarted"
        );
        let _ = catalog;
    }

    /// SP-51：`AbortTurn` 结束**整轮**（比拦下一次更强），且工具没执行。
    #[tokio::test]
    async fn test_before_tool_call_hook_can_abort_the_whole_turn() {
        let runner = Arc::new(
            ScriptedHookRunner::new().abort_at("beforeToolCall", "检测到高危操作，本轮已终止"),
        );
        let (rt, _) = runtime_with_one_tool(Some(runner), ToolReceipt::success("不该跑", 0, 5));
        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_hook_abort", mock_provider_config()).with_user_prompt("go");
        let outcome = rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");

        assert_eq!(outcome.stop_reason, TurnStopReason::Aborted, "整轮应当被中止");
        assert!(
            !sink.snapshot().iter().any(|e| matches!(&e.body, AgentEventBody::ToolCallStarted { .. })),
            "中止之后不该执行工具"
        );
        // 中止原因要如实带上（"谁中止的"是 §1.2 记的缺口）
        let aborted = sink
            .snapshot()
            .into_iter()
            .find_map(|e| match e.body {
                AgentEventBody::Diagnostic(d) if d.point == POINT_TURN_ABORTED => Some(d),
                _ => None,
            })
            .expect("要有 turn.aborted 诊断");
        assert!(
            aborted
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("beforeToolCall"),
            "中止理由要指出是哪个点位：{aborted:?}"
        );
    }

    /// SP-51：`afterToolCall` 的旁注**追进模型正在读的那段文本**，且**不改状态**。
    #[tokio::test]
    async fn test_after_tool_call_note_is_appended_without_changing_status() {
        let store = Arc::new(InMemorySessionStore::new());
        let runner = Arc::new(ScriptedHookRunner::new().note_at("afterToolCall", "记得跑一遍测试"));
        let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![Arc::new(MockTool::new(
            make_test_descriptor("run_command", ApprovalPolicy::Never, Termination::ContinueTurn),
            ToolReceipt::success("命令输出", 0, 5),
        ))]));
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "run_command".into(),
                        args: "{}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));
        let rt = AgentRuntime::new(
            model,
            catalog,
            Arc::new(RecordingApprovalGate::new(true)),
            store.clone(),
            Arc::new(FixedPrompt::new("system")),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
        )
        .with_hooks(runner);

        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_hook_note", mock_provider_config()).with_user_prompt("go");
        rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");

        let text = tool_output_from_store(&store, "t_hook_note");
        assert!(text.contains("命令输出"), "原输出要留着：{text}");
        assert!(
            text.contains("[插件旁注]") && text.contains("记得跑一遍测试"),
            "旁注要追进模型读到的文本，并标明来源：{text}"
        );
        let receipts: Vec<ToolStatus> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.status.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(receipts, vec![ToolStatus::Success], "事后**不得**改状态");
    }

    /// SP-52：**成对性契约**——每条 `hook.entered` 恰好一个 `hook.finished`，
    /// 且配对键一致；没人订阅时报 `hook.skipped`。
    #[tokio::test]
    async fn test_hook_diagnostics_are_paired_and_absence_is_reported() {
        let runner = Arc::new(ScriptedHookRunner::new().note_at("afterToolCall", "旁注"));
        let (rt, _) = runtime_with_one_tool(Some(runner), ToolReceipt::success("ok", 0, 5));
        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_hook_pairs", mock_provider_config()).with_user_prompt("go");
        rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");

        let points: Vec<(String, Option<String>)> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::Diagnostic(d)
                    if d.point.starts_with("hook.") =>
                {
                    Some((
                        d.point.clone(),
                        d.data.get("callId").and_then(|v| v.as_str()).map(String::from),
                    ))
                }
                _ => None,
            })
            .collect();

        let entered: Vec<&(String, Option<String>)> =
            points.iter().filter(|(p, _)| p == POINT_HOOK_ENTERED).collect();
        let finished: Vec<&(String, Option<String>)> =
            points.iter().filter(|(p, _)| p == POINT_HOOK_FINISHED).collect();
        assert!(!entered.is_empty(), "钩子被调用过就必须有 entered：{points:?}");
        assert_eq!(
            entered.len(),
            finished.len(),
            "成对性是硬约束（每个 entered 恰好一个 finished）：{points:?}"
        );
        for e in &entered {
            assert!(
                finished.iter().any(|f| f.1 == e.1),
                "配对键必须一致：entered={:?} finished={:?}",
                e.1,
                finished.iter().map(|f| f.1.clone()).collect::<Vec<_>>()
            );
        }

        // 没订阅的点位（这里：afterToolCall 有订阅、beforeApproval 没被问过）
        // —— 用"没人订阅"的替身再跑一遍，断言 skipped 如实出现
        let quiet = Arc::new(ScriptedHookRunner::new().unsubscribed());
        let (rt2, _) = runtime_with_one_tool(Some(quiet), ToolReceipt::success("ok", 0, 5));
        let sink2 = RecordingSink::new();
        let req2 = TurnRequest::new("t_hook_none", mock_provider_config()).with_user_prompt("go");
        rt2.run_turn(req2, &sink2, &NeverCancel).await.expect("轮次应正常结束");
        let skipped: Vec<_> = sink2
            .snapshot()
            .into_iter()
            .filter_map(|e| match e.body {
                AgentEventBody::Diagnostic(d) if d.point == POINT_HOOK_SKIPPED => Some(d),
                _ => None,
            })
            .collect();
        assert!(!skipped.is_empty(), "没人订阅也要报（缺席可见）");
        assert_eq!(skipped[0].reason.as_deref(), Some("no_subscriber"));
        assert_eq!(
            skipped[0].data.get("point").and_then(|v| v.as_str()),
            Some("beforeToolCall"),
            "要指出是哪个点位没人管：{:?}",
            skipped[0]
        );
    }

    /// 没装钩子的产品**一条钩子诊断都不发**——否则面板会被"没人订阅"刷满，
    /// 真问题反而被淹掉（这与"缺席可见"不矛盾：没开这个能力的产品，点位压根不问）。
    #[tokio::test]
    async fn test_no_hook_diagnostics_when_hooks_are_not_installed() {
        let (rt, _) = runtime_with_one_tool(None, ToolReceipt::success("ok", 0, 5));
        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_no_hooks", mock_provider_config()).with_user_prompt("go");
        rt.run_turn(req, &sink, &NeverCancel).await.expect("轮次应正常结束");
        assert!(
            !sink.snapshot().iter().any(|e| matches!(
                &e.body,
                AgentEventBody::Diagnostic(d) if d.point.starts_with("hook.")
            )),
            "没装钩子就不该有点位诊断：{:?}",
            sink.snapshot()
        );
    }

    // ── P3b：改写（SP-60）与改写后的重校验（SP-61/62）────────────────────

    /// 记录"工具到底是被哪份参数调用的"。
    struct CapturingTool {
        descriptor: ToolDescriptor,
        seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    }

    impl Tool for CapturingTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute<'a>(
            &'a self,
            call: &'a ToolCall,
            _ctx: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            if let Ok(mut v) = self.seen.lock() {
                v.push(call.args.clone());
            }
            Box::pin(async { ToolReceipt::success("跑完了", 0, 5) })
        }
    }

    /// 按**参数内容**判定的假闸门：命令里出现 `rm -rf` 才要问。
    ///
    /// 用它才能表达"风险单调性"要验的那件事——同一个工具的**不同参数**风险不同，
    /// 而改写不得把风险降下去。
    #[derive(Default)]
    struct DangerScanGate {
        asked: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl ApprovalGate for DangerScanGate {
        fn needs_approval<'a>(
            &'a self,
            _tool: &'a str,
            _policy: &'a ApprovalPolicy,
            args: &'a serde_json::Value,
        ) -> BoxFuture<'a, bool> {
            Box::pin(async move { args.to_string().contains("rm -rf") })
        }
        fn decide<'a>(
            &'a self,
            req: ApprovalRequest,
            _cancel: Option<&'a dyn CancelToken>,
        ) -> BoxFuture<'a, ApprovalOutcome> {
            if let Ok(mut v) = self.asked.lock() {
                v.push(req.args.clone());
            }
            Box::pin(async { ApprovalOutcome::allowed(AnsweredBy::Policy) })
        }
        fn direction(&self) -> FailDirection {
            FailDirection::Closed
        }
    }

    /// 造一个"模型调一次 `probe`、再收尾"的运行时；`probe` 的参数由用例给。
    fn runtime_with_capturing_probe(
        runner: Arc<dyn HookRunner>,
        access: Access,
        schema: serde_json::Value,
        approval_policy: ApprovalPolicy,
        raw_args: &str,
    ) -> (
        AgentRuntime,
        Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        Arc<DangerScanGate>,
    ) {
        use crate::model::{StreamDelta as SD, ToolCallInfo};
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tool: Arc<dyn Tool> = Arc::new(CapturingTool {
            descriptor: ToolDescriptor {
                name: "probe".to_string(),
                summary: "探针".to_string(),
                schema,
                access,
                approval: approval_policy,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            seen: seen.clone(),
        });
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                SD::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "probe".into(),
                        args: raw_args.to_string(),
                    },
                },
                SD::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                SD::Text { text: "done".into() },
                SD::Done { stop_reason: "stop".into() },
            ],
        ]));
        let gate = Arc::new(DangerScanGate::default());
        let rt = AgentRuntime::new(
            model,
            Arc::new(InMemoryToolCatalog::with_tools(vec![tool])),
            gate.clone(),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(FixedPrompt::new("system")),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
        )
        .with_hooks(runner);
        (rt, seen, gate)
    }

    /// 跑一轮并返回（看到的参数、接收器）。
    async fn run_probe_turn(
        rt: &AgentRuntime,
        thread: &str,
    ) -> RecordingSink {
        let sink = RecordingSink::new();
        rt.run_turn(
            TurnRequest::new(thread, mock_provider_config()).with_user_prompt("go"),
            &sink,
            &NeverCancel,
        )
        .await
        .expect("轮次应正常结束");
        sink
    }

    fn diag_reason(sink: &RecordingSink, point: &str) -> Option<String> {
        sink.snapshot().into_iter().find_map(|e| match e.body {
            AgentEventBody::Diagnostic(d) if d.point == point => d.reason,
            _ => None,
        })
    }

    /// SP-60/61：改写**生效**——工具真的拿到新参数，而且留了改写痕迹（指纹）。
    #[tokio::test]
    async fn test_replacement_is_applied_and_the_tool_sees_new_args() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "cmd": "echo 更精确的路径" }),
        ));
        let (rt, seen, _gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({ "type": "object" }),
            ApprovalPolicy::Never,
            r#"{"cmd":"echo 太宽了"}"#,
        );
        let sink = run_probe_turn(&rt, "t_replace").await;

        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen.len(), 1, "工具应当被执行一次：{seen:?}");
        assert_eq!(
            seen[0].get("cmd").and_then(|v| v.as_str()),
            Some("echo 更精确的路径"),
            "执行用的必须是**改写后**的参数：{seen:?}"
        );
        let replaced = sink
            .snapshot()
            .into_iter()
            .find_map(|e| match e.body {
                AgentEventBody::Diagnostic(d) if d.point == POINT_HOOK_REPLACE_APPLIED => Some(d),
                _ => None,
            })
            .expect("改写生效必须留下 hook.replace 诊断");
        assert_eq!(
            replaced.origin,
            Origin::Plugin { id: "fake-plugin".to_string() },
            "要能归因到哪个插件改的"
        );
        assert!(
            replaced.data.get("beforeHash").is_some() && replaced.data.get("afterHash").is_some(),
            "改前改后的指纹要带上：{:?}",
            replaced.data
        );
    }

    /// SP-61（安全序的关键一条）：**审批读的是改写后的 args**。
    ///
    /// 用例设计成"改之前无害、改之后危险"：如果审批读的是原参数，它就永远不会被问到——
    /// 那正是这条链要防的形态（插件把危险命令塞进参数、审批却按旧参数放行）。
    #[tokio::test]
    async fn test_approval_reads_the_rewritten_args() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "cmd": "rm -rf /workspace" }),
        ));
        let (rt, seen, gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({ "type": "object" }),
            ApprovalPolicy::DangerScan { patterns: vec!["rm -rf".to_string()] },
            r#"{"cmd":"echo 无害"}"#,
        );
        let _sink = run_probe_turn(&rt, "t_risk_up").await;

        let asked = gate.asked.lock().expect("lock").clone();
        assert_eq!(asked.len(), 1, "改写后的危险参数**必须**触发审批：{asked:?}");
        assert!(
            asked[0].to_string().contains("rm -rf"),
            "闸门看到的必须是改写后的参数：{asked:?}"
        );
        let seen = seen.lock().expect("lock").clone();
        assert!(
            seen[0].to_string().contains("rm -rf"),
            "审批通过后执行的是改写后的参数：{seen:?}"
        );
    }

    /// SP-62：**风险单调性**——改写不得把"需要审批"降成"不需要"。
    ///
    /// 这是最容易被绕过的一环：执行的是改写后的参数，所以只要允许把危险形式改成无害形式
    /// 骗过审批，闸门就形同虚设。被拒时**用回原参数**，并留下理由。
    #[tokio::test]
    async fn test_risk_downgrade_is_rejected_and_original_args_are_used() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "cmd": "echo 看起来人畜无害" }),
        ));
        let (rt, seen, gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({ "type": "object" }),
            ApprovalPolicy::DangerScan { patterns: vec!["rm -rf".to_string()] },
            r#"{"cmd":"rm -rf /workspace"}"#,
        );
        let sink = run_probe_turn(&rt, "t_risk_down").await;

        let asked = gate.asked.lock().expect("lock").clone();
        assert_eq!(asked.len(), 1, "原参数该问就问：{asked:?}");
        assert!(
            asked[0].to_string().contains("rm -rf"),
            "闸门看到的应当是**原**参数：{asked:?}"
        );
        let seen = seen.lock().expect("lock").clone();
        assert!(
            seen[0].to_string().contains("rm -rf"),
            "被拒的改写不得生效，执行的必须是原参数：{seen:?}"
        );
        let reason = diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).expect("降级企图必须留痕");
        assert!(
            reason.starts_with("risk_downgrade"),
            "理由要说清是风险降级：{reason}"
        );
    }

    /// SP-62：改写**不得把路径挪出作用域**——越界路径必须被重新校验拦下。
    #[tokio::test]
    async fn test_out_of_scope_path_in_replacement_is_rejected() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "path": "/etc/passwd" }),
        ));
        let (rt, seen, _gate) = runtime_with_capturing_probe(
            runner,
            Access::Mutates {
                paths: PathSelector::Single("path"),
            },
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            ApprovalPolicy::Never,
            r#"{"path":"ws/a.txt"}"#,
        );
        let sink = run_probe_turn(&rt, "t_path").await;

        let seen = seen.lock().expect("lock").clone();
        assert_eq!(
            seen[0].get("path").and_then(|v| v.as_str()),
            Some("ws/a.txt"),
            "越界改写必须被拒、用回原参数：{seen:?}"
        );
        let reason = diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).expect("越界改写必须留痕");
        assert!(reason.contains("path_out_of_scope"), "{reason}");
    }

    /// SP-62：改写不得让参数**脱离工具契约**（schema 由描述符声明，插件改不动它）。
    #[tokio::test]
    async fn test_schema_mismatch_in_replacement_is_rejected() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "count": "不是数字" }),
        ));
        let (rt, seen, _gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({
                "type": "object",
                "properties": { "count": { "type": "integer" } },
                "required": ["count"]
            }),
            ApprovalPolicy::Never,
            r#"{"count":1}"#,
        );
        let sink = run_probe_turn(&rt, "t_schema").await;

        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen[0].get("count").and_then(|v| v.as_i64()), Some(1), "{seen:?}");
        assert_eq!(
            diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).as_deref(),
            Some("schema_mismatch"),
            "schema 不符必须留痕"
        );
    }

    /// SP-62：**描述符字段不可改**——载荷只带 args，工具名与读写性都改不动。
    #[tokio::test]
    async fn test_replacement_cannot_change_the_tool_identity() {
        let runner = Arc::new(ScriptedHookRunner::new().replace_at(
            "beforeToolCall",
            serde_json::json!({ "tool": "read_file", "cmd": "echo hi" }),
        ));
        let (rt, seen, _gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({ "type": "object" }),
            ApprovalPolicy::Never,
            r#"{"cmd":"echo 原样"}"#,
        );
        let sink = run_probe_turn(&rt, "t_identity").await;

        let started: Vec<String> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::ToolCallStarted { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(started, vec!["probe".to_string()], "工具名改不动：{started:?}");
        assert_eq!(seen.lock().expect("lock").len(), 1);
    }

    /// 没有插件订阅（或插件只观察）时，改写路径不许影响任何东西。
    #[tokio::test]
    async fn test_no_replacement_path_when_nothing_subscribes() {
        let runner = Arc::new(ScriptedHookRunner::new().unsubscribed());
        let (rt, seen, _gate) = runtime_with_capturing_probe(
            runner,
            Access::ReadOnly,
            serde_json::json!({ "type": "object" }),
            ApprovalPolicy::Never,
            r#"{"cmd":"原样"}"#,
        );
        let sink = run_probe_turn(&rt, "t_nosub").await;
        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen[0].get("cmd").and_then(|v| v.as_str()), Some("原样"));
        assert!(
            !sink.snapshot().iter().any(|e| matches!(
                &e.body,
                AgentEventBody::Diagnostic(d)
                    if d.point == POINT_HOOK_REPLACE_APPLIED
                        || d.point == POINT_HOOK_REPLACE_REJECTED
            )),
            "没订阅就没有改写这回事"
        );
    }

    #[tokio::test]
    async fn test_turn_completes_without_tools() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Thinking { thinking: "思考中...".into() },
            StreamDelta::Text { text: "你好！".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));

        let runtime = AgentRuntime::new(
            model,
            catalog,
            approval,
            store.clone(),
            prompt,
            scope,
            clock,
            RunPolicy::default(),
        );

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config()).with_user_prompt("嗨");

        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
        assert_eq!(outcome.steps_taken, 1);

        // 验证事件序列（只看对话面）
        let kinds = conversational_kinds(&sink);
        assert_eq!(kinds, vec!["turn.started", "thinking.delta", "text.delta", "usage", "turn.finished"]);

        // 验证恰好一个 TurnFinished
        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);

        // 验证事件单调有序
        let events = sink.snapshot();
        for i in 1..events.len() {
            assert_eq!(events[i].seq, events[i - 1].seq + 1);
        }

        // 验证消息记录落库：用户消息 + Assistant 消息
        let messages = store.get("t1");
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], AgentMessage::User { .. }));
        assert!(matches!(messages[1], AgentMessage::Assistant { .. }));
    }

    /// 只看**对话面**的事件名序列。
    ///
    /// 诊断遥测（P0 埋点）刻意不属于这条序列：把它混进来会让"这一轮到底发了什么"
    /// 变成噪音。它自己另有断言（见 `test_diagnostics_carry_trace_and_stay_out_of_conversation`），
    /// 所以这里过滤掉**不等于**放过它。
    fn conversational_kinds(sink: &RecordingSink) -> Vec<&'static str> {
        sink.snapshot()
            .into_iter()
            .filter(|e| !matches!(e.body, AgentEventBody::Diagnostic(_)))
            .map(|e| e.body.kind())
            .collect()
    }

    #[tokio::test]
    async fn test_turn_aborted_before_start() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));
        let model = Arc::new(ScriptedModelClient::new(vec![]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = ManualCancel::new();
        cancel.cancel(); // 启动前即取消

        let req = TurnRequest::new("t1", mock_provider_config()).with_user_prompt("嗨");
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::Aborted);
        assert_eq!(outcome.steps_taken, 0);

        let kinds = conversational_kinds(&sink);
        assert_eq!(kinds, vec!["turn.started", "turn.finished"]);

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
    }

    #[tokio::test]
    async fn test_turn_aborted_during_stream() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        // 模型在流结束时自报 aborted
        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Thinking { thinking: "想...".into() },
            StreamDelta::Done { stop_reason: "aborted".into() },
        ]]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::Aborted);

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
    }

    #[tokio::test]
    async fn test_turn_model_error_yields_single_finished() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Error { error: "连接超时 504".into() },
        ]]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::ModelError);

        let kinds = conversational_kinds(&sink);
        assert_eq!(kinds, vec!["turn.started", "failed", "turn.finished"]);

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
    }

    /// SP-43：`prompt.composed` 必须**按片段**报出来（"这段是谁加的、多少字节"）。
    ///
    /// `FixedPrompt` 是不细分的替身，所以这里自造一个会逐段报告的源——
    /// 否则这条用例只能证明"有 segments 字段"，证明不了"片段真的被报出来"。
    #[tokio::test]
    async fn test_prompt_composed_diagnostic_reports_segments() {
        use crate::ports::{PromptSegment, PromptSource};

        struct SegmentedPrompt;
        impl PromptSource for SegmentedPrompt {
            fn system_prompt(&self) -> String {
                "AB".to_string()
            }
            fn prompt_segments(&self) -> Vec<PromptSegment> {
                vec![
                    PromptSegment {
                        id: "persona".to_string(),
                        source: "builtin:core".to_string(),
                        bytes: 1,
                    },
                    PromptSegment {
                        id: "extra".to_string(),
                        source: "plugin:p1".to_string(),
                        bytes: 1,
                    },
                ]
            }
        }

        let runtime = AgentRuntime::new(
            Arc::new(ScriptedModelClient::new(vec![vec![
                StreamDelta::Text { text: "ok".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ]])),
            Arc::new(InMemoryToolCatalog::new()),
            Arc::new(RecordingApprovalGate::new(true)),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(SegmentedPrompt),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy::default(),
        );

        let sink = RecordingSink::new();
        runtime
            .run_turn(TurnRequest::new("t1", mock_provider_config()), &sink, &NeverCancel)
            .await
            .unwrap();

        let composed = sink
            .snapshot()
            .into_iter()
            .find_map(|e| match e.body {
                AgentEventBody::Diagnostic(d) if d.point == POINT_PROMPT_COMPOSED => Some(d),
                _ => None,
            })
            .expect("每轮必须发 prompt.composed");
        assert_eq!(composed.bytes, Some(2), "总字节要如实上报");
        let segments = composed
            .data
            .get("segments")
            .and_then(|v| v.as_array())
            .expect("要带 segments");
        assert_eq!(segments.len(), 2, "{segments:?}");
        assert_eq!(segments[0]["id"], "persona");
        assert_eq!(segments[1]["source"], "plugin:p1", "来源要能看出是插件加的");
        assert_eq!(composed.data["totalBytes"], serde_json::json!(2));
    }

    /// SP-08 契约（P0 埋点地基）：诊断遥测**真的被发出来、带得上关联键、且不进对话序列**。
    ///
    /// 守三件事：
    /// 1. 每轮**必须有** `prompt.composed`（否则"提示词组装"这个扩展点等于没有观测）；
    /// 2. 全部事件携带**同一个 `turn_id`**，工具事件还带 `call_id` 与来源（归因可用）；
    /// 3. 诊断不进对话面序列——它是给人排查用的，不是对话的一部分。
    #[tokio::test]
    async fn test_diagnostics_carry_trace_and_stay_out_of_conversation() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
        let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 1010, 1020)));
        catalog.register(tool);

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "call_1".into(), name: "echo".into(), args: "{}".into() },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "完成".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());
        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        runtime
            .run_turn(TurnRequest::new("t1", mock_provider_config()), &sink, &cancel)
            .await
            .unwrap();

        let events = sink.snapshot();

        // 1. 提示词组装的埋点必须在（每轮一次），且如实上报字节数
        let composed = events
            .iter()
            .find_map(|e| match &e.body {
                AgentEventBody::Diagnostic(d) if d.point == POINT_PROMPT_COMPOSED => Some(d),
                _ => None,
            })
            .expect("每轮必须发 `prompt.composed`——否则提示词组装这个扩展点没有观测");
        assert_eq!(composed.family, DiagFamily::Prompt);
        assert_eq!(composed.origin, Origin::Engine);
        assert!(composed.bytes.unwrap_or(0) > 0, "提示词长度必须如实上报");

        // 2. 关联键：一轮里所有事件共享同一个 turn_id；工具事件带 call_id 与来源
        let turn_ids: Vec<&str> = events.iter().filter_map(|e| e.trace.turn_id.as_deref()).collect();
        assert_eq!(turn_ids.len(), events.len(), "引擎发出的每个事件都必须带 turn_id");
        assert!(
            turn_ids.iter().all(|t| *t == turn_ids[0]),
            "同一轮不得出现两个 turn_id：{turn_ids:?}"
        );

        let started = events
            .iter()
            .find(|e| matches!(e.body, AgentEventBody::ToolCallStarted { .. }))
            .expect("这一轮应当有一次工具调用");
        assert_eq!(started.trace.call_id.as_deref(), Some("call_1"));
        assert_eq!(started.trace.origin, Origin::Builtin { name: "echo".into() });
        assert!(started.trace.step_index >= 1, "工具调用发生在第 1 步或之后");

        // 3. 诊断不进对话面
        let conv = conversational_kinds(&sink);
        assert!(!conv.contains(&"diagnostic"), "诊断不得混进对话序列：{conv:?}");
        assert!(conv.contains(&"tool.started") && conv.contains(&"tool.finished"));
    }

    /// 归因复位：工具调用结束后，后续事件不应继续挂着那个 `call_id`。
    #[tokio::test]
    async fn test_trace_call_id_is_cleared_after_tool_call() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
        catalog.register(Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 0, 0))));

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "call_9".into(), name: "echo".into(), args: "{}".into() },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "完成".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());
        let sink = RecordingSink::new();
        runtime
            .run_turn(TurnRequest::new("t1", mock_provider_config()), &sink, &NeverCancel)
            .await
            .unwrap();

        let finished = sink
            .snapshot()
            .into_iter()
            .find(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
            .expect("必须有收尾事件");
        assert_eq!(
            finished.trace.call_id, None,
            "工具调用结束后 call_id 必须复位，否则后续事件的归因是错的"
        );
        assert_eq!(finished.trace.origin, Origin::Engine);
    }

    #[tokio::test]
    async fn test_tool_call_started_before_finished_and_multi_turn() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        // 注册一个简单工具
        let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
        let receipt = ToolReceipt::success("echoed_result", 1010, 1020);
        let tool = Arc::new(MockTool::new(desc, receipt));
        catalog.register(tool);

        // 轮次 1：模型发起工具调用；轮次 2：收到工具回执后完成
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "call_1".into(),
                        name: "echo".into(),
                        args: "{\"arg\":\"hello\"}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "工具执行成功！".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval, store.clone(), prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
        assert_eq!(outcome.steps_taken, 2);

        // 断言 ToolCallStarted 先于 ToolCallFinished
        let kinds = sink.kinds();
        let started_pos = kinds.iter().position(|k| *k == "tool.started").unwrap();
        let finished_pos = kinds.iter().position(|k| *k == "tool.finished").unwrap();
        assert!(started_pos < finished_pos);

        // 恰好一个收尾
        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);

        // 消息中包含了 ToolResult
        let messages = store.get("t1");
        assert_eq!(messages.len(), 3); // assistant(call) -> tool_result -> assistant(text)
        assert!(matches!(messages[1], AgentMessage::ToolResult { .. }));
    }

    /// 守门：**上下文撞顶必须提前失败**，而不是发一个注定被拒的请求。
    ///
    /// 实测事故：445k tokens 的请求被网关在流式过程中掐断，报告只写了一半，
    /// 界面只看到笼统的"模型调用失败"。这里用"上一轮如实报回的用量 + 登记的窗口"
    /// 在模型调用**之前**判定：撞顶就 `ContextWindowExceeded` 收尾，且不再发起第 2 次调用。
    #[tokio::test]
    async fn test_context_window_guard_stops_before_over_limit_request() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
        let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("echoed", 0, 0)));
        catalog.register(tool);

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "call_ctx".into(),
                        name: "echo".into(),
                        args: "{}".into(),
                    },
                },
                // 上一轮的真实用量：445k（远超 128k 窗口）
                StreamDelta::Usage {
                    usage: TokenUsage {
                        prompt_tokens: 445_080,
                        completion_tokens: 557,
                        total_tokens: 445_637,
                        ..Default::default()
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "不该走到这里".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());
        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t_ctx", mock_provider_config()).with_context_window(Some(128_000));

        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(
            outcome.stop_reason,
            TurnStopReason::ContextWindowExceeded {
                prompt_tokens: 445_637,
                window: 128_000
            },
            "必须如实自报是上下文撞顶（不是笼统的模型错误）"
        );
        assert_eq!(outcome.steps_taken, 1, "只跑了第 1 轮，第 2 轮不该被发起");

        // 撞顶由**停止原因**承载（不再额外发 Failed：那是"笼统一句模型调用失败"的老毛病）
        let events = sink.snapshot();
        assert!(
            !events.iter().any(|e| matches!(&e.body, AgentEventBody::Failed { .. })),
            "撞顶不该再发 Failed——理由由 TurnStopReason 承载，调度层据此自动压缩续跑"
        );
        assert!(
            !events.iter().any(|e| matches!(&e.body, AgentEventBody::TextDelta { text } if text.contains("不该走到这里"))),
            "撞顶后不得再发起模型调用"
        );
    }

    #[tokio::test]
    async fn test_approval_denied_returns_as_tool_result_without_throwing() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());

        // 注册必须审批的工具，审批闸门配置为拒绝
        let desc = make_test_descriptor("dangerous_cmd", ApprovalPolicy::Always, Termination::ContinueTurn);
        let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 0, 0)));
        catalog.register(tool);

        let approval = Arc::new(RecordingApprovalGate::new(false)); // 拒绝

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "call_dang".into(),
                        name: "dangerous_cmd".into(),
                        args: "{\"arg\":\"rm -rf\"}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "操作被拒绝，放弃执行。".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval.clone(), store.clone(), prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::Completed);

        // 断言审批闸门被调了一次
        assert_eq!(approval.recorded_calls().len(), 1);
        assert_eq!(approval.recorded_calls()[0].tool, "dangerous_cmd");

        // 断言 Denied 作为工具结果返回给模型
        let messages = store.get("t1");
        assert_eq!(messages.len(), 3);
        match &messages[1] {
            AgentMessage::ToolResult { is_error, status, .. } => {
                assert_eq!(is_error, &Some(true));
                assert_eq!(status.as_deref(), Some("denied"));
            }
            _ => panic!("必须为 ToolResult"),
        }
    }

    #[tokio::test]
    async fn test_termination_end_turn_stops_entire_turn() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        // 声明 Termination::EndTurn 的工具（例如 finish 或 terminate）
        let desc = make_test_descriptor("finish", ApprovalPolicy::Never, Termination::EndTurn);
        let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("任务已结束", 1000, 1005)));
        catalog.register(tool);

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "call_fin".into(),
                        name: "finish".into(),
                        args: "{}".into(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
        ]));

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, RunPolicy::default());

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        // 步数为 1，直接完成整轮
        assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
        assert_eq!(outcome.steps_taken, 1);

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
    }

    #[tokio::test]
    async fn test_budget_exhausted_reported_explicitly() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));
        let catalog = Arc::new(InMemoryToolCatalog::new());
        let approval = Arc::new(RecordingApprovalGate::new(true));

        let desc = make_test_descriptor("step_tool", ApprovalPolicy::Never, Termination::ContinueTurn);
        catalog.register(Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 0, 0))));

        // 持续返回工具调用
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "c1".into(), name: "step_tool".into(), args: "{}".into() },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "c2".into(), name: "step_tool".into(), args: "{}".into() },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
        ]));

        let policy = RunPolicy {
            max_steps: Some(1), // 预算上限设为 1 步
            max_parallel_tools: 1,
            tool_timeout: None,
        };

        let runtime = AgentRuntime::new(model, catalog, approval, store, prompt, scope, clock, policy);

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new("t1", mock_provider_config());
        let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

        assert_eq!(outcome.stop_reason, TurnStopReason::BudgetExhausted { limit_steps: 1 });
        // W3-T5 补强：`steps_taken` 必须等于**实际执行**的步数。
        // 原来这里只断言 stop_reason，于是 off-by-one（报 2 而非 1）长期无人发现。
        assert_eq!(
            outcome.steps_taken, 1,
            "max_steps=1 时只应执行 1 步；被预算拒绝的那次尝试不算执行"
        );

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
    }

    /// **设置项 → 引擎 → 适配器**的契约：`TurnRequest::with_max_retries` 必须原样出现在    /// 模型请求的 `options.max_retries` 里（界面上的"请求重试次数"就是靠这条路生效的）。
    ///
    /// 不写这条就会出现"设置项存下来了、界面也显示 5，但真的发请求时还是写死的 3"——
    /// 正是本仓反复出现的"看起来装上了其实没接线"。
    #[tokio::test]
    async fn test_max_retries_reaches_the_model_request() {
        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "ok".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));
        let runtime = AgentRuntime::new(
            model.clone(),
            Arc::new(InMemoryToolCatalog::new()),
            Arc::new(RecordingApprovalGate::new(true)),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(FixedPrompt::new("system prompt")),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy { max_steps: Some(1), max_parallel_tools: 1, tool_timeout: None },
        );

        // 1) 显式设置 7 次 → 必须原样送达
        let sink = RecordingSink::new();
        let req = TurnRequest::new("t_retry_setting", mock_provider_config())
            .with_user_prompt("go")
            .with_max_retries(Some(7));
        runtime.run_turn(req, &sink, &NeverCancel).await.unwrap();

        let seen = model.requests();
        assert_eq!(seen.len(), 1, "一轮只该有一次模型请求");
        assert_eq!(
            seen[0].options.max_retries,
            Some(7),
            "界面设置的重试次数必须原样送到适配器"
        );

        // 2) 不设置 → 落到默认值（既不是 0，也不是早先写死的 3）
        let model2 = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "ok".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));
        let runtime2 = AgentRuntime::new(
            model2.clone(),
            Arc::new(InMemoryToolCatalog::new()),
            Arc::new(RecordingApprovalGate::new(true)),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(FixedPrompt::new("system prompt")),
            Arc::new(MockScope::new("ws")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy { max_steps: Some(1), max_parallel_tools: 1, tool_timeout: None },
        );
        let sink2 = RecordingSink::new();
        runtime2
            .run_turn(
                TurnRequest::new("t_retry_default", mock_provider_config()).with_user_prompt("go"),
                &sink2,
                &NeverCancel,
            )
            .await
            .unwrap();
        assert_eq!(
            model2.requests()[0].options.max_retries,
            Some(crate::model::DEFAULT_MODEL_RETRY_COUNT),
            "未设置时应当用默认重试次数"
        );
    }

    /// 一个**永远不返回**的工具，必须被策略超时（`RunPolicy::tool_timeout`）切断，
    /// 而不是把整轮永远挂住——这正是"工具卡住 → 智能体停止"那条症状的兜底。
    struct HangingTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for HangingTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute<'a>(
            &'a self,
            _call: &'a ToolCall,
            _ctx: &'a ToolContext<'a>,
        ) -> crate::ports::BoxFuture<'a, ToolReceipt> {
            Box::pin(async {
                // 永不返回：没有引擎侧超时就等于整轮死等
                std::future::pending::<()>().await;
                unreachable!()
            })
        }
    }

    /// 声明"我合法地长时间运行"的工具必须**不受**策略超时约束。
    struct LongRunningHangingTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for LongRunningHangingTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn long_running(&self) -> bool {
            true
        }
        fn execute<'a>(
            &'a self,
            _call: &'a ToolCall,
            _ctx: &'a ToolContext<'a>,
        ) -> crate::ports::BoxFuture<'a, ToolReceipt> {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                ToolReceipt::success("慢但合法", 0, 1)
            })
        }
    }

    #[tokio::test]
    async fn test_tool_timeout_cuts_a_hanging_tool_and_turn_continues() {
        let clock = Arc::new(FixedClock::new(1000));
        let store = Arc::new(InMemorySessionStore::new());
        let prompt = Arc::new(FixedPrompt::new("system prompt"));
        let scope = Arc::new(MockScope::new("ws"));

        let hang = Arc::new(HangingTool {
            descriptor: make_test_descriptor(
                "hang_tool",
                ApprovalPolicy::Never,
                Termination::ContinueTurn,
            ),
        });
        // 一个"合法的慢工具"：同样耗时，但声明豁免 → 不该被超时掐掉
        let slow = Arc::new(LongRunningHangingTool {
            descriptor: make_test_descriptor(
                "slow_tool",
                ApprovalPolicy::Never,
                Termination::ContinueTurn,
            ),
        });
        let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![hang, slow]));
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "c1".into(), name: "hang_tool".into(), args: "{}".into() },
                },
                StreamDelta::ToolCall {
                    call: ToolCallInfo { id: "c2".into(), name: "slow_tool".into(), args: "{}".into() },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        let rt = AgentRuntime::new(
            model,
            catalog,
            Arc::new(RecordingApprovalGate::new(true)),
            store,
            prompt,
            scope,
            clock,
            RunPolicy {
                max_steps: Some(3),
                max_parallel_tools: 1,
                tool_timeout: Some(std::time::Duration::from_millis(120)),
            },
        );

        let sink = RecordingSink::new();
        let started = std::time::Instant::now();
        let outcome = rt
            .run_turn(
                TurnRequest::new("t_tool_timeout", mock_provider_config()).with_user_prompt("go"),
                &sink,
                &NeverCancel,
            )
            .await
            .expect("超时必须被兜住，整轮照常收尾");
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "挂住的工具必须被超时切断（实际用了 {elapsed:?}）"
        );
        assert_eq!(outcome.stop_reason, TurnStopReason::Completed, "超时后整轮应继续跑完");

        let receipts: Vec<ToolReceipt> = sink
            .snapshot()
            .into_iter()
            .filter_map(|e| match e.body {
                AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt),
                _ => None,
            })
            .collect();
        let hang_receipt = receipts
            .iter()
            .find(|r| r.output.contains("超过"))
            .expect("挂住的工具必须留下一条超时回执（否则界面只有一张永远转圈的卡片）");
        assert_eq!(hang_receipt.status, ToolStatus::Timeout);
        assert!(
            hang_receipt.output.contains("hang_tool"),
            "超时回执要说清是哪个工具：{}",
            hang_receipt.output
        );
        assert!(
            receipts.iter().any(|r| r.status == ToolStatus::Success && r.output.contains("慢但合法")),
            "声明 long_running 的慢工具不该被策略超时掐掉"
        );
    }
}
