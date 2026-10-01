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
#[serde(rename_all = "camelCase")]
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
