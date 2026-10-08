//! 会话消息：**领域模型**（不是落盘格式）。
//!
//! 从 `agent_core::session::types` 拆出来：那里同时装着"消息是什么"（本文件）与
//! "会话文件长什么样"（`SessionHeader` / `SessionEntry*`，留在 store 适配器）。
//!
//! 兼容性由 serde 承担（INV-9）：写出一律驼峰（与 TS 版互通），同时接受早期 Rust 版写下的蛇形键。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallBlock {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    #[serde(default)]
    pub raw_arguments: String,
}

fn deserialize_string_or_blocks<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    match val {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(txt.to_string());
                }
            }
            Ok(parts.join("\n"))
        }
        serde_json::Value::Null => Ok(String::new()),
        other => Ok(other.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User {
        #[serde(deserialize_with = "deserialize_string_or_blocks")]
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<i64>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        // 落盘统一驼峰（与 TS 版会话文件互通）；同时接受早期 Rust 版写下的蛇形键
        #[serde(alias = "tool_calls", skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCallBlock>>,
        #[serde(alias = "stop_reason", skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        #[serde(alias = "error_message", skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<serde_json::Value>,
        #[serde(alias = "duration_ms", skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(alias = "turn_duration_ms", skip_serializing_if = "Option::is_none")]
        turn_duration_ms: Option<u64>,
    },
    #[serde(rename = "toolResult")]
    ToolResult {
        #[serde(alias = "tool_call_id")]
        tool_call_id: String,
        #[serde(alias = "tool_name")]
        tool_name: String,
        content: String,
        #[serde(alias = "is_error", skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        patch: Option<String>,
        #[serde(alias = "checkpoint_id", skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(alias = "duration_ms", rename = "durationMs", skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(alias = "started_at", rename = "startedAt", skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        #[serde(alias = "finished_at", rename = "finishedAt", skip_serializing_if = "Option::is_none")]
        finished_at: Option<i64>,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TS 版（原始实现）写下的会话行：字段全驼峰
    const TS_ASSISTANT: &str = r#"{"role":"assistant","content":"答案","thinking":"推理","toolCalls":[{"id":"call_1","name":"run_command","arguments":{"command":"ls"},"rawArguments":"{\"command\":\"ls\"}"}],"stopReason":"tool_calls","usage":{"promptTokens":100,"completionTokens":5,"totalTokens":105,"cachedTokens":80},"durationMs":1200,"turnDurationMs":3400,"timestamp":1700000000000}"#;
    const TS_TOOL_RESULT: &str = r#"{"role":"toolResult","toolCallId":"call_1","toolName":"run_command","content":"ok","isError":false,"timestamp":1700000000001}"#;

    /// 早期 Rust 版写下的会话行：字段全蛇形，必须继续读得进来
    const LEGACY_ASSISTANT: &str = r#"{"role":"assistant","content":"答案","tool_calls":[{"id":"call_1","name":"run_command","arguments":{"command":"ls"},"rawArguments":"{}"}],"duration_ms":1200,"turn_duration_ms":3400}"#;
    const LEGACY_TOOL_RESULT: &str = r#"{"role":"toolResult","tool_call_id":"call_1","tool_name":"run_command","content":"ok","is_error":false}"#;

    #[test]
    fn test_agent_message_reads_typescript_keys() {
        match serde_json::from_str::<AgentMessage>(TS_ASSISTANT).expect("驼峰助手行解析失败") {
            AgentMessage::Assistant { content, tool_calls, usage, duration_ms, turn_duration_ms, .. } => {
                assert_eq!(content, "答案");
                assert_eq!(tool_calls.as_ref().map(|c| c.len()), Some(1));
                assert_eq!(tool_calls.unwrap()[0].raw_arguments, "{\"command\":\"ls\"}");
                assert_eq!(usage.unwrap()["cachedTokens"], 80);
                assert_eq!(duration_ms, Some(1200));
                assert_eq!(turn_duration_ms, Some(3400));
            }
            other => panic!("应解析为 Assistant，实际 {other:?}"),
        }

        match serde_json::from_str::<AgentMessage>(TS_TOOL_RESULT).expect("驼峰工具结果行解析失败") {
            AgentMessage::ToolResult { tool_call_id, tool_name, is_error, .. } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "run_command");
                assert_eq!(is_error, Some(false));
            }
            other => panic!("应解析为 ToolResult，实际 {other:?}"),
        }
    }

    #[test]
    fn test_agent_message_reads_legacy_snake_keys() {
        match serde_json::from_str::<AgentMessage>(LEGACY_ASSISTANT).expect("蛇形助手行解析失败") {
            AgentMessage::Assistant { tool_calls, duration_ms, .. } => {
                assert_eq!(tool_calls.as_ref().map(|c| c.len()), Some(1));
                assert_eq!(duration_ms, Some(1200));
            }
            other => panic!("应解析为 Assistant，实际 {other:?}"),
        }

        match serde_json::from_str::<AgentMessage>(LEGACY_TOOL_RESULT).expect("蛇形工具结果行解析失败") {
            AgentMessage::ToolResult { tool_call_id, tool_name, .. } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "run_command");
            }
            other => panic!("应解析为 ToolResult，实际 {other:?}"),
        }
    }

    #[test]
    fn test_agent_message_writes_camel_case() {
        let msg = AgentMessage::ToolResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "run_command".to_string(),
            content: "ok".to_string(),
            is_error: Some(false),
            details: None,
            patch: None,
            checkpoint_id: None,
            timestamp: Some(1),
            status: Some("success".to_string()),
            duration_ms: Some(10),
            started_at: Some(0),
            finished_at: Some(10),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"toolCallId\""), "落盘需与 TS 版兼容：{json}");
        assert!(json.contains("\"toolName\""), "落盘需与 TS 版兼容：{json}");
        assert!(!json.contains("tool_call_id"), "不应再写出蛇形键：{json}");
    }
}
