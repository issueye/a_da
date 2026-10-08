//! 会话**落盘格式**（不是领域模型）。
//!
//! 领域模型 [`AgentMessage`] / [`ToolCallBlock`] 已按设计搬到 `agent-base::domain`
//! （`docs/agent-base-plan.md` §8.2）；这里只留"会话文件长什么样"：
//! `SessionHeader` / `SessionEntry*` / `SessionSummary`。
//! 下一步（M1）这些类型连同 `SessionManager` 一起搬进 `agent-adapter` 的 store 适配器。

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub use agent_base::domain::{AgentMessage, ToolCallBlock};

pub const CURRENT_SESSION_VERSION: u32 = 1;

fn default_session_type() -> String { "session".to_string() }
fn default_message_type() -> String { "message".to_string() }
fn default_notice_type() -> String { "notice".to_string() }
fn default_compact_type() -> String { "compact".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    #[serde(rename = "type", default = "default_session_type")]
    pub entry_type: String, // 恒为 "session"
    pub version: u32,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub workspace: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_data: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageEntry {
    #[serde(rename = "type", default = "default_message_type")]
    pub entry_type: String, // 恒为 "message"
    pub id: String,
    pub timestamp: i64,
    pub message: AgentMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionNoticeEntry {
    #[serde(rename = "type", default = "default_notice_type")]
    pub entry_type: String, // 恒为 "notice"
    pub id: String,
    pub timestamp: i64,
    pub text: String,
    pub level: String, // "info" | "error"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactEntry {
    #[serde(rename = "type", default = "default_compact_type")]
    pub entry_type: String, // 恒为 "compact"
    pub id: String,
    pub timestamp: i64,
    pub summary: String,
    #[serde(default)]
    pub pre_tokens: u64,
    #[serde(default)]
    pub post_tokens: u64,
    #[serde(default)]
    pub saved_tokens: u64,
    #[serde(default)]
    pub turns_summarized: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "session")]
    Header(SessionHeader),
    #[serde(rename = "message")]
    Message(SessionMessageEntry),
    #[serde(rename = "notice")]
    Notice(SessionNoticeEntry),
    #[serde(rename = "compact")]
    Compact(SessionCompactEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
}
