use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tracing::warn;


use super::executor::execute_tool_call;
use super::prompt::{build_system_prompt, builtin_tools, format_messages_for_model};
use crate::ai::{stream_model_chat, ModelChatOptions, ProviderConfig, StreamDelta};
use crate::checkpoint::CheckpointManager;
use crate::session::{AgentMessage, SessionManager, ToolCallBlock};

const MAX_LOOP_STEPS: usize = 30;

#[derive(Debug, Clone)]
pub enum AgentLoopEvent {
    Thinking { text: String },
    TextDelta { text: String },
    ToolCallStarted { name: String, id: String, args: String },
    ToolCallFinished { name: String, id: String, ok: bool, output: Option<String> },
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
    let tools = builtin_tools();

    // 3. 多轮驱动
    for _step in 0..MAX_LOOP_STEPS {
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
                StreamDelta::Usage { .. } => {}
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
            usage: None,
            duration_ms: None,
            turn_duration_ms: None,
        };

        session_mgr.append_message(thread_id, assistant_msg.clone(), Some(&ws_str))?;
        history_messages.push(assistant_msg);

        // 如果没有工具调用，本轮结束
        if tool_calls.is_empty() {
            let _ = event_tx
                .send(AgentLoopEvent::TurnFinished {
                    stop_reason: "completed".to_string(),
                })
                .await;
            break;
        }

        // 执行工具调用
        for call in &tool_calls {
            let _ = event_tx
                .send(AgentLoopEvent::ToolCallStarted {
                    name: call.name.clone(),
                    id: call.id.clone(),
                    args: call.args.clone(),
                })
                .await;

            let result_msg = execute_tool_call(
                workspace,
                thread_id,
                call,
                Some(&checkpoint_mgr),
            )
            .await;

            let (is_ok, output_str) = match &result_msg {
                AgentMessage::ToolResult { is_error, content, .. } => {
                    let ok = is_error.is_none() || is_error == &Some(false);
                    (ok, Some(content.clone()))
                }
                _ => (true, None),
            };

            let _ = event_tx
                .send(AgentLoopEvent::ToolCallFinished {
                    name: call.name.clone(),
                    id: call.id.clone(),
                    ok: is_ok,
                    output: output_str,
                })
                .await;

            session_mgr.append_message(thread_id, result_msg.clone(), Some(&ws_str))?;
            history_messages.push(result_msg);
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
            AgentMessage::ToolResult { content, is_error, .. } => {
                assert!(is_error.is_none() || is_error == Some(false));
                assert!(content.contains("已写入"));
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
            AgentMessage::ToolResult { content, .. } => {
                assert!(content.contains("1 | Hello Runner"));
            }
            _ => panic!("返回类型必须为 ToolResult"),
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

