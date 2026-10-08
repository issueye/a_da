//! 将会话流水消息转换为大模型补全请求序列。

use serde_json::json;

use crate::domain::AgentMessage;
use crate::model::ChatCompletionMessage;

/// 将会话历史与系统提示词格式化为模型可接受的消息列表。
pub fn format_messages_for_model(
    system_prompt: &str,
    history: &[AgentMessage],
) -> Vec<ChatCompletionMessage> {
    let mut out = Vec::new();

    if !system_prompt.trim().is_empty() {
        out.push(ChatCompletionMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in history {
        match msg {
            AgentMessage::User { content, .. } => {
                out.push(ChatCompletionMessage {
                    role: "user".to_string(),
                    content: Some(content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            AgentMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let tc_json = tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    "arguments": if !c.raw_arguments.is_empty() {
                                        c.raw_arguments.clone()
                                    } else {
                                        c.arguments.to_string()
                                    }
                                }
                            })
                        })
                        .collect()
                });

                out.push(ChatCompletionMessage {
                    role: "assistant".to_string(),
                    content: if content.is_empty() && tc_json.is_some() {
                        None
                    } else {
                        Some(content.clone())
                    },
                    tool_calls: tc_json,
                    tool_call_id: None,
                });
            }
            AgentMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                out.push(ChatCompletionMessage {
                    role: "tool".to_string(),
                    content: Some(content.clone()),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id.clone()),
                });
            }
            AgentMessage::Unknown => {}
        }
    }

    out
}
