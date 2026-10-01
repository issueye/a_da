use crate::protocol::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_id(prefix: &str) -> String {
    let count = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", prefix, now_millis(), count)
}

#[derive(Debug, Clone)]
pub struct AgentStore {
    pub threads: Vec<Thread>,
    pub active_id: String,
    pub running_thread_ids: Vec<String>,
    pub waiting_thread_ids: Vec<String>,
    pub queue: Vec<QueuedItem>,
    pub log: Vec<DebugEntry>,
    pub workspace: WorkspaceSnapshot,
    pub config: ConfigSnapshot,
    pub provider: crate::ai::ProviderConfig,
    pub pending_questions: Vec<PendingQuestionEntry>,
    pub public_workspace: String,
    pub appearance: String,
    pub ui: UiSnapshot,
}


impl Default for AgentStore {
    fn default() -> Self {
        Self::new("".to_string())
    }
}

impl AgentStore {
    pub fn new(workspace_path: String) -> Self {
        let initial_thread_id = next_id("thread");
        let initial_thread = Thread {
            id: initial_thread_id.clone(),
            title: "新会话".to_string(),
            created_at: now_millis(),
            workspace: workspace_path.clone(),
            items: Vec::new(),
            messages: Vec::new(),
            mode: Some(AgentMode::Code),
            parent_id: None,
            subagent_id: None,
            is_subagent: Some(false),
            plugin_data: None,
        };

        Self {
            threads: vec![initial_thread],
            active_id: initial_thread_id.clone(),
            running_thread_ids: Vec::new(),
            waiting_thread_ids: Vec::new(),
            queue: Vec::new(),
            log: Vec::new(),
            workspace: WorkspaceSnapshot {
                project: workspace_path.clone(),
                files: 0,
                dirs: 0,
                scanning: false,
                entries: Vec::new(),
            },
            config: ConfigSnapshot {
                model: "".to_string(),
                context_window: 128_000,
                supports_images: true,
                approval: ApprovalMode::Auto,
                effort: Effort::Max,
                mode: AgentMode::Code,
            },
            provider: {
                let mut prov = crate::ai::ProviderConfig {
                    id: "default".to_string(),
                    name: "默认大模型".to_string(),
                    base_url: std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                    api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
                    model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
                };
                let cfg_file = crate::session::get_app_home().join("config.json");
                if cfg_file.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cfg_file) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                            if let Some(bu) = val.get("baseUrl").and_then(|v| v.as_str()) {
                                if !bu.is_empty() { prov.base_url = bu.to_string(); }
                            }
                            if let Some(ak) = val.get("apiKey").and_then(|v| v.as_str()) {
                                if !ak.is_empty() { prov.api_key = ak.to_string(); }
                            }
                            if let Some(md) = val.get("model").and_then(|v| v.as_str()) {
                                if !md.is_empty() { prov.model = md.to_string(); }
                            }
                            if let Some(nm) = val.get("name").and_then(|v| v.as_str()) {
                                if !nm.is_empty() { prov.name = nm.to_string(); }
                            }
                        }
                    }
                }
                prov
            },
            pending_questions: Vec::new(),
            public_workspace: workspace_path,

            appearance: "dark".to_string(),
            ui: UiSnapshot {
                active_id: initial_thread_id.clone(),
                open_tab_ids: vec![initial_thread_id],
                pending_draft: None,
                debug_open: false,
                settings_open: false,
                plugins_open: false,
                changes_open: false,
                palette_open: false,
                sidebar_open: true,
                search_open: false,
            },
        }
    }

    /// 设置活动会话
    pub fn focus_thread(&mut self, thread_id: String) -> bool {
        if self.threads.iter().any(|t| t.id == thread_id) {
            self.active_id = thread_id.clone();
            self.ui.active_id = thread_id.clone();
            if !self.ui.open_tab_ids.contains(&thread_id) {
                self.ui.open_tab_ids.push(thread_id);
            }
            true
        } else {
            false
        }
    }

    /// 打开标签页
    pub fn open_tab(&mut self, thread_id: String) {
        if !self.ui.open_tab_ids.contains(&thread_id) {
            self.ui.open_tab_ids.push(thread_id.clone());
        }
        self.focus_thread(thread_id);
    }

    /// 关闭标签页
    pub fn close_tab(&mut self, thread_id: &str) {
        self.ui.open_tab_ids.retain(|id| id != thread_id);
        if self.active_id == thread_id {
            if let Some(next_id) = self.ui.open_tab_ids.last().cloned() {
                self.focus_thread(next_id);
            }
        }
    }

    /// 创建新会话
    pub fn create_thread(&mut self, title: Option<String>) -> String {
        let id = next_id("thread");
        let thread = Thread {
            id: id.clone(),
            title: title.unwrap_or_else(|| "新会话".to_string()),
            created_at: now_millis(),
            workspace: self.workspace.project.clone(),
            items: Vec::new(),
            messages: Vec::new(),
            mode: Some(self.config.mode),
            parent_id: None,
            subagent_id: None,
            is_subagent: Some(false),
            plugin_data: None,
        };
        self.threads.insert(0, thread);
        self.open_tab(id.clone());
        id
    }

    /// 删除会话
    pub fn delete_thread(&mut self, thread_id: &str) -> bool {
        let prev_len = self.threads.len();
        self.threads.retain(|t| t.id != thread_id);
        self.close_tab(thread_id);
        if self.threads.is_empty() {
            self.create_thread(None);
        }
        self.threads.len() < prev_len
    }

    /// 记录一条调试日志（受控上限 60 条）
    pub fn push_log(&mut self, kind: impl Into<String>, text: impl Into<String>, payload: Option<serde_json::Value>) {
        const MAX_LOG: usize = 60;
        let id = self.log.last().map(|e| e.id + 1).unwrap_or(1);
        self.log.push(DebugEntry {
            id,
            at: now_millis(),
            kind: kind.into(),
            text: text.into(),
            payload,
            raw: None,
            model: None,
            duration_ms: None,
        });
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    /// 清空调试日志
    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// 设置会话是否处于运行中
    pub fn set_thread_running(&mut self, thread_id: &str, running: bool) {
        if running {
            if !self.running_thread_ids.iter().any(|id| id == thread_id) {
                self.running_thread_ids.push(thread_id.to_string());
            }
        } else {
            self.running_thread_ids.retain(|id| id != thread_id);
        }
    }

    /// 查找指定会话的可变引用
    pub fn get_thread_mut(&mut self, thread_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == thread_id)
    }

    /// 追加用户提问 Item
    pub fn add_user_message(&mut self, thread_id: &str, text: &str) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            t.items.push(Item::User {
                id: next_id("item_user"),
                at: now_millis(),
                text: text.to_string(),
                images: None,
                queued: None,
            });
        }
    }

    /// 标记结束未闭合的 Thinking Item
    pub fn end_thinking(&mut self, thread_id: &str) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            if let Some(Item::Thinking { ended_at, .. }) = t.items.last_mut() {
                if ended_at.is_none() {
                    *ended_at = Some(now_millis());
                }
            }
        }
    }

    /// 追加 Thinking 思考增量
    pub fn append_thinking_delta(&mut self, thread_id: &str, delta: &str) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            if let Some(Item::Thinking { text, ended_at, .. }) = t.items.last_mut() {
                if ended_at.is_none() {
                    text.push_str(delta);
                    return;
                }
            }
            // 否则创建新的 Thinking 卡片
            t.items.push(Item::Thinking {
                id: next_id("item_think"),
                at: now_millis(),
                text: delta.to_string(),
                ended_at: None,
            });
        }
    }

    /// 追加 Assistant 回复增量
    pub fn append_assistant_delta(&mut self, thread_id: &str, delta: &str) {
        // 先确保 Thinking 已结束
        self.end_thinking(thread_id);

        if let Some(t) = self.get_thread_mut(thread_id) {
            if let Some(Item::Assistant { text, streaming, .. }) = t.items.last_mut() {
                if *streaming == Some(true) {
                    text.push_str(delta);
                    return;
                }
            }
            // 否则创建新的流式 Assistant 卡片
            t.items.push(Item::Assistant {
                id: next_id("item_asst"),
                at: now_millis(),
                text: delta.to_string(),
                streaming: Some(true),
                duration_ms: None,
                turn_duration_ms: None,
            });
        }
    }

    /// 记录工具调用开始
    pub fn start_tool_call(&mut self, thread_id: &str, call_id: &str, name: &str, raw_args: &str) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            // 如果最后一个是流式 Assistant，停止其流式标记
            if let Some(Item::Assistant { streaming, .. }) = t.items.last_mut() {
                *streaming = None;
            }

            t.items.push(Item::Tool {
                id: next_id("item_tool"),
                at: now_millis(),
                call_id: call_id.to_string(),
                name: name.to_string(),
                args: serde_json::from_str(raw_args).unwrap_or(serde_json::Value::Null),
                raw_args: raw_args.to_string(),
                status: "running".to_string(),
                output: None,
                patch: None,
                details: None,
                thread_id: Some(thread_id.to_string()),
                checkpoint_id: None,
                reverted: None,
            });
        }
    }

    /// 记录工具调用完成
    pub fn finish_tool_call(&mut self, thread_id: &str, call_id: &str, ok: bool, output: Option<String>) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            for item in t.items.iter_mut().rev() {
                if let Item::Tool { call_id: cid, status, output: out, .. } = item {
                    if cid == call_id {
                        *status = if ok { "done".to_string() } else { "error".to_string() };
                        *out = output;
                        break;
                    }
                }
            }
        }
    }

    /// 完成当前轮次
    pub fn finish_turn(&mut self, thread_id: &str) {
        self.end_thinking(thread_id);
        if let Some(t) = self.get_thread_mut(thread_id) {
            if let Some(Item::Assistant { streaming, .. }) = t.items.last_mut() {
                *streaming = None;
            }
        }
        self.set_thread_running(thread_id, false);
    }
}
