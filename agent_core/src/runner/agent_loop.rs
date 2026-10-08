use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tracing::warn;


use super::executor::execute_tool_call_extended;
use super::prompt::{build_system_prompt, get_all_tools_for_workspace, format_messages_for_model};
use crate::ai::{stream_model_chat, ModelChatOptions, ProviderConfig, StreamDelta, TokenUsage};
use crate::checkpoint::CheckpointManager;
use crate::session::{AgentMessage, SessionManager, ToolCallBlock};

#[derive(Debug, Clone)]
pub enum AgentLoopEvent {
    Thinking { text: String },
    TextDelta { text: String },
    ToolCallStarted { name: String, id: String, args: String },
    ToolCallFinished {
        name: String,
        id: String,
        ok: bool,
        output: Option<String>,
        duration_ms: Option<u64>,
        started_at: Option<i64>,
        finished_at: Option<i64>,
        status: Option<String>,
    },
    ToolAwaitingQuestion { id: String, question: serde_json::Value },
    /// 一次大模型调用结束后的真实用量与耗时（界面遥测条与单条回复徽章的数据源）
    AssistantStats { usage: Option<TokenUsage>, duration_ms: u64, turn_duration_ms: u64 },
    TurnFinished { stop_reason: String },
    Error { message: String },
}

/// 运行主 Agent 决策与执行多轮流式循环
pub async fn run_agent_loop(
    workspace: &Path,
    thread_id: &str,
    user_prompt: Option<&str>,
    provider_config: ProviderConfig,
    session_mgr: Arc<SessionManager>,
    checkpoint_mgr: Arc<CheckpointManager>,
    event_tx: mpsc::Sender<AgentLoopEvent>,
    abort_rx: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let ws_str = workspace.to_string_lossy().to_string();
    // 整轮耗时起点：含思考、工具执行与流式生成，与界面「总耗时」口径一致
    let turn_start = now_ms();

    // 1. 如果有新用户输入，先记录并落盘
    if let Some(prompt) = user_prompt {
        if !prompt.trim().is_empty() {
            let user_msg = AgentMessage::User {
                content: prompt.to_string(),
                images: None,
                timestamp: Some(now_ms()),
            };
            session_mgr.append_message(thread_id, user_msg, Some(&ws_str))?;
        }
    }

    // 2. 加载当前会话历史
    let mut history_messages = match session_mgr.load_session(thread_id, Some(&ws_str))? {
        Some((_, msgs)) => msgs,
        None => Vec::new(),
    };

    let system_prompt = build_system_prompt(&ws_str);
    let tools = get_all_tools_for_workspace(&ws_str);

    // 3. 多轮驱动：完全由模型（无工具调用即完成）、异常或用户中断决定退出，不设人为步数上限
    loop {
        // 如果已接收到取消信号，直接终止多轮循环
        if let Some(ref rx) = abort_rx {
            if *rx.borrow() {
                let _ = event_tx
                    .send(AgentLoopEvent::TurnFinished {
                        stop_reason: "aborted".to_string(),
                    })
                    .await;
                break;
            }
        }

        let step_start = now_ms();
        let mut step_usage: Option<TokenUsage> = None;
        let mut accumulated_usage = TokenUsage::default();

        let chat_messages = format_messages_for_model(&system_prompt, &history_messages);
        let options = ModelChatOptions {
            tools: Some(tools.clone()),
            system_prompt: None,
            temperature: Some(0.2),
            effort: None,
            max_retries: Some(3),
        };

        let mut stream_rx = stream_model_chat(
            provider_config.clone(),
            chat_messages,
            options,
            abort_rx.clone(),
        )
        .await;

        let mut accumulated_text = String::new();
        let mut accumulated_thinking = String::new();
        let mut tool_calls = Vec::new();
        let mut turn_stop_reason = "stop".to_string();
        let mut had_error = false;

        while let Some(delta) = stream_rx.recv().await {
            match delta {
                StreamDelta::Thinking { thinking } => {
                    accumulated_thinking.push_str(&thinking);
                    let _ = event_tx.send(AgentLoopEvent::Thinking { text: thinking }).await;
                }
                StreamDelta::Text { text } => {
                    accumulated_text.push_str(&text);
                    let _ = event_tx.send(AgentLoopEvent::TextDelta { text }).await;
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
                    // 保持单次请求的真实用量（含缓存命中），避免用累加值把统计撑高
                    step_usage = Some(usage);
                }
                StreamDelta::Done { stop_reason } => {
                    turn_stop_reason = stop_reason;
                }
                StreamDelta::Error { error } => {
                    had_error = true;
                    let _ = event_tx.send(AgentLoopEvent::Error { message: error.clone() }).await;
                    warn!("大模型流式异常: {}", error);
                    break;
                }
            }
        }

        if had_error {
            break;
        }

        // 单次调用的耗时与用量：usage 以模型返回为准，整轮耗时含此前所有工具执行
        let step_duration_ms = (now_ms() - step_start).max(1) as u64;
        let turn_duration_ms = (now_ms() - turn_start).max(1) as u64;
        if step_usage.is_none() && accumulated_usage.total_tokens > 0 {
            step_usage = Some(accumulated_usage);
        }

        if turn_stop_reason == "aborted" {
            let _ = event_tx
                .send(AgentLoopEvent::TurnFinished {
                    stop_reason: "aborted".to_string(),
                })
                .await;
            break;
        }

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
            stop_reason: Some(turn_stop_reason.clone()),
            error_message: None,
            timestamp: Some(now_ms()),
            usage: step_usage.as_ref().and_then(|u| serde_json::to_value(u).ok()),
            duration_ms: Some(step_duration_ms),
            turn_duration_ms: Some(turn_duration_ms),
        };

        session_mgr.append_message(thread_id, assistant_msg.clone(), Some(&ws_str))?;
        history_messages.push(assistant_msg);

        // 用量与耗时上报给界面（此刻本步的助手卡片还是"流式中"，正好挂上去）
        let _ = event_tx
            .send(AgentLoopEvent::AssistantStats {
                usage: step_usage,
                duration_ms: step_duration_ms,
                turn_duration_ms,
            })
            .await;

        // 如果没有工具调用，说明模型已输出最终回答，本轮自然结束
        if tool_calls.is_empty() {
            let _ = event_tx
                .send(AgentLoopEvent::TurnFinished {
                    stop_reason: "completed".to_string(),
                })
                .await;
            break;
        }

        // 执行工具调用
        let mut loop_aborted = false;
        for call in &tool_calls {
            if let Some(ref rx) = abort_rx {
                if *rx.borrow() {
                    let _ = event_tx
                        .send(AgentLoopEvent::TurnFinished {
                            stop_reason: "aborted".to_string(),
                        })
                        .await;
                    loop_aborted = true;
                    break;
                }
            }

            let _ = event_tx
                .send(AgentLoopEvent::ToolCallStarted {
                    name: call.name.clone(),
                    id: call.id.clone(),
                    args: call.args.clone(),
                })
                .await;

            let result_msg = execute_tool_call_extended(
                workspace,
                thread_id,
                call,
                Some(&checkpoint_mgr),
                Some(&provider_config),
                None,
                Some(&event_tx),
                abort_rx.as_ref(),
            )
            .await;

            let (is_ok, output_str, duration_ms, started_at, finished_at, status_val) = match &result_msg {
                AgentMessage::ToolResult {
                    is_error,
                    content,
                    duration_ms,
                    started_at,
                    finished_at,
                    status,
                    ..
                } => {
                    let ok = is_error.is_none() || is_error == &Some(false);
                    (
                        ok,
                        Some(content.clone()),
                        *duration_ms,
                        *started_at,
                        *finished_at,
                        status.clone(),
                    )
                }
                _ => (true, None, None, None, None, None),
            };

            let _ = event_tx
                .send(AgentLoopEvent::ToolCallFinished {
                    name: call.name.clone(),
                    id: call.id.clone(),
                    ok: is_ok,
                    output: output_str,
                    duration_ms,
                    started_at,
                    finished_at,
                    status: status_val,
                })
                .await;

            session_mgr.append_message(thread_id, result_msg.clone(), Some(&ws_str))?;
            history_messages.push(result_msg);
        }

        if loop_aborted {
            break;
        }
    }

    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::runner::executor::execute_tool_call;
    use super::*;
    use crate::ai::ToolCallInfo;

    #[tokio::test]
    async fn test_executor_read_and_write() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_runner_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let call_write = ToolCallInfo {
            id: "call_01".to_string(),
            name: "write_file".to_string(),
            args: serde_json::json!({
                "path": "test.txt",
                "content": "Hello Runner"
            }).to_string(),
        };

        let result_write = execute_tool_call(&temp_dir, "thread_01", &call_write, None).await;
        match result_write {
            AgentMessage::ToolResult {
                content,
                is_error,
                duration_ms,
                started_at,
                finished_at,
                status,
                ..
            } => {
                assert!(is_error.is_none() || is_error == Some(false));
                assert_eq!(status.as_deref(), Some("success"));
                assert!(duration_ms.is_some());
                assert!(started_at.is_some());
                assert!(finished_at.is_some());

                // 验证 content 返回合法结构化 JSON 并包含所有要求字段
                let parsed: serde_json::Value = serde_json::from_str(&content).expect("工具返回必须为结构化 JSON");
                assert_eq!(parsed["status"], "success");
                assert_eq!(parsed["ok"], true);
                assert!(parsed["duration_ms"].as_u64().is_some());
                assert!(parsed["started_at"].as_i64().is_some());
                assert!(parsed["finished_at"].as_i64().is_some());
                assert!(parsed["output"].as_str().unwrap().contains("已写入"));
            }
            _ => panic!("返回类型必须为 ToolResult"),
        }

        let call_read = ToolCallInfo {
            id: "call_02".to_string(),
            name: "read_file".to_string(),
            args: serde_json::json!({
                "path": "test.txt"
            }).to_string(),
        };

        let result_read = execute_tool_call(&temp_dir, "thread_01", &call_read, None).await;
        match result_read {
            AgentMessage::ToolResult { content, duration_ms, status, .. } => {
                assert_eq!(status.as_deref(), Some("success"));
                assert!(duration_ms.is_some());
                let parsed: serde_json::Value = serde_json::from_str(&content).expect("工具返回必须为结构化 JSON");
                assert_eq!(parsed["status"], "success");
                assert!(parsed["output"].as_str().unwrap().contains("1 | Hello Runner"));
            }
            _ => panic!("返回类型必须为 ToolResult"),
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

