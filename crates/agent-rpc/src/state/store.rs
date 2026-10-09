use crate::protocol::*;
use agent_base::ports::{AppHome, Clock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// 进程内时钟端口（组合根/测试可注入）。
///
/// 现状是若干模块各写一份 `SystemTime::now()`（本文件的 `now_millis` 等），
/// 测试无法确定性重放。这里是过渡期的显式缝隙：调用方仍用 `now_millis()`，
/// 但时间源已经可替换（测试注入 `agent_base::testing::FixedClock`）。
static CLOCK: OnceLock<Box<dyn Clock>> = OnceLock::new();

pub fn clock() -> &'static dyn Clock {
    CLOCK.get_or_init(|| Box::new(agent_adapter::clock::SystemClock)).as_ref()
}

/// 注入时钟。返回 `Err` 表示本进程已经初始化过（只允许一次）。
pub fn set_clock(c: Box<dyn Clock>) -> Result<(), Box<dyn Clock>> {
    CLOCK.set(c)
}

pub fn now_millis() -> u64 {
    clock().now_ms().max(0) as u64
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
    /// 用进程默认的应用目录端口构造（组合根/生产路径）。
    pub fn new(workspace_path: String) -> Self {
        Self::with_home(workspace_path, agent_node::session::app_home())
    }

    /// 注入应用目录端口构造（测试与组合根用）。
    ///
    /// 这是本批次引入端口后的第一个**真实使用点**：测试不再靠
    /// `std::env::set_var("A_DA_HOME")`（那种做法在端口被 `OnceLock` 初始化后即失效），
    /// 而是显式传入一个 [`AppHome`]——身份明确、可并行、不污染进程环境。
    pub fn with_home(workspace_path: String, home: &dyn AppHome) -> Self {
        let explicit_workspace = workspace_path.trim().to_string();
        let default_public_workspace = home.dir("workspace").to_string_lossy().to_string();

        let mut prov = crate::ai::ProviderConfig {
            id: "default".to_string(),
            name: "默认大模型".to_string(),
            protocol: crate::ai::ModelProtocol::OpenAiChat,
            base_url: std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            max_output_tokens: Some(8192),
            custom_headers: None,
            proxy_url: std::env::var("A_DA_PROXY_URL").or_else(|_| std::env::var("ALL_PROXY")).or_else(|_| std::env::var("HTTPS_PROXY")).or_else(|_| std::env::var("HTTP_PROXY")).ok(),
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

        let cfg_file = home.config_file();

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
                    if let Some(pu) = val.get("proxyUrl").and_then(|v| v.as_str()) {
                        prov.proxy_url = if pu.trim().is_empty() { None } else { Some(pu.trim().to_string()) };
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
                proxy_url: prov.proxy_url.clone(),
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
            prov.proxy_url = active_entry.proxy_url.clone();
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

        let session_mgr = agent_node::session::SessionManager::new(None);
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

    /// 查询指定会话是否处于运行中
    pub fn is_thread_running(&self, thread_id: &str) -> bool {
        self.running_thread_ids.iter().any(|id| id == thread_id)
    }

    /// 追加排队消息
    pub fn enqueue_message(&mut self, thread_id: &str, text: &str, images: Option<Vec<String>>) -> String {
        let qid = next_id("queue");
        self.queue.push(QueuedItem {
            id: qid.clone(),
            thread_id: thread_id.to_string(),
            text: text.to_string(),
            enqueued_at: now_millis(),
            images,
        });
        qid
    }

    /// 取出指定会话的下一个排队消息
    pub fn pop_next_queued(&mut self, thread_id: &str) -> Option<QueuedItem> {
        if let Some(idx) = self.queue.iter().position(|q| q.thread_id == thread_id) {
            Some(self.queue.remove(idx))
        } else {
            None
        }
    }

    /// 查找指定会话的可变引用
    pub fn get_thread_mut(&mut self, thread_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == thread_id)
    }

    /// 追加用户提问 Item
    pub fn add_user_message(&mut self, thread_id: &str, text: &str) {
        self.add_user_message_with_images(thread_id, text, None);
    }

    /// 追加用户提问 Item（支持图片）
    pub fn add_user_message_with_images(&mut self, thread_id: &str, text: &str, images: Option<Vec<String>>) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            t.items.push(Item::User {
                id: next_id("item_user"),
                at: now_millis(),
                text: text.to_string(),
                images,
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
        if delta.is_empty() {
            return;
        }
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
        if delta.is_empty() {
            return;
        }
        // 先确保 Thinking 已结束
        self.end_thinking(thread_id);

        if let Some(t) = self.get_thread_mut(thread_id) {
            // 清理末尾无内容的空思考卡片（避免模型思考为空时占位隔断）
            if let Some(Item::Thinking { text, .. }) = t.items.last() {
                if text.trim().is_empty() {
                    t.items.pop();
                }
            }

            // 倒序寻找当前仍处于流式中的 Assistant Item，优先复用追加
            for item in t.items.iter_mut().rev() {
                if let Item::Assistant { text, streaming, .. } = item {
                    if *streaming == Some(true) {
                        text.push_str(delta);
                        return;
                    }
                    break;
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
            // 停止所有未闭合 Assistant 的流式标记
            for item in t.items.iter_mut() {
                if let Item::Assistant { streaming, .. } = item {
                    *streaming = None;
                }
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
                duration_ms: None,
                started_at: Some(now_millis() as i64),
                finished_at: None,
            });
        }
    }

    /// 记录工具调用完成
    pub fn finish_tool_call(
        &mut self,
        thread_id: &str,
        call_id: &str,
        ok: bool,
        output: Option<String>,
        duration_ms: Option<u64>,
        started_at: Option<i64>,
        finished_at: Option<i64>,
    ) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            for item in t.items.iter_mut().rev() {
                if let Item::Tool {
                    call_id: cid,
                    status,
                    output: out,
                    duration_ms: dur,
                    started_at: st,
                    finished_at: fin,
                    ..
                } = item
                {
                    if cid == call_id {
                        *status = if ok { "done".to_string() } else { "error".to_string() };
                        *out = output;
                        *dur = duration_ms;
                        if started_at.is_some() {
                            *st = started_at;
                        }
                        *fin = finished_at;
                        break;
                    }
                }
            }
        }
        self.pending_questions.retain(|item| item.call_id != call_id);
    }

    /// 标记工具调用正在等待用户提问答复
    pub fn set_tool_awaiting_question(&mut self, thread_id: &str, call_id: &str, question_val: serde_json::Value) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            for item in t.items.iter_mut().rev() {
                if let Item::Tool { call_id: cid, status, details, .. } = item {
                    if cid == call_id {
                        *status = "awaiting".to_string();
                        let mut det = details.take().unwrap_or_else(|| serde_json::json!({}));
                        if let Some(obj) = det.as_object_mut() {
                            obj.insert("question".to_string(), question_val.clone());
                        }
                        *details = Some(det);
                        break;
                    }
                }
            }
        }

        // 同步到 pending_questions 列表中
        if let Ok(q) = serde_json::from_value::<crate::protocol::AgentQuestion>(question_val) {
            self.pending_questions.retain(|item| item.call_id != call_id);
            self.pending_questions.push(crate::protocol::PendingQuestionEntry {
                call_id: call_id.to_string(),
                question: q,
            });
        }
    }

    /// 标记工具调用**正在等待用户批准**（W3-T3）。
    ///
    /// 界面按 `status == "waiting_approval"` 渲染"批准/拒绝"按钮
    /// （`tauri-ui/src/components/Transcript.tsx:683` 的 `isAwaiting`），
    /// 点按钮发 `approval.decide` → `Dispatcher.approval_mgr.resolve_approval(...)`
    /// → 唤醒引擎的审批闸门。**前端无需任何改动**（R7）。
    ///
    /// 与 [`Self::set_tool_awaiting_question`] 的区别：那个是"等用户回答提问"（`ask_user`），
    /// 这个是"等用户批准/拒绝这次工具调用"；界面对两者的处理相同（同一组按钮），
    /// 但状态字符串不同，便于区分与诊断。
    pub fn set_tool_waiting_approval(&mut self, thread_id: &str, call_id: &str, tool: &str) {
        if let Some(t) = self.get_thread_mut(thread_id) {
            for item in t.items.iter_mut().rev() {
                if let Item::Tool { call_id: cid, status, details, .. } = item {
                    if cid == call_id {
                        *status = "waiting_approval".to_string();
                        let mut det = details.take().unwrap_or_else(|| serde_json::json!({}));
                        if let Some(obj) = det.as_object_mut() {
                            obj.insert(
                                "approval".to_string(),
                                serde_json::json!({ "tool": tool, "pending": true }),
                            );
                        }
                        *details = Some(det);
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
            // 闭合该会话中全部处于流式中的 Assistant 卡片
            for item in t.items.iter_mut() {
                if let Item::Assistant { streaming, .. } = item {
                    *streaming = None;
                }
            }
            // 清理末尾无内容的空思考卡片
            if let Some(Item::Thinking { text, .. }) = t.items.last() {
                if text.trim().is_empty() {
                    t.items.pop();
                }
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

        // 注入式应用目录：不再依赖 `std::env::set_var`（端口化后那种做法已失效）
        let home = agent_base::testing::TempAppHome::at(&temp_home).with_config_file(&cfg_path);

        let store = AgentStore::with_home("".to_string(), &home);
        assert_eq!(store.config.model, "deepseek/deepseek-v4.1-flash");
        assert_eq!(store.provider.base_url, "http://ai.20301024.xyz:36302/v1");
        assert_eq!(store.provider.api_key, "sk-test-key");
        assert_eq!(store.config.context_window, 65536);
        assert_eq!(store.appearance, "light");

        let _ = std::fs::remove_dir_all(temp_home);
    }

    /// W3-T3 守门：审批请求必须把工具卡片置为界面认得的 **`waiting_approval`**。
    ///
    /// 前端按这个字符串决定是否渲染"批准/拒绝"按钮
    /// （`tauri-ui/src/components/Transcript.tsx:683` 的 `isAwaiting`）。
    /// 改了状态字符串而没同步前端，审批按钮就会**静默消失**——所以这里钉死它。
    #[test]
    fn test_waiting_approval_status_is_the_ui_contract() {
        let mut store = AgentStore::new("E:/codes/ui_contract_ws".to_string());
        let tid = store.create_thread(Some("界面契约".to_string()), None);
        let tid = tid.as_str();
        store.add_user_message(tid, "执行工具");
        store.start_tool_call(tid, "call_ui_1", "gated_tool", "{}");

        // 起始状态是运行中
        let status_of = |s: &AgentStore| -> String {
            s.threads
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| {
                    t.items.iter().find_map(|i| match i {
                        Item::Tool { call_id, status, .. } if call_id == "call_ui_1" => {
                            Some(status.clone())
                        }
                        _ => None,
                    })
                })
                .expect("工具项应存在")
        };
        assert_eq!(status_of(&store), "running");

        store.set_tool_waiting_approval(tid, "call_ui_1", "gated_tool");
        assert_eq!(
            status_of(&store),
            "waiting_approval",
            "界面按这个字符串渲染批准/拒绝按钮（前端契约）"
        );

        // details 里带上审批信息，便于界面/诊断展示
        let details = store
            .threads
            .iter()
            .find(|t| t.id == tid)
            .and_then(|t| {
                t.items.iter().find_map(|i| match i {
                    Item::Tool { call_id, details, .. } if call_id == "call_ui_1" => details.clone(),
                    _ => None,
                })
            })
            .expect("工具项应有 details");
        assert_eq!(details["approval"]["tool"], "gated_tool");
        assert_eq!(details["approval"]["pending"], true);

        // 收尾必须覆盖等待态（否则卡片会一直停在"等批准"）
        store.finish_tool_call(tid, "call_ui_1", true, Some("done".into()), Some(1), Some(0), Some(1));
        let after = status_of(&store);
        assert_ne!(after, "waiting_approval", "工具完成后不得仍停在等待批准：{after}");
    }
}

