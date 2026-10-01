use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JSON-RPC 2.0 请求/通知帧
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 响应帧
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<super::errors::ProtocolError>,
}

impl<T: Serialize> JsonRpcResponse<T> {
    pub fn success(id: Option<serde_json::Value>, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, error: super::errors::ProtocolError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// 服务端推送事件通知帧
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification<T> {
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

impl<T: Serialize> JsonRpcNotification<T> {
    pub fn new(method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// 快照事件负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEvent {
    pub seq: u64,
    pub topic: String,
    pub payload: ClientSnapshot,
}

/// 协作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Code,
    Plan,
    Create,
}

/// 审批模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    #[default]
    Auto,
    Ask,
    Readonly,
}

/// 深度档位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    #[default]
    Max,
    High,
    Medium,
    Low,
}

/// 待办与提问
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentQuestion {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<ChoiceOption>>,
    #[serde(rename = "allowText", skip_serializing_if = "Option::is_none")]
    pub allow_text: Option<bool>,
    pub status: String,
    #[serde(rename = "askedAt")]
    pub asked_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingQuestionEntry {
    #[serde(rename = "callId")]
    pub call_id: String,
    pub question: AgentQuestion,
}

/// 界面卡片 Item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Item {
    #[serde(rename = "user")]
    User {
        id: String,
        at: u64,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queued: Option<bool>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        id: String,
        at: u64,
        text: String,
        #[serde(rename = "endedAt", skip_serializing_if = "Option::is_none")]
        ended_at: Option<u64>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        id: String,
        at: u64,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming: Option<bool>,
        #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(rename = "turnDurationMs", skip_serializing_if = "Option::is_none")]
        turn_duration_ms: Option<u64>,
    },
    #[serde(rename = "tool")]
    Tool {
        id: String,
        at: u64,
        #[serde(rename = "callId")]
        call_id: String,
        name: String,
        args: serde_json::Value,
        #[serde(rename = "rawArgs")]
        raw_args: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        patch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(rename = "threadId", skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(rename = "checkpointId", skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reverted: Option<bool>,
    },
    #[serde(rename = "notice")]
    Notice {
        id: String,
        at: u64,
        text: String,
        level: String,
    },
    #[serde(rename = "compact")]
    Compact {
        id: String,
        at: u64,
        summary: String,
        #[serde(rename = "preTokens")]
        pre_tokens: u64,
        #[serde(rename = "postTokens")]
        post_tokens: u64,
        #[serde(rename = "savedTokens")]
        saved_tokens: u64,
        #[serde(rename = "turnsSummarized")]
        turns_summarized: u64,
    },
}

/// 会话数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub title: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    pub workspace: String,
    pub items: Vec<Item>,
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentMode>,
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "subagentId", skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(rename = "isSubagent", skip_serializing_if = "Option::is_none")]
    pub is_subagent: Option<bool>,
    #[serde(rename = "pluginData", skip_serializing_if = "Option::is_none")]
    pub plugin_data: Option<HashMap<String, serde_json::Value>>,
}

/// 调试条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugEntry {
    pub id: u64,
    pub at: u64,
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// 排队待发指令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedItem {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub text: String,
    #[serde(rename = "enqueuedAt")]
    pub enqueued_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

/// 工作区信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceSnapshot {
    pub project: String,
    pub files: usize,
    pub dirs: usize,
    pub scanning: bool,
    pub entries: Vec<String>,
}

/// 配置信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigSnapshot {
    pub model: String,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "supportsImages")]
    pub supports_images: bool,
    pub approval: ApprovalMode,
    pub effort: Effort,
    pub mode: AgentMode,
}

/// UI 外壳镜像
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiSnapshot {
    #[serde(rename = "activeId")]
    pub active_id: String,
    #[serde(rename = "openTabIds")]
    pub open_tab_ids: Vec<String>,
    #[serde(rename = "pendingDraft")]
    pub pending_draft: Option<String>,
    #[serde(rename = "debugOpen")]
    pub debug_open: bool,
    #[serde(rename = "settingsOpen")]
    pub settings_open: bool,
    #[serde(rename = "pluginsOpen")]
    pub plugins_open: bool,
    #[serde(rename = "changesOpen")]
    pub changes_open: bool,
    #[serde(rename = "paletteOpen")]
    pub palette_open: bool,
    #[serde(rename = "sidebarOpen")]
    pub sidebar_open: bool,
    #[serde(rename = "searchOpen")]
    pub search_open: bool,
}

/// 客户端全量快照 (ClientSnapshot)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSnapshot {
    pub threads: Vec<Thread>,
    #[serde(rename = "activeThreadId")]
    pub active_thread_id: String,
    #[serde(rename = "runningThreadIds")]
    pub running_thread_ids: Vec<String>,
    #[serde(rename = "waitingThreadIds")]
    pub waiting_thread_ids: Vec<String>,
    pub queue: Vec<QueuedItem>,
    pub log: Vec<DebugEntry>,
    pub workspace: WorkspaceSnapshot,
    pub config: ConfigSnapshot,
    #[serde(rename = "pendingQuestions")]
    pub pending_questions: Vec<PendingQuestionEntry>,
    #[serde(rename = "publicWorkspace")]
    pub public_workspace: String,
    pub appearance: String,
    pub ui: UiSnapshot,
}

/// 文件树列表条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListing {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    pub truncated: bool,
    pub omitted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsRoot {
    pub label: String,
    pub path: String,
    #[serde(rename = "kind")]
    pub kind: String,
}
