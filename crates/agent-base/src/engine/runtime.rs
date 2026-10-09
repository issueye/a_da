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
    AgentEvent, AgentEventBody, AgentError, AgentMessage, ApprovalPolicy, FailDirection,
    Termination, ToolCall, ToolCallBlock, ToolReceipt, ToolStatus,
    TurnStopReason,
};
use crate::engine::policy::RunPolicy;
use crate::engine::prompt_format::format_messages_for_model;
use crate::engine::turn::{TurnOutcome, TurnRequest};
use crate::model::{
    ChatCompletionTool, ChatCompletionToolFunction, ModelChatOptions,
    StreamDelta, TokenUsage,
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
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    fn emit(&self, sink: &dyn EventSink, thread_id: &str, body: AgentEventBody) {
        let seq = self.next_seq();
        let at_ms = self.clock.now_ms();
        sink.emit(AgentEvent::new(seq, at_ms, thread_id, body));
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

        // 1. 发送 TurnStarted
        self.emit(sink, &thread_id, AgentEventBody::TurnStarted);

        // 2. 检查启动前是否已取消
        if cancel.is_cancelled() {
            self.emit(sink, &thread_id, AgentEventBody::TurnFinished { stop: TurnStopReason::Aborted });
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
        let final_stop_reason: TurnStopReason;

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
            step_index += 1;

            // 检查取消
            if cancel.is_cancelled() {
                final_stop_reason = TurnStopReason::Aborted;
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
                max_retries: Some(3),
            });
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
                    self.emit(sink, &thread_id, AgentEventBody::Failed { message: format!("{:?}", err) });
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
                        self.emit(sink, &thread_id, AgentEventBody::ThinkingDelta { text: thinking });
                    }
                    StreamDelta::Text { text } => {
                        accumulated_text.push_str(&text);
                        self.emit(sink, &thread_id, AgentEventBody::TextDelta { text });
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
                        self.emit(sink, &thread_id, AgentEventBody::Failed { message: error });
                        break;
                    }
                }
            }

            if let Some(_err) = stream_error {
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
            self.emit(sink, &thread_id, AgentEventBody::UsageReported {
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

                let parsed_args = serde_json::from_str(&raw_call.args).unwrap_or(serde_json::Value::Null);
                let tool_call = ToolCall {
                    id: raw_call.id.clone(),
                    name: raw_call.name.clone(),
                    args: parsed_args.clone(),
                };

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

                        // 审批检查（M1-T4 + W1-T6）：
                        // **策略归端口**——引擎不解释档位/危险命令/白名单/Named 策略，
                        // 只把工具名、声明与参数递过去（AGENTS.md §14）。
                        // `Never` 是唯一的例外：它连问都不必问，引擎直接短路。
                        let needs_approval = match &desc.approval {
                            ApprovalPolicy::Never => false,
                            policy => {
                                self.approval
                                    .needs_approval(&raw_call.name, policy, &parsed_args)
                                    .await
                            }
                        };

                        let mut is_denied = false;
                        let mut denial_reason = None;

                        if needs_approval {
                            self.emit(sink, &thread_id, AgentEventBody::ApprovalRequested {
                                call_id: raw_call.id.clone(),
                                tool: raw_call.name.clone(),
                            });

                            let app_req = ApprovalRequest {
                                call_id: raw_call.id.clone(),
                                thread_id: thread_id.clone(),
                                tool: raw_call.name.clone(),
                                args: parsed_args.clone(),
                                is_write,
                                reason: None,
                            };

                            let outcome = self.approval.decide(app_req, Some(cancel)).await;
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
                            if !outcome.approved {
                                is_denied = true;
                                denial_reason = outcome.reason.or_else(|| Some("审批未通过".to_string()));
                            }
                        }

                        if is_denied {
                            // 合规断言：Denied 以工具结果回模型，不抛错
                            let now = self.clock.now_ms();
                            let receipt = ToolReceipt::denied(
                                denial_reason.unwrap_or_else(|| "操作被拒绝".to_string()),
                                now,
                                now,
                            );

                            self.emit(sink, &thread_id, AgentEventBody::ToolCallFinished {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                receipt: receipt.clone(),
                            });

                            let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                            self.store.append_message(&thread_id, &result_msg).await?;
                            history.push(result_msg);
                        } else {
                            // 审批通过，执行工具
                            self.emit(sink, &thread_id, AgentEventBody::ToolCallStarted {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                args: parsed_args,
                            });

                            let ctx = ToolContext {
                                scope: self.scope.as_ref(),
                                cancel,
                                events: sink,
                                thread_id: &thread_id,
                            };

                            let receipt = tool.execute(&tool_call, &ctx).await;

                            self.emit(sink, &thread_id, AgentEventBody::ToolCallFinished {
                                call_id: raw_call.id.clone(),
                                name: raw_call.name.clone(),
                                receipt: receipt.clone(),
                            });

                            let result_msg = Self::receipt_to_agent_message(&raw_call.id, &raw_call.name, &receipt);
                            self.store.append_message(&thread_id, &result_msg).await?;
                            history.push(result_msg);

                            // 消费 Termination（M1-T6）
                            if desc.termination == Termination::EndTurn {
                                should_terminate_turn = true;
                            }
                        }
                    }
                }
            }

            if should_terminate_turn {
                final_stop_reason = TurnStopReason::Completed;
                break 'turn_loop;
            }
        }

        // 6. 收尾保证：每轮恰好发出一个 TurnFinished！
        self.emit(sink, &thread_id, AgentEventBody::TurnFinished { stop: final_stop_reason.clone() });

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
    use crate::domain::{Access, Execution, RollbackPolicy, ToolDescriptor};
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

        // 验证事件序列
        let kinds = sink.kinds();
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

        let kinds = sink.kinds();
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

        let kinds = sink.kinds();
        assert_eq!(kinds, vec!["turn.started", "failed", "turn.finished"]);

        let turn_finished_count = sink.snapshot().into_iter().filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. })).count();
        assert_eq!(turn_finished_count, 1);
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
}
