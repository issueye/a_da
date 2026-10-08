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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelProtocol {
    #[serde(rename = "openai_chat")]
    OpenAiChat,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
}

impl Default for ModelProtocol {
    fn default() -> Self {
        ModelProtocol::OpenAiChat
    }
}

impl ModelProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelProtocol::OpenAiChat => "openai_chat",
            ModelProtocol::Anthropic => "anthropic",
            ModelProtocol::OpenAiResponses => "openai_responses",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "anthropic" | "claude" => ModelProtocol::Anthropic,
            "openai_responses" | "responses" | "openai-responses" => ModelProtocol::OpenAiResponses,
            _ => ModelProtocol::OpenAiChat,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: ModelProtocol,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_headers: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: ModelProtocol,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_headers: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
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
    /// 思考链。thinking 模式的上游（如 DeepSeek 系 / 部分 OpenAI 兼容网关）要求把模型上一轮
    /// 返回的 `reasoning_content` **原样回传**，否则多轮工具调用会以
    /// HTTP 400「The `reasoning_content` in the thinking mode must be passed back to the API」失败。
    /// 无思考链时（非 thinking 模型、或该轮没吐思考）保持缺省，不下发空字符串。
    #[serde(
        rename = "reasoning_content",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_content: Option<String>,
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
            reasoning_content: None,
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
            reasoning_content: None,
        };
        let json_asst = serde_json::to_string(&asst_msg).unwrap();
        // 关键断言：必须是 tool_calls 而非 toolCalls
        assert!(json_asst.contains("\"tool_calls\":["));
        assert!(!json_asst.contains("toolCalls"));
    }

    /// thinking 模式的上游要求历史里的助手消息原样带回 reasoning_content；
    /// 没有思考链时不得下发该字段（部分供应商一见空串就报错）。
    #[test]
    fn test_chat_completion_message_carries_reasoning_content() {
        let with_thinking = ChatCompletionMessage {
            role: "assistant".to_string(),
            content: Some("答案".to_string()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: Some("先看用户要什么".to_string()),
        };
        let json = serde_json::to_string(&with_thinking).unwrap();
        assert!(
            json.contains("\"reasoning_content\":\"先看用户要什么\""),
            "回传思考链必须是蛇形键 reasoning_content：{json}"
        );
        assert!(!json.contains("reasoningContent"));

        let without = ChatCompletionMessage {
            role: "assistant".to_string(),
            content: Some("答案".to_string()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        let json = serde_json::to_string(&without).unwrap();
        assert!(!json.contains("reasoning_content"), "无思考链时不应下发该字段：{json}");

        // 兼容：老请求体里带 reasoning_content 也能读进来（缺省时是 None）
        let parsed: ChatCompletionMessage =
            serde_json::from_str(r#"{"role":"assistant","content":"x","reasoning_content":"y"}"#).unwrap();
        assert_eq!(parsed.reasoning_content.as_deref(), Some("y"));
        let parsed_none: ChatCompletionMessage =
            serde_json::from_str(r#"{"role":"assistant","content":"x"}"#).unwrap();
        assert_eq!(parsed_none.reasoning_content, None);
    }
}
