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
            reasoning_content: None,
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
                    reasoning_content: None,
                });
            }
            AgentMessage::Assistant {
                content,
                thinking,
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
                    // thinking 模式：上游要求把上一轮思考链原样带回，否则多轮工具调用直接 400
                    reasoning_content: thinking
                        .clone()
                        .filter(|t| !t.trim().is_empty()),
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
                    reasoning_content: None,
                });
            }
            AgentMessage::Unknown => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ToolCallBlock;

    fn assistant(content: &str, thinking: Option<&str>) -> AgentMessage {
        AgentMessage::Assistant {
            content: content.to_string(),
            thinking: thinking.map(|t| t.to_string()),
            tool_calls: Some(vec![ToolCallBlock {
                id: "call_1".to_string(),
                name: "run_command".to_string(),
                arguments: json!({ "command": "ls" }),
                raw_arguments: r#"{"command":"ls"}"#.to_string(),
            }]),
            stop_reason: Some("tool_calls".to_string()),
            error_message: None,
            timestamp: None,
            usage: None,
            duration_ms: None,
            turn_duration_ms: None,
        }
    }

    /// 回归：thinking 模式的上游（HTTP 400「reasoning_content must be passed back」）
    /// 要求助手消息把思考链原样带回去。
    #[test]
    fn test_assistant_thinking_is_echoed_as_reasoning_content() {
        let history = vec![
            AgentMessage::User {
                content: "查一下 git 状态".to_string(),
                images: None,
                timestamp: None,
            },
            assistant("", Some("先跑 git_status")),
            AgentMessage::ToolResult {
                tool_call_id: "call_1".to_string(),
                tool_name: "run_command".to_string(),
                content: "{}".to_string(),
                is_error: Some(false),
                details: None,
                patch: None,
                checkpoint_id: None,
                timestamp: None,
                status: Some("success".to_string()),
                duration_ms: None,
                started_at: None,
                finished_at: None,
            },
        ];

        let formatted = format_messages_for_model("sys", &history);
        let asst = &formatted[2];
        assert_eq!(asst.role, "assistant");
        assert_eq!(asst.content, None, "空正文 + 工具调用时不应下发空 content");
        assert_eq!(asst.reasoning_content.as_deref(), Some("先跑 git_status"));

        // 其余角色不得携带思考链
        assert_eq!(formatted[0].reasoning_content, None);
        assert_eq!(formatted[1].reasoning_content, None);
        assert_eq!(formatted[3].reasoning_content, None);

        let json = serde_json::to_string(asst).unwrap();
        assert!(json.contains(r#""reasoning_content":"先跑 git_status""#), "{json}");
    }

    /// 没有思考链（非 thinking 模型 / 该轮未吐思考）时不下发字段，避免部分供应商报错。
    #[test]
    fn test_assistant_without_thinking_omits_reasoning_content() {
        let history = vec![assistant("直接回答", None)];
        let formatted = format_messages_for_model("", &history);
        assert_eq!(formatted[0].reasoning_content, None);
        assert!(!serde_json::to_string(&formatted[0]).unwrap().contains("reasoning_content"));

        // 空白思考链同样视为没有
        let blank = vec![assistant("直接回答", Some("   \n "))];
        let formatted_blank = format_messages_for_model("", &blank);
        assert_eq!(formatted_blank[0].reasoning_content, None);
    }
}
