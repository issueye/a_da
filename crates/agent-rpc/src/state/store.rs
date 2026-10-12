use agent_proto::*;
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
    /// **按线程**记录"被第几层委派驱动"（S6 补完）。`0`/缺省 = 用户直接连的。
    ///
    /// 不进快照：它是**驱动方的记账**，界面不需要它（INV-8：一个事实一个所有者）。
    pub delegation_depths: std::collections::BTreeMap<String, u32>,
    pub active_id: String,
    pub running_thread_ids: Vec<String>,
    pub waiting_thread_ids: Vec<String>,
    pub queue: Vec<QueuedItem>,
    pub log: Vec<DebugEntry>,
    pub workspace: WorkspaceSnapshot,
    pub config: ConfigSnapshot,
    pub provider: agent_base::model::ProviderConfig,
    pub providers: Vec<agent_base::model::ProviderEntry>,
    pub active_provider_id: String,
    pub pending_questions: Vec<PendingQuestionEntry>,
    pub public_workspace: String,
    pub appearance: String,
    pub ui: UiSnapshot,
    /// 本实例的 `config.json` 落盘位置（构造时从注入的 [`AppHome`] 取）。
    ///
    /// 🔴 存下来而不是每次去问全局 `agent_node::session::get_config_path()`：
    /// 测试/嵌入式宿主注入了自己的应用目录，写盘却跑到用户真实的 `~/.a-da` 去，
    /// 既污染用户配置又让"设置真的落盘了吗"无法断言。
    pub config_path: std::path::PathBuf,
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

        let mut prov = agent_base::model::ProviderConfig {
            id: "default".to_string(),
            name: "默认大模型".to_string(),
            protocol: agent_base::model::ModelProtocol::OpenAiChat,
            base_url: std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            max_output_tokens: Some(8192),
            custom_headers: None,
            proxy_url: std::env::var("A_DA_PROXY_URL").or_else(|_| std::env::var("ALL_PROXY")).or_else(|_| std::env::var("HTTPS_PROXY")).or_else(|_| std::env::var("HTTP_PROXY")).ok(),
        };
        let mut providers_list: Vec<agent_base::model::ProviderEntry> = Vec::new();
        let mut active_pid = "default".to_string();
        let mut config_snapshot = ConfigSnapshot {
            model: prov.model.clone(),
            context_window: 128_000,
            max_output_tokens: Some(8192),
            supports_images: true,
            approval: ApprovalMode::Auto,
            effort: Effort::Max,
            mode: AgentMode::Code,
            retry_count: agent_base::model::DEFAULT_MODEL_RETRY_COUNT as u32,
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
                    // 模型请求重试次数（设置项）；缺失/越界一律回落到合法区间
                    if let Some(rc) = val.get("retryCount").and_then(|v| v.as_u64()) {
                        config_snapshot.retry_count =
                            agent_base::model::normalize_model_retry_count(rc as usize) as u32;
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
                        if let Ok(parsed_providers) = serde_json::from_value::<Vec<agent_base::model::ProviderEntry>>(serde_json::Value::Array(p_array.clone())) {
                            providers_list = parsed_providers;
                        }
                    }
                }
            }
        }

        // 如果配置中未指定多供应商（旧版配置迁移），构造并填充默认供应商
        if providers_list.is_empty() {
            providers_list.push(agent_base::model::ProviderEntry {
                id: "default".to_string(),
                name: "默认大模型".to_string(),
                protocol: prov.protocol,
                base_url: prov.base_url.clone(),
                api_key: prov.api_key.clone(),
                models: vec![agent_base::model::ModelEntry {
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
                agent_id: Some("ada-coding".to_string()),
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
            delegation_depths: std::collections::BTreeMap::new(),
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
            config_path: cfg_file.clone(),
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
        self.create_thread_with_agent(title, workspace, None, None)
    }

    /// 创建带指定模式和 Agent 角色的会话
    pub fn create_thread_with_agent(
        &mut self,
        title: Option<String>,
        workspace: Option<String>,
        mode: Option<AgentMode>,
        agent_id: Option<String>,
    ) -> String {
        self.create_thread_with_details(title, workspace, mode, agent_id, None, None, None)
    }

    /// 创建带完整子智能体属性的会话
    pub fn create_thread_with_details(
        &mut self,
        title: Option<String>,
        workspace: Option<String>,
        mode: Option<AgentMode>,
        agent_id: Option<String>,
        is_subagent: Option<bool>,
        parent_id: Option<String>,
        subagent_id: Option<String>,
    ) -> String {
        let id = next_id("thread");
        let ws = workspace
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| self.workspace.project.clone());
        let m = mode.unwrap_or(self.config.mode);
        // **PM 是模式，不是产品**：产品身份恒为本机唯一产品 `ada-coding`。
        //
        // 早先这里按模式把 `agent_id` 写成 `ada-pm`——于是"这条会话是不是 PM"有了两个
        // 真源（产品 id 与 mode），落盘/还原/徽标/进程面板全要跟着对齐，本次会话里
        // 反复出问题的就是这笔账。现在只认 `mode`。
        let actual_agent_id = agent_id.or_else(|| Some("ada-coding".to_string()));
        let is_sub = is_subagent.unwrap_or(false);
        let thread = Thread {
            id: id.clone(),
            title: title.unwrap_or_else(|| "新会话".to_string()),
            created_at: now_millis(),
            workspace: ws,
            items: Vec::new(),
            messages: Vec::new(),
            mode: Some(m),
            agent_id: actual_agent_id,
            parent_id,
            subagent_id,
            is_subagent: Some(is_sub),
            plugin_data: None,
        };
        self.threads.insert(0, thread);
        if !is_sub {
            self.open_tab(id.clone());
        }
        id
    }

    /// 同步并合并来自磁盘持久化存储的会话（支持多 Agent 实例/跨网关创建的子智能体会话动态发现）
    pub fn sync_threads_from_disk(&mut self, session_mgr: &agent_node::session::SessionManager) -> bool {
        let disk_threads = session_mgr.restore_all_threads();
        if disk_threads.is_empty() {
            return false;
        }
        let mut changed = false;
        for dt in disk_threads {
            // 「运行中就不合并磁盘」这条保护只对**本进程自己流式吐字**的会话成立。
            // 子智能体会话跑在自己的运行时里（跨网关时甚至是另一个进程），
            // 本进程从不往它的 item 上追加 delta——它的**唯一真源就是磁盘会话文件**。
            // 早先把这条保护也套在子会话上，结果是子会话永远停在创建那一刻的空壳：
            // 界面整轮（本例 4.5 分钟）看不到任何进展，用户只能盯着一张静止的卡片，
            // 无法判断它是"在干活"还是"卡死了"。
            let locally_streamed =
                dt.is_subagent != Some(true) && self.running_thread_ids.contains(&dt.id);
            if let Some(existing) = self.threads.iter_mut().find(|t| t.id == dt.id) {
                if !locally_streamed {
                    if items_visibly_differ(&existing.items, &dt.items)
                        || dt.messages.len() != existing.messages.len()
                    {
                        existing.items = dt.items;
                        existing.messages = dt.messages;
                        changed = true;
                    }
                    if dt.title != "新会话" && (existing.title == "新会话" || existing.title.is_empty()) {
                        existing.title = dt.title;
                        changed = true;
                    }
                    if dt.is_subagent.is_some() && existing.is_subagent != dt.is_subagent {
                        existing.is_subagent = dt.is_subagent;
                        changed = true;
                    }
                    if dt.parent_id.is_some() && existing.parent_id != dt.parent_id {
                        existing.parent_id = dt.parent_id;
                        changed = true;
                    }
                    if dt.subagent_id.is_some() && existing.subagent_id != dt.subagent_id {
                        existing.subagent_id = dt.subagent_id;
                        changed = true;
                    }
                }
            } else {
                // 发现跨进程新创建的会话（如子智能体会话）
                self.threads.push(dt);
                changed = true;
            }
        }
        changed
    }

    /// 从 `parent` 名下的子智能体会话里挑出**本次派活新建的那一个**。
    ///
    /// - `known`：这次调用**之前**该父会话就已经有的子会话（那些是历史，不是本次的产物）
    /// - `bound`：已经被**别的运行中委派卡片**绑定过的子会话
    ///
    /// 早先这里是「任取一个 `parent_id` 匹配的子会话」，而 store 里子会话是**按发现顺序
    /// 追加**的，于是第二次派活抓到的往往是**上一次已经结束**的子会话：它被重新标成运行中、
    /// 新卡片也被绑到旧会话上——界面表现就是"报告都出来了，会话却一直显示在运行"。
    pub fn pick_new_subagent_child(
        &self,
        parent: &str,
        known: &[String],
        bound: &[String],
    ) -> Option<String> {
        self.threads
            .iter()
            .find(|t| {
                t.parent_id.as_deref() == Some(parent)
                    && t.is_subagent == Some(true)
                    && !known.iter().any(|k| k == &t.id)
                    && !bound.iter().any(|b| b == &t.id)
            })
            .map(|t| t.id.clone())
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
        usage: Option<agent_base::model::TokenUsage>,
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

            // 🔴 审批阶段可能已经补过一张 `waiting_approval` 占位卡（引擎在 `ToolCallStarted`
            // **之前**就发 `ApprovalRequested`，见 `set_tool_waiting_approval`）。这里必须
            // **复用同一张**，不能再 push：同一 call_id 出现两张卡时，`finish_tool_call`
            // 只会命中最后一张，先出现的那张会永远停在"等待批准"上。
            let existing = t.items.iter().rposition(
                |it| matches!(it, Item::Tool { call_id: cid, .. } if cid.as_str() == call_id),
            );
            if let Some(idx) = existing {
                if let Item::Tool {
                    name: n,
                    args,
                    raw_args: ra,
                    status,
                    started_at,
                    ..
                } = &mut t.items[idx]
                {
                    *n = name.to_string();
                    *args = serde_json::from_str(raw_args).unwrap_or(serde_json::Value::Null);
                    *ra = raw_args.to_string();
                    *status = "running".to_string();
                    if started_at.is_none() {
                        *started_at = Some(now_millis() as i64);
                    }
                }
            } else {
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

        // 审批通过、工具开始执行 → "正在等你批准"不再是事实，必须重算：
        // 否则"有会话在等你操作"的全局提示会一直挂在界面上（这条状态由卡片状态派生）。
        self.refresh_waiting_state(thread_id);
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
        details: Option<serde_json::Value>,
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
                    details: det,
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
                        if let Some(new_det) = details {
                            let mut base = det.take().unwrap_or_else(|| serde_json::json!({}));
                            if let (Some(b_obj), Some(n_obj)) = (base.as_object_mut(), new_det.as_object()) {
                                for (k, v) in n_obj {
                                    b_obj.insert(k.clone(), v.clone());
                                }
                                *det = Some(base);
                            } else {
                                *det = Some(new_det);
                            }
                        }
                        break;
                    }
                }
            }
        }
        self.pending_questions.retain(|item| item.call_id != call_id);
        // 该工具不再等待 → 若这个会话已经没有别的等待项，就从"等你操作"里摘掉
        self.refresh_waiting_state(thread_id);
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
        // （状态与提问时刻由**持有方**填：提问事件里没有这两个字段）
        if let Ok(mut q) = serde_json::from_value::<agent_proto::AgentQuestion>(question_val) {
            q.status = "awaiting".to_string();
            q.asked_at = now_millis();
            self.pending_questions.retain(|item| item.call_id != call_id);
            self.pending_questions.push(agent_proto::PendingQuestionEntry {
                call_id: call_id.to_string(),
                question: q,
            });
        }

        // 提问同样属于"等你操作"：别的会话里也要能看见
        self.refresh_waiting_state(thread_id);
    }

    /// 维护 `waiting_thread_ids`：界面靠它做"**别的会话正在等你操作**"的全局提示。
    ///
    /// 这个字段以前只初始化、只进快照，**从来没被填过**——于是某个会话在等批准时，
    /// 你在别的标签页上什么都看不到（实测：一条 `cargo test` 的审批请求等了 5 分钟超时
    /// 被自动拒绝，用户完全不知道发生过）。
    fn refresh_waiting_state(&mut self, thread_id: &str) {
        let still_waiting = self
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .map(|t| {
                t.items.iter().any(|it| {
                    matches!(
                        it,
                        Item::Tool { status, .. }
                            if status == "waiting_approval" || status == "awaiting"
                    )
                })
            })
            .unwrap_or(false);
        if still_waiting {
            if !self.waiting_thread_ids.iter().any(|id| id == thread_id) {
                self.waiting_thread_ids.push(thread_id.to_string());
            }
        } else {
            self.waiting_thread_ids.retain(|id| id != thread_id);
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
        let approval_detail = serde_json::json!({ "tool": tool, "pending": true });
        if let Some(t) = self.get_thread_mut(thread_id) {
            let mut found = false;
            for item in t.items.iter_mut().rev() {
                if let Item::Tool { call_id: cid, status, details, .. } = item {
                    if cid == call_id {
                        *status = "waiting_approval".to_string();
                        let mut det = details.take().unwrap_or_else(|| serde_json::json!({}));
                        if let Some(obj) = det.as_object_mut() {
                            obj.insert("approval".to_string(), approval_detail.clone());
                        } else {
                            det = serde_json::json!({ "approval": approval_detail.clone() });
                        }
                        *details = Some(det);
                        found = true;
                        break;
                    }
                }
            }

            // 🔴 找不到卡片就**补一张**。
            //
            // 引擎发 `ApprovalRequested` 是在 `ToolCallStarted`（唯一的建卡点，且在审批
            // **通过之后**）之前，所以正常路径下这里**必然找不到**已存在的卡片。早先只
            // "改已存在的卡片"，于是审批事件静默 no-op：界面既没有审批卡片、`waiting_thread_ids`
            // 也为空——用户根本无从批准，只能干等审批超时（30 分钟）被自动拒绝。
            // 卡片的 `call_id` 与后续 `ToolCallStarted` 一致，届时由 `start_tool_call` 复用。
            if !found {
                t.items.push(Item::Tool {
                    id: next_id("item_tool"),
                    at: now_millis(),
                    call_id: call_id.to_string(),
                    name: tool.to_string(),
                    args: serde_json::Value::Null,
                    raw_args: String::new(),
                    status: "waiting_approval".to_string(),
                    output: None,
                    patch: None,
                    details: Some(serde_json::json!({ "approval": approval_detail })),
                    thread_id: Some(thread_id.to_string()),
                    checkpoint_id: None,
                    reverted: None,
                    duration_ms: None,
                    started_at: None,
                    finished_at: None,
                });
            }
        }

        // 让"别的会话在等你批准"这件事可见（否则只能在那条会话里看到，极易错过）
        self.refresh_waiting_state(thread_id);
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

        // 兜底：父会话这一轮已经收尾，它名下的子智能体就不可能还在跑（派活是同步等待的）。
        // 任何遗漏（绑定错、回执缺 id、进程重启后的残留）都不该让子会话的标签页
        // 永远停在"正在分析并执行操作…"——那正是用户看到的"报告出来了却不结束"。
        let children: Vec<String> = self
            .threads
            .iter()
            .filter(|t| {
                t.parent_id.as_deref() == Some(thread_id) && t.is_subagent == Some(true)
            })
            .map(|t| t.id.clone())
            .collect();
        self.running_thread_ids.retain(|tid| !children.contains(tid));
    }
}

/// 一条 item 的**可见指纹**：用来判断磁盘上的会话是否比内存里的更新。
///
/// `agent_proto::Item` 没有实现 `PartialEq`，而这里也不需要逐字节相等——
/// 流式追加只会让文本变长、工具状态只会翻转，所以「id + 文本长度 + 状态 +
/// 完成时刻」就足以识别出"磁盘上有新内容"。
fn item_visible_key(it: &Item) -> (String, usize, String, bool, Option<u64>) {
    match it {
        Item::User { id, text, .. } => (id.clone(), text.len(), String::new(), false, None),
        Item::Thinking { id, text, ended_at, .. } => (
            id.clone(),
            text.len(),
            String::new(),
            ended_at.is_none(),
            *ended_at,
        ),
        Item::Assistant { id, text, streaming, .. } => (
            id.clone(),
            text.len(),
            String::new(),
            *streaming == Some(true),
            None,
        ),
        Item::Tool { id, status, output, duration_ms, .. } => (
            id.clone(),
            output.as_ref().map(|o| o.len()).unwrap_or(0),
            status.clone(),
            false,
            *duration_ms,
        ),
        Item::Notice { id, text, .. } => (id.clone(), text.len(), String::new(), false, None),
        Item::Compact { id, summary, .. } => {
            (id.clone(), summary.len(), String::new(), false, None)
        }
    }
}

/// 两份 item 列表是否有**可见差异**（见 [`item_visible_key`]）。
fn items_visibly_differ(a: &[Item], b: &[Item]) -> bool {
    if a.len() != b.len() {
        return true;
    }
    a.iter()
        .zip(b.iter())
        .any(|(x, y)| item_visible_key(x) != item_visible_key(y))
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
            "appearance": "light",
            "retryCount": 8
        }).to_string()).unwrap();

        // 注入式应用目录：不再依赖 `std::env::set_var`（端口化后那种做法已失效）
        let home = agent_base::testing::TempAppHome::at(&temp_home).with_config_file(&cfg_path);

        let store = AgentStore::with_home("".to_string(), &home);
        assert_eq!(store.config.model, "deepseek/deepseek-v4.1-flash");
        assert_eq!(store.provider.base_url, "http://ai.20301024.xyz:36302/v1");
        assert_eq!(store.provider.api_key, "sk-test-key");
        assert_eq!(store.config.context_window, 65536);
        assert_eq!(store.appearance, "light");
        // 请求重试次数（设置项）：配置文件里写了 8 就得读成 8，而不是永远默认 5
        assert_eq!(store.config.retry_count, 8, "配置里的 retryCount 必须生效");
        // 落盘路径跟着注入的应用目录走——绝不写到用户真实的 ~/.a-da
        assert_eq!(store.config_path, cfg_path);

        let _ = std::fs::remove_dir_all(temp_home);
    }

    /// 老配置文件没有 `retryCount` → 回落到默认值（**不是 0**：0 意味着一次都不重试）。
    #[test]
    fn test_retry_count_defaults_when_absent_and_clamps_when_absurd() {
        let temp_home = std::env::temp_dir().join(format!("a_da_retry_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_home);
        let cfg_path = temp_home.join("config.json");

        std::fs::write(&cfg_path, serde_json::json!({ "model": "m" }).to_string()).unwrap();
        let home = agent_base::testing::TempAppHome::at(&temp_home).with_config_file(&cfg_path);
        let store = AgentStore::with_home("".to_string(), &home);
        assert_eq!(
            store.config.retry_count,
            agent_base::model::DEFAULT_MODEL_RETRY_COUNT as u32,
            "缺失 retryCount 时必须回落到默认值"
        );

        std::fs::write(&cfg_path, serde_json::json!({ "retryCount": 9999 }).to_string()).unwrap();
        let store2 = AgentStore::with_home("".to_string(), &home);
        assert_eq!(
            store2.config.retry_count,
            agent_base::model::MAX_MODEL_RETRY_COUNT as u32,
            "越界的 retryCount 必须夹到上限"
        );

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
        store.finish_tool_call(tid, "call_ui_1", true, Some("done".into()), Some(1), Some(0), Some(1), None);
        let after = status_of(&store);
        assert_ne!(after, "waiting_approval", "工具完成后不得仍停在等待批准：{after}");
    }

    /// 守门：`waiting_thread_ids` 必须如实反映"哪个会话在等你操作"。
    ///
    /// 这个字段以前只初始化、只进快照，**从来没被填过**——实测一条 `cargo test` 的审批
    /// 请求静默等了 5 分钟超时被自动拒绝，用户在别的标签页上完全不知道发生过。
    #[test]
    fn test_waiting_thread_ids_track_pending_approval_and_question() {
        let mut store = AgentStore::new("E:/codes/waiting_ws".to_string());
        let tid = store.create_thread(Some("等待态".to_string()), None);
        let tid = tid.as_str();
        let is_waiting = |s: &AgentStore| s.waiting_thread_ids.iter().any(|id| id == tid);

        // 1. 审批等待
        store.start_tool_call(tid, "call_w1", "run_command", "{}");
        assert!(!is_waiting(&store), "刚发起还不算等待");
        store.set_tool_waiting_approval(tid, "call_w1", "run_command");
        assert!(is_waiting(&store), "等待批准必须能被界面全局看见");

        store.finish_tool_call(tid, "call_w1", true, Some("ok".into()), Some(1), Some(0), Some(1), None);
        assert!(!is_waiting(&store), "答复/完成后必须摘掉，否则提示会一直挂着");

        // 2. 提问等待同理
        store.start_tool_call(tid, "call_q1", "ask_user", "{}");
        store.set_tool_awaiting_question(
            tid,
            "call_q1",
            serde_json::json!({"question": "选哪个？", "choices": []}),
        );
        assert!(is_waiting(&store), "等待用户答复同样属于'等你操作'");
        store.finish_tool_call(tid, "call_q1", true, Some("ok".into()), Some(1), Some(0), Some(1), None);
        assert!(!is_waiting(&store));
    }

    /// 守门：审批事件到达时工具卡片**还不存在**，也必须把卡片补出来。
    ///
    /// 引擎的真实顺序是：`ApprovalRequested` → 等用户 →（通过后）`ToolCallStarted`。
    /// 也就是说建卡点（`start_tool_call`）**在审批之后**，`set_tool_waiting_approval`
    /// 必然找不到已存在的卡片。早先它只"改已存在的卡片"，于是静默 no-op：
    /// 界面没有审批卡、`waiting_thread_ids` 为空，用户无从批准，只能干等 30 分钟超时被拒。
    /// （实测：一条含 `Format-Table` 的只读列目录命令就这样把整轮卡死。）
    #[test]
    fn test_approval_creates_the_tool_card_when_it_does_not_exist_yet() {
        let mut store = AgentStore::new("E:/codes/approval_card_ws".to_string());
        let tid = store.create_thread(Some("审批补卡".to_string()), None);
        let tid = tid.as_str();

        // 取某 call_id 的卡片快照：(状态, 工具名, details)
        fn cards(
            store: &AgentStore,
            tid: &str,
            call_id: &str,
        ) -> Vec<(String, String, serde_json::Value)> {
            store
                .threads
                .iter()
                .find(|t| t.id == tid)
                .map(|t| {
                    t.items
                        .iter()
                        .filter_map(|it| match it {
                            Item::Tool { call_id: cid, status, name, details, .. }
                                if cid == call_id =>
                            {
                                Some((
                                    status.clone(),
                                    name.clone(),
                                    details.clone().unwrap_or(serde_json::Value::Null),
                                ))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        }

        // 审批先到：此时**没有任何**工具卡片
        store.set_tool_waiting_approval(tid, "call_a1", "run_command");

        let waiting = cards(&store, tid, "call_a1");
        assert_eq!(waiting.len(), 1, "必须补出一张卡，而不是静默 no-op");
        assert_eq!(waiting[0].0, "waiting_approval");
        assert_eq!(waiting[0].1, "run_command");
        assert_eq!(
            waiting[0].2["approval"]["pending"],
            serde_json::Value::Bool(true),
            "卡片要带上'待批准'标记，界面据此渲染批准/拒绝按钮"
        );
        assert!(
            store.waiting_thread_ids.iter().any(|id| id == tid),
            "补出来的卡片也必须让'有会话在等你操作'可见"
        );

        // 审批通过后 `ToolCallStarted` 到达：**复用**这张卡，不得出现同 call_id 的两张
        store.start_tool_call(tid, "call_a1", "run_command", r#"{"command":"ls"}"#);
        let running = cards(&store, tid, "call_a1");
        assert_eq!(running.len(), 1, "同一个 call_id 只能有一张卡，否则完成事件只会闭合最后一张");
        assert_eq!(
            (running[0].0.as_str(), running[0].1.as_str()),
            ("running", "run_command")
        );
        assert!(
            !store.waiting_thread_ids.iter().any(|id| id == tid),
            "开始执行后不再是'等待中'"
        );

        // 收尾仍然能命中这张卡
        store.finish_tool_call(tid, "call_a1", true, Some("ok".into()), Some(3), Some(0), Some(3), None);
        assert_eq!(cards(&store, tid, "call_a1")[0].0, "done");
    }

    /// 守门：**运行中的子智能体会话必须继续从磁盘回流内容**。
    ///
    /// 子智能体跑在自己的运行时里（跨网关委派时甚至是另一个进程），本进程的 store
    /// 从不直接往它的 item 上追加 delta——磁盘会话文件是它**唯一**的真源。
    /// 早先 `sync_threads_from_disk` 用「运行中」一刀切跳过合并，结果子会话整轮
    /// （实测 4.5 分钟）停在创建那一刻的空壳：父会话的委派卡片与子会话标签页
    /// **整轮没有任何实时内容**，用户只能盯着一张静止的卡片，无法判断子智能体
    /// 是在干活还是卡死了。
    #[test]
    fn test_running_subagent_thread_still_merges_disk_content() {
        use agent_base::domain::AgentMessage;

        let sessions_root = std::env::temp_dir()
            .join(format!("a_da_store_subagent_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&sessions_root);
        let mgr = agent_node::session::SessionManager::new(Some(sessions_root.clone()));
        let ws = "E:/codes/go_projects/aluka_cloud";
        let parent = "thread_parent_1";
        let child = "thread_child_1";

        let assistant_msg = |text: &str, ts: i64| AgentMessage::Assistant {
            content: text.to_string(),
            thinking: None,
            tool_calls: None,
            stop_reason: Some("tool_calls".to_string()),
            error_message: None,
            timestamp: Some(ts),
            usage: None,
            duration_ms: None,
            turn_duration_ms: None,
        };

        mgr.create_session(parent, ws, Some("父会话"), None, None).unwrap();
        mgr.create_session(child, ws, Some("子代理 [general_purpose]"), Some(parent), Some("general_purpose"))
            .unwrap();

        let mut store = AgentStore::new(ws.to_string());
        // 1. 子会话刚建好就被发现——此刻它还没有任何内容
        assert!(store.sync_threads_from_disk(&mgr), "新会话应被发现");
        assert!(
            store
                .threads
                .iter()
                .any(|t| t.id == child && t.is_subagent == Some(true)),
            "带 parentId 的会话必须被认成子智能体会话"
        );

        // 2. 它正在跑：进入运行态（界面据此把它算作"运行中的子智能体"）
        store.set_thread_running(child, true);

        // 3. 子智能体在自己的运行时里产出并落盘 → 必须回流到 store
        mgr.append_message(child, assistant_msg("第一步：枚举 go.mod", 1), Some(ws))
            .unwrap();
        assert!(store.sync_threads_from_disk(&mgr), "运行中的子会话有新内容，必须回流");
        assert_eq!(
            store.threads.iter().find(|t| t.id == child).map(|t| t.items.len()),
            Some(1),
            "运行中的子会话必须拿到磁盘上的新 item，否则界面整轮空白"
        );

        // 4. 继续产出：内容必须继续增长（这就是界面上的实时进展）
        mgr.append_message(child, assistant_msg("第二步：读 replace 指令", 2), Some(ws))
            .unwrap();
        assert!(store.sync_threads_from_disk(&mgr));
        assert_eq!(
            store.threads.iter().find(|t| t.id == child).map(|t| t.items.len()),
            Some(2),
            "子会话的 item 必须随磁盘继续增长"
        );

        // 5. 磁盘没变化时不得每 1.5s 谎报 changed（否则快照被无谓地反复广播）
        assert!(
            !store.sync_threads_from_disk(&mgr),
            "磁盘无变化时不应报告 changed"
        );

        // 6. 反例保护：本进程**正在流式吐字**的父会话不得被磁盘内容覆盖
        store.set_thread_running(parent, true);
        mgr.append_message(
            parent,
            AgentMessage::User { content: "磁盘上的内容".to_string(), images: None, timestamp: Some(3) },
            Some(ws),
        )
        .unwrap();
        let _ = store.sync_threads_from_disk(&mgr);
        assert_eq!(
            store.threads.iter().find(|t| t.id == parent).map(|t| t.items.len()),
            Some(0),
            "运行中的父会话由本进程自己吐字，不得被磁盘合并覆盖"
        );

        // 7. 一旦停下，磁盘内容照常回流（保护只针对"运行中"）
        store.set_thread_running(parent, false);
        assert!(store.sync_threads_from_disk(&mgr));
        assert_eq!(
            store.threads.iter().find(|t| t.id == parent).map(|t| t.items.len()),
            Some(1)
        );

        let _ = std::fs::remove_dir_all(&sessions_root);
    }

    /// 守门：派活时只能认领**本次新建**的子会话。
    ///
    /// 用「任取一个 parent 匹配的子会话」的写法，第二次派活会把**上一次已经结束**的子会话
    /// 重新标成运行中（并把它绑到新卡片上）——界面表现：报告都出来了，子会话却一直
    /// 显示"正在分析并执行操作…"。实测就是这条：子A 报告完成、父会话随后派出子B，
    /// 子A 被重新点亮。
    #[test]
    fn test_pick_new_subagent_child_never_claims_an_old_one() {
        use agent_base::domain::AgentMessage;

        let sessions_root = std::env::temp_dir()
            .join(format!("a_da_store_pick_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&sessions_root);
        let mgr = agent_node::session::SessionManager::new(Some(sessions_root.clone()));
        let ws = "E:/codes/go_projects/aluka_cloud";
        let parent = "thread_p1";
        let child_a = "thread_c_a";
        let child_b = "thread_c_b";

        mgr.create_session(parent, ws, Some("父会话"), None, None).unwrap();
        mgr.create_session(child_a, ws, Some("子代理 A"), Some(parent), Some("researcher")).unwrap();
        mgr.append_message(
            child_a,
            AgentMessage::User { content: "A 的活".to_string(), images: None, timestamp: Some(1) },
            Some(ws),
        )
        .unwrap();

        let mut store = AgentStore::new(ws.to_string());
        assert!(store.sync_threads_from_disk(&mgr), "父/子会话都应被发现");

        // A 派活当时：已知集合为空 → 认领 A
        assert_eq!(
            store.pick_new_subagent_child(parent, &[], &[]).as_deref(),
            Some(child_a)
        );

        // A 已经结束、A 的 id 进入"已知"：此时再派活，绝不能又认领 A
        let known = vec![child_a.to_string()];
        assert_eq!(
            store.pick_new_subagent_child(parent, &known, &[]),
            None,
            "已知的子会话不是本次派活的产物，不得认领"
        );

        // 第二次派活新建了 B → 必须认领 B（而不是 A）
        mgr.create_session(child_b, ws, Some("子代理 B"), Some(parent), Some("researcher")).unwrap();
        assert!(store.sync_threads_from_disk(&mgr));
        assert_eq!(
            store.pick_new_subagent_child(parent, &known, &[]).as_deref(),
            Some(child_b),
            "第二次派活必须认领新建的 B"
        );

        // 已被别的运行中卡片绑定的子会话也不再认领
        let bound = vec![child_b.to_string()];
        assert_eq!(store.pick_new_subagent_child(parent, &known, &bound), None);

        // 兜底：父会话收尾时，名下子会话的运行态必须一并解除
        store.set_thread_running(parent, true);
        store.set_thread_running(child_b, true);
        store.finish_turn(parent);
        assert!(
            !store.running_thread_ids.iter().any(|t| t == child_b),
            "父会话收尾后，子会话不得还挂在运行态（否则标签页永远显示'正在执行'）"
        );

        let _ = std::fs::remove_dir_all(&sessions_root);
    }
}

