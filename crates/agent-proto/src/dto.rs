use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

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

/// 服务端事件能力位细项
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerEventsCapability {
    pub granularity: String,
    pub snapshot_seq: bool,
}

/// 服务端声明的能力位集合（INV-10 / M3-T4：如实回答，绝不捏造）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub images: bool,
    pub rollback: bool,
    pub plugins: bool,
    pub plugin_events: bool,
    pub resync: bool,
    pub events: ServerEventsCapability,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            images: false, // MVP 如实声明不支持图片，绝不做半吊子
            rollback: true,
            plugins: true,
            plugin_events: false,
            resync: false,
            events: ServerEventsCapability {
                granularity: "coarse".to_string(),
                snapshot_seq: true,
            },
        }
    }
}

/// 宿主进程信息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub pid: u32,
}

/// 产品元数据（支持多产品架构体系）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProductInfo {
    pub id: String,
    pub name: String,
    pub archetype: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
}

/// 客户端初始化握手响应负载
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub session_id: String,
    pub protocol_version: String,
    pub host: HostInfo,
    pub capabilities: ServerCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<ProductInfo>,
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
    Pm,
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
        /// 本次请求模型真正返回的用量（驼峰：promptTokens/completionTokens/totalTokens/cachedTokens）。
        /// 界面遥测条与单条回复的 Token 徽章只认它，不做事后估算。
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<agent_base::model::TokenUsage>,
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
        #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        #[serde(rename = "finishedAt", skip_serializing_if = "Option::is_none")]
        finished_at: Option<i64>,
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
    /// 卡片列表。用 `Arc` 是为了让快照/增量帧**共享**卡片而不是整份 clone：
    /// 长会话的单帧序列化曾经是主要卡顿来源（几百张卡、每张都带文本/工具输出）。
    /// 线上形状与 `Vec<Item>` 完全一致（serde 的 `rc` 特性透明序列化 `Arc<T>`）。
    pub items: Vec<Arc<Item>>,
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentMode>,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "subagentId", skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(rename = "isSubagent", skip_serializing_if = "Option::is_none")]
    pub is_subagent: Option<bool>,
    #[serde(rename = "pluginData", skip_serializing_if = "Option::is_none")]
    pub plugin_data: Option<HashMap<String, serde_json::Value>>,
    /// 会话的条目总数（**不受快照裁剪影响**）。
    ///
    /// 为什么单独给一份：`/api/snapshot` 与 `evt.state.patch` 为了压帧体积，只给
    /// "激活会话 / 打开的 Tab / 运行中 / 子智能体"这几种线程带 `items`，其余线程的
    /// `items` 是空数组（见 `agent_rpc::state::snapshot`）。界面若拿 `items.len()`
    /// 当"这个会话有没有内容"的判据，整屏历史会话都会被误判成空会话——
    /// 实测侧栏只剩当前会话，用户以为历史丢了。
    #[serde(rename = "itemCount", default)]
    pub item_count: usize,
    /// 最近活动时刻（毫秒）：最后一条 item 的 `at`，空会话回退 `createdAt`。
    ///
    /// 与 `itemCount` 同一个原因：`items` 被裁剪时，界面仍要能按"最近活动"排序、
    /// 显示"3 分钟前"，否则时间会全部退化成创建时刻（列表看起来像随机序）。
    #[serde(rename = "updatedAt", default)]
    pub updated_at: u64,
}

impl Item {
    /// 卡片的时刻戳（六个变体都带 `at`，但没有公共字段）。
    ///
    /// 全仓唯一实现：会话"最近活动"、条目的单调顺延（`next_item_at`）都用它，
    /// 免得每个变体各写一份 `match`。
    pub fn at(&self) -> u64 {
        match self {
            Item::User { at, .. }
            | Item::Thinking { at, .. }
            | Item::Assistant { at, .. }
            | Item::Tool { at, .. }
            | Item::Notice { at, .. }
            | Item::Compact { at, .. } => *at,
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub model: String,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(rename = "supportsImages")]
    pub supports_images: bool,
    pub effort: Effort,
    pub mode: AgentMode,
    /// 模型请求**重试次数**（首次失败后最多再试几次；默认见
    /// [`agent_base::model::DEFAULT_MODEL_RETRY_COUNT`]）。
    ///
    /// 重试耗尽即断开本轮对话（不再消费排队指令），由界面提示用户手动重试。
    /// 老配置文件没有这个字段时按默认值回填（`serde(default)`），不是 0。
    #[serde(rename = "retryCount", default = "default_retry_count")]
    pub retry_count: u32,
}

fn default_retry_count() -> u32 {
    agent_base::model::DEFAULT_MODEL_RETRY_COUNT as u32
}

impl Default for ConfigSnapshot {
    fn default() -> Self {
        Self {
            model: String::new(),
            context_window: 0,
            max_output_tokens: None,
            supports_images: false,
            effort: Effort::default(),
            mode: AgentMode::default(),
            retry_count: default_retry_count(),
        }
    }
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
    #[serde(default)]
    pub seq: u64,
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
    #[serde(rename = "publicWorkspace")]
    pub public_workspace: String,
    pub appearance: String,
    pub ui: UiSnapshot,
    #[serde(default)]
    pub providers: Vec<agent_base::model::ProviderEntry>,
    #[serde(rename = "activeProviderId", default)]
    pub active_provider_id: String,
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
    #[serde(default)]
    pub dirs: Vec<FsEntry>,
    #[serde(default)]
    pub files: Vec<FsEntry>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `Thread.items: Vec<Arc<Item>>` 的线上形状必须与 `Vec<Item>` **完全一致**：
    /// 客户端（SSR / 旧客户端）看到的仍是普通数组，任何 `Arc` 痕迹都不许漏出去。
    #[test]
    fn test_arc_items_serialize_exactly_like_plain_items() {
        let items = vec![
            Item::User {
                id: "u1".to_string(),
                at: 1,
                text: "你好".to_string(),
                images: None,
                queued: None,
            },
            Item::Assistant {
                id: "a1".to_string(),
                at: 2,
                text: "收到".to_string(),
                streaming: None,
                duration_ms: None,
                turn_duration_ms: None,
                usage: None,
            },
        ];
        let plain = serde_json::to_value(&items).expect("普通 items 序列化");
        let shared: Vec<Arc<Item>> = items.iter().map(|item| Arc::new(item.clone())).collect();
        let wrapped = serde_json::to_value(&shared).expect("Arc items 序列化");
        assert_eq!(plain, wrapped, "Arc 只是内存共享，不许改变线上形状");

        // 反序列化同一条：serde 的 rc 特性让 `Vec<Arc<Item>>` 可直接收下普通数组
        let back: Vec<Arc<Item>> = serde_json::from_value(plain).expect("反序列化 Arc items");
        assert_eq!(back.len(), 2);
        assert!(matches!(back[0].as_ref(), Item::User { text, .. } if text == "你好"));
    }
}
