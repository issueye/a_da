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
    pub providers: Vec<crate::ai::ProviderEntry>,
    pub active_provider_id: String,
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
        let explicit_workspace = workspace_path.trim().to_string();
        let default_public_workspace = crate::session::get_app_home().join("workspace").to_string_lossy().to_string();

        let mut prov = crate::ai::ProviderConfig {
            id: "default".to_string(),
            name: "默认大模型".to_string(),
            protocol: crate::ai::ModelProtocol::OpenAiChat,
            base_url: std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            max_output_tokens: Some(8192),
            custom_headers: None,
        };
        let mut providers_list: Vec<crate::ai::ProviderEntry> = Vec::new();
        let mut active_pid = "default".to_string();
        let mut config_snapshot = ConfigSnapshot {
            model: prov.model.clone(),
            context_window: 128_000,
            max_output_tokens: Some(8192),
            supports_images: true,
            approval: ApprovalMode::Auto,
            effort: Effort::Max,
            mode: AgentMode::Code,
        };
        let mut appearance_str = "dark".to_string();

        let cfg_file = crate::session::get_config_path();

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
                        if !md.is_empty() {
                            prov.model = md.to_string();
                            config_snapshot.model = md.to_string();
                        }
                    }
                    if let Some(nm) = val.get("name").and_then(|v| v.as_str()) {
                        if !nm.is_empty() { prov.name = nm.to_string(); }
                    }
                    if let Some(cw) = val.get("contextWindow").and_then(|v| v.as_u64()) {
                        config_snapshot.context_window = cw;
                    }
                    if let Some(mot) = val.get("maxOutputTokens").and_then(|v| v.as_u64()) {
                        config_snapshot.max_output_tokens = Some(mot);
                        prov.max_output_tokens = Some(mot);
                    }
                    if let Some(si) = val.get("supportsImages").and_then(|v| v.as_bool()) {
                        config_snapshot.supports_images = si;
                    }
                    if let Some(ch) = val.get("customHeaders").and_then(|v| v.as_object()) {
                        let mut map = std::collections::HashMap::new();
                        for (k, v) in ch {
                            if let Some(s) = v.as_str() {
                                map.insert(k.clone(), s.to_string());
                            }
                        }
                        prov.custom_headers = Some(map);
                    }
                    if let Some(appr) = val.get("approval").and_then(|v| v.as_str()) {
                        config_snapshot.approval = match appr.to_lowercase().as_str() {
                            "ask" => ApprovalMode::Ask,
                            "readonly" => ApprovalMode::Readonly,
                            _ => ApprovalMode::Auto,
                        };
                    }
                    if let Some(eff) = val.get("effort").and_then(|v| v.as_str()) {
                        config_snapshot.effort = match eff.to_lowercase().as_str() {
                            "high" => Effort::High,
                            "medium" => Effort::Medium,
                            "low" => Effort::Low,
                            _ => Effort::Max,
                        };
                    }
                    if let Some(md) = val.get("mode").and_then(|v| v.as_str()) {
                        config_snapshot.mode = match md.to_lowercase().as_str() {
                            "plan" => AgentMode::Plan,
                            "create" => AgentMode::Create,
                            _ => AgentMode::Code,
                        };
                    }
                    if let Some(app) = val.get("appearance").and_then(|v| v.as_str()) {
                        if !app.is_empty() { appearance_str = app.to_string(); }
                    }

                    if let Some(ap_id) = val.get("activeProviderId").and_then(|v| v.as_str()) {
                        if !ap_id.is_empty() { active_pid = ap_id.to_string(); }
                    }

                    if let Some(p_array) = val.get("providers").and_then(|v| v.as_array()) {
                        if let Ok(parsed_providers) = serde_json::from_value::<Vec<crate::ai::ProviderEntry>>(serde_json::Value::Array(p_array.clone())) {
                            providers_list = parsed_providers;
                        }
                    }
                }
            }
        }

        // 如果配置中未指定多供应商（旧版配置迁移），构造并填充默认供应商
        if providers_list.is_empty() {
            providers_list.push(crate::ai::ProviderEntry {
                id: "default".to_string(),
                name: "默认大模型".to_string(),
                protocol: prov.protocol,
                base_url: prov.base_url.clone(),
                api_key: prov.api_key.clone(),
                models: vec![crate::ai::ModelEntry {
                    id: prov.model.clone(),
                    name: Some(prov.model.clone()),
                    context_window: Some(config_snapshot.context_window),
                    max_output_tokens: config_snapshot.max_output_tokens,
                    supports_images: Some(config_snapshot.supports_images),
                }],
                custom_headers: prov.custom_headers.clone(),
            });
        }

        // 根据 activeProviderId 从 providers_list 同步更新 prov 与 config_snapshot
        if let Some(active_entry) = providers_list.iter().find(|p| p.id == active_pid) {
            prov.id = active_entry.id.clone();
            prov.name = active_entry.name.clone();
            prov.base_url = active_entry.base_url.clone();
            prov.api_key = active_entry.api_key.clone();
            prov.protocol = active_entry.protocol;
            prov.custom_headers = active_entry.custom_headers.clone();
            if let Some(first_m) = active_entry.models.first() {
                prov.model = first_m.id.clone();
                config_snapshot.model = first_m.id.clone();
                if let Some(cw) = first_m.context_window {
                    config_snapshot.context_window = cw;
                }
                if let Some(mot) = first_m.max_output_tokens {
                    config_snapshot.max_output_tokens = Some(mot);
                    prov.max_output_tokens = Some(mot);
                }
                if let Some(si) = first_m.supports_images {
                    config_snapshot.supports_images = si;
                }
            }
        }

        let session_mgr = crate::session::SessionManager::new(None);
        let restored_threads = session_mgr.restore_all_threads();

        let (threads, active_id, current_workspace) = if !restored_threads.is_empty() {
            let target_ws = if !explicit_workspace.is_empty() {
                explicit_workspace.clone()
            } else if let Ok(env_ws) = std::env::var("A_DA_WORKSPACE") {
                if !env_ws.trim().is_empty() { env_ws.trim().to_string() } else { String::new() }
            } else {
                String::new()
            };

            let chosen = if !target_ws.is_empty() {
                restored_threads.iter().find(|t| t.workspace == target_ws && t.is_subagent != Some(true))
                    .or_else(|| restored_threads.iter().find(|t| t.workspace == target_ws))
                    .unwrap_or(&restored_threads[0])
            } else {
                restored_threads.iter().find(|t| t.is_subagent != Some(true))
                    .unwrap_or(&restored_threads[0])
            };

            let act_id = chosen.id.clone();
            let ws = if !target_ws.is_empty() {
                target_ws
            } else if !chosen.workspace.is_empty() {
                chosen.workspace.clone()
            } else {
                default_public_workspace.clone()
            };

            (restored_threads, act_id, ws)
        } else {
            let ws = if !explicit_workspace.is_empty() {
                explicit_workspace
            } else if let Ok(env_ws) = std::env::var("A_DA_WORKSPACE") {
                if !env_ws.trim().is_empty() { env_ws.trim().to_string() } else { default_public_workspace.clone() }
            } else {
                default_public_workspace.clone()
            };

            let initial_thread_id = next_id("thread");
            let initial_thread = Thread {
                id: initial_thread_id.clone(),
                title: "新会话".to_string(),
                created_at: now_millis(),
                workspace: ws.clone(),
                items: Vec::new(),
                messages: Vec::new(),
                mode: Some(config_snapshot.mode),
                parent_id: None,
                subagent_id: None,
                is_subagent: Some(false),
                plugin_data: None,
            };

            (vec![initial_thread], initial_thread_id, ws)
        };

        let open_tab_ids = vec![active_id.clone()];

        Self {
            threads,
            active_id: active_id.clone(),
            running_thread_ids: Vec::new(),
            waiting_thread_ids: Vec::new(),
            queue: Vec::new(),
            log: Vec::new(),
            workspace: WorkspaceSnapshot {
                project: current_workspace,
                files: 0,
                dirs: 0,
                scanning: false,
                entries: Vec::new(),
            },
            config: config_snapshot,
            provider: prov,
            providers: providers_list,
            active_provider_id: active_pid,
            pending_questions: Vec::new(),
            public_workspace: default_public_workspace,
            appearance: appearance_str,
            ui: UiSnapshot {
                active_id,
                open_tab_ids,
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
        if let Some(thread) = self.threads.iter().find(|t| t.id == thread_id) {
            // 会话与工作区严格绑定：当前工作区跟随被聚焦的会话走，
            // 否则 fs.roots / plugin.list / 新建会话这些"取当前工作区为默认值"的路径
            // 会一直停留在启动时那个工作区上
            let ws = thread.workspace.trim().to_string();
            self.active_id = thread_id.clone();
            self.ui.active_id = thread_id.clone();
            if !ws.is_empty() {
                self.workspace.project = ws;
            }
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
    ///
    /// `workspace` 为空时继承当前工作区。传入工作区时会话即绑定到该工作区，
    /// 后续 Agent 执行的沙箱根、会话落盘目录都以它为准。
    pub fn create_thread(&mut self, title: Option<String>, workspace: Option<String>) -> String {
        let id = next_id("thread");
        let ws = workspace
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| self.workspace.project.clone());
        let thread = Thread {
            id: id.clone(),
            title: title.unwrap_or_else(|| "新会话".to_string()),
            created_at: now_millis(),
            workspace: ws,
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
            self.create_thread(None, None);
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
                usage: None,
            });
        }
    }

    /// 记录一次大模型调用的真实用量与耗时
    ///
    /// 遥测条只认"最后一次请求"的真实数字，所以这里既要在当前流式助手卡片上落数，
    /// 也要在本步只调了工具、没产出文本时补一张隐藏卡片承载它——否则界面会一直
    /// 显示上一轮的旧数字。
    pub fn set_assistant_stats(
        &mut self,
        thread_id: &str,
        usage: Option<crate::ai::TokenUsage>,
        duration_ms: u64,
        turn_duration_ms: u64,
    ) {
        let Some(t) = self.get_thread_mut(thread_id) else { return };

        if matches!(t.items.last(), Some(Item::Assistant { .. })) {
            if let Some(Item::Assistant { usage: u, duration_ms: d, turn_duration_ms: td, .. }) =
                t.items.last_mut()
            {
                *u = usage;
                *d = Some(duration_ms);
                *td = Some(turn_duration_ms);
            }
            return;
        }

        t.items.push(Item::Assistant {
            id: next_id("item_asst"),
            at: now_millis(),
            text: String::new(),
            streaming: None,
            duration_ms: Some(duration_ms),
            turn_duration_ms: Some(turn_duration_ms),
            usage,
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_config_and_session_restore() {
        let temp_home = std::env::temp_dir().join(format!("a_da_store_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_home);
        let cfg_path = temp_home.join("config.json");
        std::fs::write(&cfg_path, serde_json::json!({
            "model": "deepseek/deepseek-v4.1-flash",
            "baseUrl": "http://ai.20301024.xyz:36302/v1",
            "apiKey": "sk-test-key",
            "contextWindow": 65536,
            "appearance": "light"
        }).to_string()).unwrap();

        unsafe {
            std::env::set_var("A_DA_HOME", &temp_home);
            std::env::set_var("A_DA_CONFIG", &cfg_path);
        }

        let store = AgentStore::new("".to_string());
        assert_eq!(store.config.model, "deepseek/deepseek-v4.1-flash");
        assert_eq!(store.provider.base_url, "http://ai.20301024.xyz:36302/v1");
        assert_eq!(store.provider.api_key, "sk-test-key");
        assert_eq!(store.config.context_window, 65536);
        assert_eq!(store.appearance, "light");

        let _ = std::fs::remove_dir_all(temp_home);
        unsafe {
            std::env::remove_var("A_DA_CONFIG");
        }
    }
}

