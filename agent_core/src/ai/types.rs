use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamDelta {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_call")]
    ToolCall { call: ToolCallInfo },
    #[serde(rename = "usage")]
    Usage { usage: TokenUsage },
    #[serde(rename = "done")]
    Done {
        #[serde(rename = "stopReason")]
        stop_reason: String, // "stop" | "tool_calls" | "length" | "error" | "aborted"
    },
    #[serde(rename = "error")]
    Error { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCompletionToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCompletionTool {
    #[serde(rename = "type")]
    pub tool_type: String, // 恒为 "function"
    pub function: ChatCompletionToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelChatOptions {
    pub tools: Option<Vec<ChatCompletionTool>>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub effort: Option<String>,
    pub max_retries: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completion_message_openai_compatibility() {
        let tool_msg = ChatCompletionMessage {
            role: "tool".to_string(),
            content: Some("ok".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
        };
        let json = serde_json::to_string(&tool_msg).unwrap();
        // 关键断言：必须是 tool_call_id 而非 toolCallId
        assert!(json.contains("\"tool_call_id\":\"call_123\""));
        assert!(!json.contains("toolCallId"));

        let asst_msg = ChatCompletionMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(vec![serde_json::json!({ "id": "call_123" })]),
            tool_call_id: None,
        };
        let json_asst = serde_json::to_string(&asst_msg).unwrap();
        // 关键断言：必须是 tool_calls 而非 toolCalls
        assert!(json_asst.contains("\"tool_calls\":["));
        assert!(!json_asst.contains("toolCalls"));
    }
}
