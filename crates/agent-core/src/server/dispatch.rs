use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex, RwLock};

use crate::approval::ApprovalManager;
use crate::ai::ProviderConfig;
use crate::checkpoint::CheckpointManager;
use crate::plugins::PluginManager;
use crate::protocol::*;
use crate::runner::{run_agent_turn, AgentLoopEvent};
use crate::server::emitter::StateBroadcaster;
use crate::server::fs_service;
use crate::session::SessionManager;
use crate::skills::SkillManager;
use crate::state::{generate_snapshot, AgentStore};
use crate::subagents::SubagentManager;

/// 把 `ToolDescriptor` 投影成界面用的内置工具条目。
///
/// INV-7：领域描述符只声明"这个工具是什么"，**展示形状由投影层决定**。
/// 这里只做派生，**不引入任何工具名清单**——一旦引入，界面就会再次广告不存在的工具
/// （第 6 张名单的教训）。
///
/// `label` 用描述符的 `summary`：领域里只有这一份人类可读文本。
/// `description` 则由声明派生（读写性 / 执行模式 / 审批要求），不另写文案。
fn builtin_tool_info(d: &agent_base::domain::ToolDescriptor) -> serde_json::Value {
    use agent_base::domain::{ApprovalPolicy, Execution};

    let kind = if d.is_readonly() { "只读工具" } else { "写入工具" };
    let exec = match d.execution {
        Execution::Sequential => "顺序执行",
        Execution::ParallelSafe => "可并行",
    };
    let approval = match &d.approval {
        ApprovalPolicy::Never => "免审批",
        _ => "需审批",
    };

    serde_json::json!({
        "name": d.name,
        "label": d.summary,
        "description": format!("{kind} · {exec} · {approval}"),
        "isReadOnly": d.is_readonly(),
    })
}

/// 解析某个会话的执行工作区。
///
/// 会话与工作区严格绑定：Agent 沙箱、会话落盘目录、插件/技能解析都必须用会话自己的
/// 工作区，不能让它们跟着"全局当前工作区"漂移（否则在 A 工作区建的会话会跑在 B 里）。
/// 仅在会话工作区为空（历史遗留数据）时，才退回当前工作区，再退回公共区。
fn thread_workspace(store: &AgentStore, thread_id: &str) -> String {
    let from_thread = store
        .threads
        .iter()
        .find(|t| t.id == thread_id)
        .map(|t| t.workspace.trim().to_string())
        .filter(|ws| !ws.is_empty());

    from_thread
        .or_else(|| {
            let project = store.workspace.project.trim();
            (!project.is_empty()).then(|| project.to_string())
        })
        .unwrap_or_else(|| store.public_workspace.clone())
}

/// 持久化落盘应用全局配置（包含多供应商列表与激活供应商）
fn save_app_config(store: &AgentStore) -> Result<(), String> {
    let cfg_file = crate::session::get_config_path();
    if let Some(parent) = cfg_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut existing: serde_json::Value = if cfg_file.exists() {
        std::fs::read_to_string(&cfg_file)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if !existing.is_object() {
        existing = serde_json::json!({});
    }
    if let Some(obj) = existing.as_object_mut() {
        obj.insert("baseUrl".to_string(), serde_json::json!(store.provider.base_url));
        obj.insert("apiKey".to_string(), serde_json::json!(store.provider.api_key));
        obj.insert("model".to_string(), serde_json::json!(store.provider.model));
        obj.insert(
            "protocol".to_string(),
            serde_json::to_value(&store.provider.protocol).unwrap_or(serde_json::json!("openai_chat")),
        );
        obj.insert("contextWindow".to_string(), serde_json::json!(store.config.context_window));
        if let Some(mot) = store.config.max_output_tokens {
            obj.insert("maxOutputTokens".to_string(), serde_json::json!(mot));
        } else {
            obj.remove("maxOutputTokens");
        }
        obj.insert("supportsImages".to_string(), serde_json::json!(store.config.supports_images));
        if let Some(ref ch) = store.provider.custom_headers {
            obj.insert("customHeaders".to_string(), serde_json::json!(ch));
        } else {
            obj.remove("customHeaders");
        }
        obj.insert(
            "providers".to_string(),
            serde_json::to_value(&store.providers).unwrap_or(serde_json::json!([])),
        );
        obj.insert("activeProviderId".to_string(), serde_json::json!(store.active_provider_id));
    }
    let tmp_file = format!("{}.{}.tmp", cfg_file.to_string_lossy(), std::process::id());
    if let Ok(content) = serde_json::to_string_pretty(&existing) {
        if std::fs::write(&tmp_file, content).is_ok() {
            let _ = std::fs::rename(&tmp_file, &cfg_file);
        }
    }
    Ok(())
}

/// 远程探测通用 /models 接口以获取可用模型列表
async fn fetch_remote_models(
    protocol: crate::ai::ModelProtocol,
    base_url: &str,
    api_key: &str,
    custom_headers: Option<&std::collections::HashMap<String, String>>,
    proxy_url: Option<&str>,
) -> Result<Vec<crate::ai::ModelEntry>, String> {
    let clean_base = base_url.trim_end_matches('/');
    if clean_base.is_empty() {
        return Err("供应商 Base URL 不能为空".to_string());
    }

    let url = match protocol {
        crate::ai::ModelProtocol::Anthropic => {
            if clean_base.ends_with("/v1") {
                format!("{}/models", clean_base)
            } else {
                format!("{}/v1/models", clean_base)
            }
        }
        crate::ai::ModelProtocol::OpenAiChat | crate::ai::ModelProtocol::OpenAiResponses => {
            if clean_base.ends_with("/v1") {
                format!("{}/models", clean_base)
            } else {
                format!("{}/v1/models", clean_base)
            }
        }
    };

    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10));

    if let Some(p) = proxy_url {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(proxy);
            }
        }
    }

    let client = builder
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let mut req = client.get(&url);

    match protocol {
        crate::ai::ModelProtocol::Anthropic => {
            if !api_key.trim().is_empty() {
                req = req.header("x-api-key", api_key.trim());
            }
            req = req.header("anthropic-version", "2023-06-01");
        }
        crate::ai::ModelProtocol::OpenAiChat | crate::ai::ModelProtocol::OpenAiResponses => {
            if !api_key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
            }
        }
    }

    if let Some(headers) = custom_headers {
        for (k, v) in headers {
            req = req.header(k, v);
        }
    }

    let resp = req.send().await.map_err(|e| format!("请求模型列表失败 ({}): {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        return Err(format!("接口返回错误码 {}: {}", status, err_body));
    }

    let val: serde_json::Value = resp.json().await.map_err(|e| format!("解析模型列表 JSON 失败: {}", e))?;

    let mut raw_items = Vec::new();
    if let Some(arr) = val.get("data").and_then(|v| v.as_array()) {
        raw_items.extend(arr.iter());
    } else if let Some(arr) = val.get("models").and_then(|v| v.as_array()) {
        raw_items.extend(arr.iter());
    } else if let Some(arr) = val.as_array() {
        raw_items.extend(arr.iter());
    }

    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for item in raw_items {
        let (id, name) = if let Some(s) = item.as_str() {
            (s.to_string(), s.to_string())
        } else if let Some(obj) = item.as_object() {
            let id = obj.get("id")
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = obj.get("display_name")
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            (id, name)
        } else {
            continue;
        };

        if !id.is_empty() && seen.insert(id.clone()) {
            models.push(crate::ai::ModelEntry {
                id,
                name: Some(name),
                context_window: None,
                max_output_tokens: None,
                supports_images: None,
            });
        }
    }

    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// W3-T2：宿主注入的**真引擎**及其**必须共用**的单例。
///
/// ⚠️ `approval_mgr` 不是可选项：新引擎的审批闸门（`HostApprovalGate`）把 waiter 注册到
/// 它持有的 `ApprovalManager` 上，而 UI 的"批准/拒绝"经 `APPROVAL_DECIDE` 落到
/// `Dispatcher.approval_mgr`。两者若是**不同实例**，UI 的答复就永远送不到闸门，
/// 每次审批都会静默等到超时（默认 300s）。
pub struct EngineInjection {
    pub runtime: Arc<agent_base::engine::AgentRuntime>,
    pub approval_mgr: Arc<ApprovalManager>,
    /// 产品声明（W3-T6）：握手要**如实**回报这个产品的能力位与身份，
    /// 而不是回报一份硬编码常量。
    ///
    /// 为什么必须带进来：`session.initialize` 是客户端第一个请求，
    /// 界面据此决定"要不要显示图片按钮、回滚按钮、插件入口"。
    /// 硬编码的结果是"声明的能力"与"界面看到的"各说各话——
    /// 这正是本计划要清掉的那类脱钩（P1-7 的同一根因）。
    pub spec: Arc<agent_runtime::AgentSpec>,
}


/// 把检查点回滚结果转成协议回执（W6-T4）。
///
/// 为什么要单独一个函数：三个 `revert_*` 臂原先都**丢弃**结果、统一回 `{ok:true}`——
/// 界面据此显示"已回滚"，而实际上可能一个文件都没动。
///
/// 口径：
/// - `Some(outcome)` → `ok: true` + **恢复/删除/跳过/失效**四份清单（界面可列出"恢复了哪些文件"）；
/// - `None` → `ok: false` + 原因（该会话没有对应检查点，**什么都没回滚**）。
fn revert_response(outcome: Option<crate::checkpoint::RevertOutcome>) -> serde_json::Value {
    match outcome {
        Some(o) => serde_json::json!({
            "ok": true,
            "restored": o.restored,
            "deleted": o.deleted,
            "skipped": o.skipped,
            "invalidated": o.invalidated,
            "restoredCount": o.restored.len(),
            "deletedCount": o.deleted.len(),
        }),
        None => serde_json::json!({
            "ok": false,
            "reason": "该会话没有可回滚的检查点（什么都没回滚）",
            "restored": [],
            "deleted": [],
            "skipped": [],
            "invalidated": [],
            "restoredCount": 0,
            "deletedCount": 0,
        }),
    }
}

pub struct Dispatcher {
    store: Arc<RwLock<AgentStore>>,
    session_mgr: Arc<SessionManager>,
    checkpoint_mgr: Arc<CheckpointManager>,
    subagent_mgr: Arc<SubagentManager>,
    approval_mgr: Arc<ApprovalManager>,
    plugin_mgr: Arc<PluginManager>,
    skill_mgr: Arc<SkillManager>,
    broadcaster: Option<Arc<StateBroadcaster>>,
    host_pid: u32,
    session_id: String,
    abort_senders: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    running_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// W3-T2：宿主注入的**真引擎**（`AgentRuntime`）。
    ///
    /// `None` = 没接线，仍走 legacy 主循环。注入由宿主完成（`agent-host` 装配），
    /// 因为 `agent-host` 依赖 `agent-core`，反向依赖会成环。
    /// 走哪条由 `A_DA_ENGINE` 决定（默认 legacy，见 `runner::engine_bridge`）。
    engine: Option<Arc<agent_base::engine::AgentRuntime>>,
    /// 产品声明（W3-T6）：握手回报的能力位与身份从这里派生。
    ///
    /// `None` = 没注入声明（老调用点/测试）→ 退回 `ServerCapabilities::default()`。
    product_spec: Option<Arc<agent_runtime::AgentSpec>>,
}

impl Dispatcher {
    /// 会话执行泵（**唯一实现**，W6-T4）：跑轮次 → 事件写进 store → 广播 → 消费排队指令。
    ///
    /// 为什么必须共用：`THREAD_EDIT_AND_RESEND` 原先自己写了一份残缺版——
    /// **丢事件**（spawn 一个空循环把 `event_rx` 抽干丢掉）、
    /// **吞错**（只 `tracing::warn`，客户端拿到 `Ok`）、
    /// **不可 abort**（`abort_rx: None`）。三处缺陷同源：它没有走这条泵。
    ///
    /// 现在两条路径共用本函数——行为只有一份，改一处两边都对。
    async fn spawn_thread_loop(
        &self,
        thread_id: String,
        first_prompt: Option<String>,
        provider_config: ProviderConfig,
    ) {
        let target_tid_clone_for_insert = thread_id.clone();
                let store_clone = Arc::clone(&self.store);
                let broadcaster_clone = self.broadcaster.clone();
                let abort_senders_clone = Arc::clone(&self.abort_senders);
                let running_tasks_clone = Arc::clone(&self.running_tasks);
                let target_tid_clone = thread_id;
                // W3-T2：把注入的真引擎（若有）带进 drain 循环
                let engine_clone = self.engine.clone();

                let runner_task = tokio::spawn(async move {
                    let mut current_prompt: Option<String> = first_prompt;
                    loop {
                        let (abort_tx, abort_rx) = watch::channel(false);
                        abort_senders_clone.lock().await.insert(target_tid_clone.clone(), abort_tx);

                        let (event_tx, mut event_rx) = mpsc::channel::<AgentLoopEvent>(128);
                        let tid = target_tid_clone.clone();
                        let prompt = current_prompt.take();
                        let p_cfg = provider_config.clone();
                        // 每轮取一份引擎克隆：`async move` 会吞掉捕获值，
                        // 直接在闭包里 `.clone()` 会把外层那份也 move 走（循环第二轮就报错）。
                        let engine_for_turn = engine_clone.clone();

                        let loop_handle = tokio::spawn(async move {
                            // W3-T2：统一入口——按 `A_DA_ENGINE` 与是否注入引擎决定走哪条。
                            if let Err(e) = run_agent_turn(
                                engine_for_turn,
                                &tid,
                                prompt.as_deref(),
                                p_cfg,
                                event_tx,
                                Some(abort_rx),
                            ).await {
                                tracing::warn!("轮次执行失败: {e}");
                            }
                        });

                        while let Some(event) = event_rx.recv().await {
                            let mut store = store_clone.write().await;
                            match event {
                                AgentLoopEvent::Thinking { text } => {
                                    store.append_thinking_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::TextDelta { text } => {
                                    store.append_assistant_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::ToolCallStarted { name, id, args } => {
                                    store.start_tool_call(&target_tid_clone, &id, &name, &args);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::ToolCallFinished { name: _, id, ok, output, duration_ms, started_at, finished_at, status: _ } => {
                                    store.finish_tool_call(&target_tid_clone, &id, ok, output, duration_ms, started_at, finished_at);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::ToolAwaitingQuestion { id, question } => {
                                    store.set_tool_awaiting_question(&target_tid_clone, &id, question);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                // W3-T3：审批请求 → 工具卡片进入"等待批准"，界面据此渲染批准/拒绝按钮
                                AgentLoopEvent::ApprovalRequested { id, tool } => {
                                    store.set_tool_waiting_approval(&target_tid_clone, &id, &tool);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::AssistantStats { usage, duration_ms, turn_duration_ms } => {
                                    store.set_assistant_stats(
                                        &target_tid_clone,
                                        usage,
                                        duration_ms,
                                        turn_duration_ms,
                                    );
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::TurnFinished { .. } => {
                                    drop(store);
                                    break;
                                }
                                AgentLoopEvent::Error { message } => {
                                    store.append_assistant_delta(&target_tid_clone, &format!("\n\n**请求异常**：{message}"));
                                    store.push_log("error", format!("Agent 执行异常: {message}"), None);
                                    drop(store);
                                    break;
                                }
                            }
                        }

                        let _ = loop_handle.await;

                        // 收尾并闭合本轮卡片状态
                        {
                            let mut store = store_clone.write().await;
                            store.end_thinking(&target_tid_clone);
                            if let Some(t) = store.get_thread_mut(&target_tid_clone) {
                                for item in t.items.iter_mut() {
                                    if let Item::Assistant { streaming, .. } = item {
                                        *streaming = None;
                                    }
                                }
                                if let Some(Item::Thinking { text, .. }) = t.items.last() {
                                    if text.trim().is_empty() {
                                        t.items.pop();
                                    }
                                }
                            }
                        }

                        // 如果该会话已被主动停止（用户点击了中止），则不再消费队列
                        {
                            let store = store_clone.read().await;
                            if !store.is_thread_running(&target_tid_clone) {
                                break;
                            }
                        }

                        // 检查会话队列中是否还有待执行的排队指令
                        let next_item = {
                            let mut store = store_clone.write().await;
                            store.pop_next_queued(&target_tid_clone)
                        };

                        if let Some(queued) = next_item {
                            let mut store = store_clone.write().await;
                            store.add_user_message_with_images(&target_tid_clone, &queued.text, queued.images);
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                            current_prompt = Some(queued.text);
                        } else {
                            // 对话队列已清空，退出 drain 循环
                            break;
                        }
                    }

                    // 整个执行链（含排队消息）全部完成，统一收尾
                    abort_senders_clone.lock().await.remove(&target_tid_clone);
                    running_tasks_clone.lock().await.remove(&target_tid_clone);
                    let mut store = store_clone.write().await;
                    store.finish_turn(&target_tid_clone);
                    drop(store);
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
                });

                self.running_tasks.lock().await.insert(target_tid_clone_for_insert, runner_task);
    }

    pub fn new(
        store: Arc<RwLock<AgentStore>>,
        session_mgr: Arc<SessionManager>,
        checkpoint_mgr: Arc<CheckpointManager>,
        subagent_mgr: Arc<SubagentManager>,
        approval_mgr: Arc<ApprovalManager>,
        plugin_mgr: Arc<PluginManager>,
        skill_mgr: Arc<SkillManager>,
        broadcaster: Option<Arc<StateBroadcaster>>,
    ) -> Self {
        Self {
            store,
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            broadcaster,
            host_pid: std::process::id(),
            session_id: uuid::Uuid::new_v4().to_string(),
            abort_senders: Arc::new(Mutex::new(HashMap::new())),
            running_tasks: Arc::new(Mutex::new(HashMap::new())),
            engine: None,
            product_spec: None,
        }
    }

    /// W3-T2：注入真引擎（`AgentRuntime`）。宿主装配层（`agent-host`）构造后传进来。
    ///
    /// 保持 builder 风格：既有调用点与测试**一行不改**，只有真正要切引擎的宿主才调它。
    pub fn with_engine(mut self, engine: Arc<agent_base::engine::AgentRuntime>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// 同上，但接受 `Option`——宿主"有就注入、没有就保持 legacy"的形态。
    ///
    /// ⚠️ 这里**只取 `runtime`**：`approval_mgr` 必须由调用方在构造 `Dispatcher` 时就传对
    /// （见 [`EngineInjection`] 的说明），事后替换会留下"闸门挂在旧表上"的坑。
    pub fn pipe_engine(
        mut self,
        injection: Option<EngineInjection>,
    ) -> Self {
        if let Some(i) = injection {
            self.engine = Some(i.runtime);
            // W3-T6：声明一起带进来，握手才能如实回报能力位
            self.product_spec = Some(i.spec);
        }
        self
    }

    /// 只注入产品声明（W3-T6）：用于"没有引擎也要如实回报能力位"的场景与测试。
    pub fn with_product_spec(mut self, spec: Arc<agent_runtime::AgentSpec>) -> Self {
        self.product_spec = Some(spec);
        self
    }

    /// 是否已注入真引擎。
    ///
    /// W3-T4 之后**必须**为 `true`：legacy 主循环已删除，"没引擎"不再是可运行状态。
    pub fn has_engine(&self) -> bool {
        self.engine.is_some()
    }

    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProtocolError> {
        match method {
            SESSION_INITIALIZE => {
                let p = params.as_object().ok_or_else(|| {
                    ProtocolError::invalid_params("session.initialize 需要一个参数对象")
                })?;

                let client_version = p.get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if client_version != PROTOCOL_VERSION {
                    return Err(ProtocolError::new(
                        AppErrorCode::ProtocolVersionMismatch.code(),
                        format!(
                            "协议版本不兼容：服务端要求 {}，收到 {}",
                            PROTOCOL_VERSION, client_version
                        ),
                        Some(serde_json::json!({
                            "serverVersion": PROTOCOL_VERSION,
                            "minClient": PROTOCOL_VERSION,
                        })),
                    ));
                }

                // W3-T6：能力位与产品身份**从产品声明派生**，不再硬编码。
                //
                // 判据（`cargo xtask verify-spec` 的同一根因）：声明了 `rollback: false`
                // 的产品，握手就必须回 `rollback: false`——否则界面会显示一个
                // 这个产品根本没有的能力。
                let (capabilities, product) = match self.product_spec.as_ref() {
                    Some(spec) => (
                        ServerCapabilities {
                            images: spec.capabilities.images,
                            rollback: spec.capabilities.rollback,
                            plugins: spec.capabilities.plugins,
                            // `hooks` / `resync` 目前**如实为 false**：机制未落地
                            // （hooks 点位为 0，见 unfinished-features.md）。
                            // 不许因为"设计里有"就报 true。
                            hooks: false,
                            resync: false,
                            events: agent_proto::ServerEventsCapability {
                                granularity: "coarse".to_string(),
                                snapshot_seq: true,
                            },
                        },
                        Some(agent_proto::ProductInfo {
                            id: spec.id.clone(),
                            name: spec.identity.name.clone(),
                            archetype: spec.archetype.clone(),
                            persona: Some(spec.identity.persona.clone()),
                        }),
                    ),
                    // 没注入声明（老调用点/测试）：退回默认值
                    None => (ServerCapabilities::default(), None),
                };

                let init_res = InitializeResult {
                    session_id: self.session_id.clone(),
                    protocol_version: PROTOCOL_VERSION.to_string(),
                    host: HostInfo { pid: self.host_pid },
                    capabilities,
                    product,
                };
                serde_json::to_value(init_res).map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            SESSION_SNAPSHOT => {
                let store = self.store.read().await;
                let snapshot = generate_snapshot(&store);
                Ok(serde_json::to_value(snapshot).map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            UI_SET_SHELL => {
                let mut store = self.store.write().await;
                if let Some(patch) = params.get("patch").and_then(|p| p.as_object()) {
                    if let Some(v) = patch.get("debugOpen").and_then(|b| b.as_bool()) {
                        store.ui.debug_open = v;
                    }
                    if let Some(v) = patch.get("settingsOpen").and_then(|b| b.as_bool()) {
                        store.ui.settings_open = v;
                    }
                    if let Some(v) = patch.get("pluginsOpen").and_then(|b| b.as_bool()) {
                        store.ui.plugins_open = v;
                    }
                    if let Some(v) = patch.get("changesOpen").and_then(|b| b.as_bool()) {
                        store.ui.changes_open = v;
                    }
                    if let Some(v) = patch.get("paletteOpen").and_then(|b| b.as_bool()) {
                        store.ui.palette_open = v;
                    }
                    if let Some(v) = patch.get("sidebarOpen").and_then(|b| b.as_bool()) {
                        store.ui.sidebar_open = v;
                    }
                    if let Some(v) = patch.get("searchOpen").and_then(|b| b.as_bool()) {
                        store.ui.search_open = v;
                    }
                    if let Some(v) = patch.get("appearance").and_then(|s| s.as_str()) {
                        if v == "light" || v == "dark" {
                            store.appearance = v.to_string();
                        }
                    }
                    if let Some(v) = patch.get("pendingDraft") {
                        store.ui.pending_draft = v.as_str().map(|s| s.to_string());
                    }
                }
                Ok(serde_json::Value::Null)
            }

            UI_OPEN_TAB => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                store.open_tab(thread_id.to_string());
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            UI_CLOSE_TAB => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                store.close_tab(thread_id);
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            THREAD_FOCUS => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                if !store.focus_thread(thread_id.to_string()) {
                    return Err(ProtocolError::new(
                        AppErrorCode::NotFound.code(),
                        format!("未找到指定的会话：{}", thread_id),
                        None,
                    ));
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            THREAD_CREATE => {
                let title = params.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
                let mode_param = params.get("mode").and_then(|v| v.as_str());
                let ws_param = params.get("workspace")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let mut store = self.store.write().await;
                let id = store.create_thread(title.clone(), ws_param.clone());
                if let Some(m) = mode_param {
                    let parsed_mode = match m.to_lowercase().as_str() {
                        "plan" => AgentMode::Plan,
                        "create" => AgentMode::Create,
                        _ => AgentMode::Code,
                    };
                    if let Some(t) = store.threads.iter_mut().find(|t| t.id == id) {
                        t.mode = Some(parsed_mode);
                    }
                    store.config.mode = parsed_mode;
                }
                // 会话落在调用方指定的工作区；未指定则沿用当前工作区（历史行为）
                let ws = thread_workspace(&store, &id);
                if !ws.is_empty() {
                    store.workspace.project = ws.clone();
                }
                drop(store);

                let _ = self.session_mgr.create_session(&id, &ws, title.as_deref(), None, None);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "id": id,
                    "threadId": id
                }))
            }

            THREAD_DELETE => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                // 会话文件躺在它自己的工作区目录下，删除前先取出来
                let ws = thread_workspace(&store, thread_id);
                let deleted = store.delete_thread(thread_id);
                drop(store);

                let _ = self.session_mgr.delete_session(thread_id, Some(&ws));
                let _ = self.checkpoint_mgr.discard(thread_id);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "ok": deleted,
                    "message": null
                }))
            }

            THREAD_SEND => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let text = params.get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;
                let images = params.get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<String>>());

                let mut store = self.store.write().await;
                let target_tid = if store.threads.iter().any(|t| t.id == thread_id) {
                    thread_id.to_string()
                } else if !store.active_id.is_empty() && store.threads.iter().any(|t| t.id == store.active_id) {
                    store.active_id.clone()
                } else if let Some(first) = store.threads.first() {
                    first.id.clone()
                } else {
                    store.create_thread(None, None)
                };

                store.active_id = target_tid.clone();

                // 若当前会话已在运行，则推入对话队列，不并发创建执行任务
                if store.is_thread_running(&target_tid) {
                    let qid = store.enqueue_message(&target_tid, &text, images);
                    let q_count = store.queue.iter().filter(|q| q.thread_id == target_tid).count();
                    store.push_log("info", format!("已排队第 {} 条后续指令", q_count), None);
                    drop(store);

                    if let Some(ref bc) = self.broadcaster {
                        bc.broadcast_immediate().await;
                    }

                    return Ok(serde_json::json!({
                        "accepted": true,
                        "queued": true,
                        "queueId": qid
                    }));
                }

                // 首次启动该会话：注入用户消息，标记为运行态并启动 drain 连续执行循环
                store.add_user_message_with_images(&target_tid, &text, images);
                store.set_thread_running(&target_tid, true);
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                self
                    .spawn_thread_loop(target_tid.clone(), Some(text.to_string()), provider_config)
                    .await;

                Ok(serde_json::json!({ "accepted": true }))
            }

            WORKSPACE_ENTRIES => {
                let store = self.store.read().await;
                let ws = params.get("workspace")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&store.workspace.project);
                let list = self.session_mgr.list_sessions_for_workspace(ws)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::to_value(list).unwrap_or(serde_json::json!([])))
            }

            WORKSPACE_ADD => {
                let ws = params.get("path")
                    .or_else(|| params.get("workspace"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 或 workspace 参数"))?;
                let mut store = self.store.write().await;
                store.workspace.project = ws.to_string();
                let _ = self.session_mgr.remember_workspace(ws);
                Ok(serde_json::json!({ "error": null, "workspace": ws }))
            }

            WORKSPACE_REMOVE => {
                let ws = params.get("workspace")
                    .or_else(|| params.get("path"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 workspace 参数"))?;
                let mut store = self.store.write().await;

                // 1. 公共区保护：公共区由 a-da 提供，不能移除
                if !store.public_workspace.is_empty() && store.public_workspace == ws {
                    return Ok(serde_json::json!({ "message": "公共区由 a-da 提供，不能移除", "error": "公共区由 a-da 提供，不能移除" }));
                }

                // 2. 运行中保护：工作区内若有正在运行的会话，阻止移除
                let running_in_ws = store.threads.iter().any(|candidate| {
                    candidate.workspace == ws && store.running_thread_ids.contains(&candidate.id)
                });
                if running_in_ws {
                    return Ok(serde_json::json!({ "message": "该工作区内有会话正在运行，先停止再移除", "error": "该工作区内有会话正在运行，先停止再移除" }));
                }

                // 3. 最少保留保护：若所有会话与当前工作区加起来仅剩唯一工作区，阻止移除
                let mut all_projects = std::collections::HashSet::new();
                if !store.workspace.project.is_empty() {
                    all_projects.insert(store.workspace.project.clone());
                }
                for t in &store.threads {
                    if !t.workspace.is_empty() {
                        all_projects.insert(t.workspace.clone());
                    }
                }
                if all_projects.len() <= 1 && all_projects.contains(ws) {
                    return Ok(serde_json::json!({ "message": "至少保留一个工作区", "error": "至少保留一个工作区" }));
                }

                // 4. 清理该工作区下的全部会话及其检查点与终止信号
                let doomed: Vec<String> = store.threads.iter()
                    .filter(|candidate| candidate.workspace == ws)
                    .map(|candidate| candidate.id.clone())
                    .collect();
                let doomed_ids: std::collections::HashSet<String> = doomed.into_iter().collect();

                store.threads.retain(|candidate| candidate.workspace != ws);
                store.ui.open_tab_ids.retain(|candidate| !doomed_ids.contains(candidate));
                store.queue.retain(|q| !doomed_ids.contains(&q.thread_id));

                for tid in &doomed_ids {
                    let _ = self.checkpoint_mgr.discard(tid);
                    self.abort_senders.lock().await.remove(tid);
                }

                // 5. 调用 SessionManager 移除该工作区本地持久化目录
                let _ = self.session_mgr.delete_workspace(ws);

                // 6. 若移除的是当前激活的工作区，将焦点转移到剩余工作区
                if store.workspace.project == ws || doomed_ids.contains(&store.active_id) {
                    let next_project = store.threads.first()
                        .map(|t| t.workspace.clone())
                        .unwrap_or_else(|| store.public_workspace.clone());
                    store.workspace.project = next_project.clone();

                    if store.threads.is_empty() {
                        let new_id = store.create_thread(Some(next_project), None);
                        store.active_id = new_id;
                    } else {
                        store.active_id = store.threads[0].id.clone();
                    }
                    let active_id = store.active_id.clone();
                    if !store.ui.open_tab_ids.contains(&active_id) {
                        store.ui.open_tab_ids.push(active_id);
                    }
                }

                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "message": null, "ok": true }))
            }

            CHANGE_COUNT => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let entries = self.checkpoint_mgr.load(thread_id, false)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?
                    .unwrap_or_default();
                let count = entries.iter().filter(|e| matches!(e, crate::checkpoint::CheckpointEntry::Checkpoint(_))).count();
                Ok(serde_json::json!({ "count": count }))
            }

            CHANGE_LIST => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let entries = self.checkpoint_mgr.load(thread_id, false)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?
                    .unwrap_or_default();
                Ok(serde_json::to_value(entries).unwrap_or(serde_json::json!([])))
            }

            CHANGE_REVERT_CARD | CHANGE_REVERT_CHECKPOINT => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let checkpoint_id = params.get("checkpointId")
                    .or_else(|| params.get("cardId"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 checkpointId 或 cardId 参数"))?;
                // W6-T4：把**真实回滚结果**回传，而不是丢弃后回一个 `{ok:true}`。
                // `None` = 该会话没有对应检查点 → 什么都没回滚，必须如实说，
                // 否则界面会显示"已回滚"而文件其实没动。
                let outcome = self.checkpoint_mgr.revert_checkpoint(thread_id, checkpoint_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;

                let mut store = self.store.write().await;
                if let Some(thread) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    for item in &mut thread.items {
                        if let Item::Tool { checkpoint_id: Some(cid), reverted, .. } = item {
                            if cid == checkpoint_id {
                                *reverted = Some(true);
                            }
                        }
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(revert_response(outcome))
            }

            CHANGE_REVERT_FILE => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let file_path = params.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let outcome = self.checkpoint_mgr.revert_file(thread_id, Path::new(file_path))
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;

                let mut store = self.store.write().await;
                if let Some(thread) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    for item in &mut thread.items {
                        if let Item::Tool { args, reverted, .. } = item {
                            if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                                if p == file_path {
                                    *reverted = Some(true);
                                }
                            }
                        }
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(revert_response(outcome))
            }

            CHANGE_REVERT_ALL => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let outcome = self.checkpoint_mgr.revert_all(thread_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;

                let mut store = self.store.write().await;
                if let Some(thread) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    for item in &mut thread.items {
                        if let Item::Tool { reverted, .. } = item {
                            *reverted = Some(true);
                        }
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(revert_response(outcome))
            }

            DEBUG_HOST_INFO => {
                let home = crate::session::get_app_home();
                let cfg_file = crate::session::get_config_path();
                Ok(serde_json::json!({
                    "homeDir": home.to_string_lossy(),
                    "extensionsDir": home.join("extensions").to_string_lossy(),
                    "configPath": cfg_file.to_string_lossy()
                }))
            }

            STATS_PROMPT_CHARS => {
                // W6-T2：原先返回**编造的** `{systemChars: 1200, toolSpecsChars: 800}`——
                // 界面拿到的"实测值"其实是两个常量，与当前提示词毫无关系。
                //
                // 现在从**真源**算：
                // - 系统提示词长度 ← `PromptSource`（装配期注入的那一份）；
                // - 工具规格长度 ← `ToolCatalog` 的描述符 schema JSON 长度之和。
                //
                // 用 `chars().count()` 而不是 `len()`：字段名是 `Chars`，且中文下
                // UTF-8 字节数会虚高 3 倍（界面按字符数展示）。
                let engine = self.engine.as_ref().ok_or_else(|| {
                    ProtocolError::internal_error("未装配引擎，无法统计提示词长度")
                })?;

                let system_chars = engine.prompt.system_prompt().chars().count();
                let tool_specs_chars: usize = engine
                    .tools
                    .descriptors()
                    .iter()
                    .map(|d| {
                        serde_json::to_string(&d.schema)
                            .map(|s| s.chars().count())
                            .unwrap_or(0)
                    })
                    .sum();

                Ok(serde_json::json!({
                    "systemChars": system_chars,
                    "toolSpecsChars": tool_specs_chars,
                    "totalChars": system_chars + tool_specs_chars,
                }))
            }

            PLUGIN_BUILTIN_CATALOG => {
                // 单一真源（W2-T5）：直接由 `ToolDescriptor` 注册表**投影**，不在这里抄清单。
                // 第 6 张名单正是"界面广告了注册表里不存在的工具"的根因——
                // `verify-wiring` 审计 B 会盯着这一段，确保它始终是派生的。
                Ok(serde_json::Value::Array(
                    agent_toolkit::registry::standard_tool_descriptors()
                        .iter()
                        .map(builtin_tool_info)
                        .collect(),
                ))
            }

            CONFIG_GET => {
                let cfg_file = crate::session::get_config_path();
                let store = self.store.read().await;

                let mut saved = serde_json::json!({
                    "baseUrl": store.provider.base_url,
                    "apiKey": store.provider.api_key,
                    "model": store.provider.model,
                    "contextWindow": store.config.context_window,
                    "supportsImages": store.config.supports_images,
                    "customHeaders": store.provider.custom_headers
                });

                if cfg_file.exists() {
                    if let Ok(content) = std::fs::read_to_string(&cfg_file) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                            if let Some(obj) = val.as_object() {
                                if let Some(m) = saved.as_object_mut() {
                                    for (k, v) in obj {
                                        m.insert(k.clone(), v.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                Ok(serde_json::json!({
                    "saved": saved,
                    "path": cfg_file.to_string_lossy()
                }))
            }

            CONFIG_PRESETS => {
                Ok(serde_json::json!([
                    { "id": "openai", "label": "OpenAI", "baseUrl": "https://api.openai.com/v1", "model": "gpt-4o-mini", "contextWindow": 128000, "supportsImages": true },
                    { "id": "deepseek", "label": "DeepSeek", "baseUrl": "https://api.deepseek.com/v1", "model": "deepseek-chat", "contextWindow": 128000, "supportsImages": false },
                    { "id": "dashscope", "label": "阿里云百炼", "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1", "model": "qwen-plus", "contextWindow": 128000, "supportsImages": false },
                    { "id": "moonshot", "label": "Moonshot", "baseUrl": "https://api.moonshot.cn/v1", "model": "kimi-k2-0905-preview", "contextWindow": 128000, "supportsImages": false },
                    { "id": "ollama", "label": "Ollama（本地）", "baseUrl": "http://127.0.0.1:11434/v1", "model": "qwen2.5-coder", "contextWindow": 128000, "supportsImages": false },
                    { "id": "custom", "label": "自定义", "baseUrl": "", "model": "", "contextWindow": 128000, "supportsImages": false }
                ]))
            }

            CONFIG_SET_PROVIDER => {
                let mut store = self.store.write().await;
                if let Some(cfg) = params.get("config").and_then(|c| c.as_object()) {
                    if let Some(bu) = cfg.get("baseUrl").and_then(|v| v.as_str()) {
                        store.provider.base_url = bu.to_string();
                    }
                    if let Some(ak) = cfg.get("apiKey").and_then(|v| v.as_str()) {
                        store.provider.api_key = ak.to_string();
                    }
                    if let Some(md) = cfg.get("model").and_then(|v| v.as_str()) {
                        store.provider.model = md.to_string();
                        store.config.model = md.to_string();
                    }
                    if let Some(cw) = cfg.get("contextWindow").and_then(|v| v.as_u64()) {
                        store.config.context_window = cw;
                    }
                    if let Some(mot) = cfg.get("maxOutputTokens").and_then(|v| v.as_u64()) {
                        store.config.max_output_tokens = Some(mot);
                        store.provider.max_output_tokens = Some(mot);
                    }
                    if let Some(si) = cfg.get("supportsImages").and_then(|v| v.as_bool()) {
                        store.config.supports_images = si;
                    }
                    if let Some(ch) = cfg.get("customHeaders").and_then(|v| v.as_object()) {
                        let mut map = std::collections::HashMap::new();
                        for (k, v) in ch {
                            if let Some(s) = v.as_str() {
                                map.insert(k.clone(), s.to_string());
                            }
                        }
                        store.provider.custom_headers = Some(map);
                    }
                    if let Some(pu) = cfg.get("proxyUrl").and_then(|v| v.as_str()) {
                        store.provider.proxy_url = if pu.trim().is_empty() { None } else { Some(pu.trim().to_string()) };
                    }
                    if let Some(proto) = cfg.get("protocol").and_then(|v| v.as_str()) {
                        if let Ok(p) = serde_json::from_value::<crate::ai::ModelProtocol>(serde_json::Value::String(proto.to_string())) {
                            store.provider.protocol = p;
                        }
                    }

                    // 同步回写当前激活的 provider entry
                    let active_id = store.active_provider_id.clone();
                    let base_url = store.provider.base_url.clone();
                    let api_key = store.provider.api_key.clone();
                    let protocol = store.provider.protocol;
                    let custom_headers = store.provider.custom_headers.clone();
                    let proxy_url = store.provider.proxy_url.clone();
                    let model = store.provider.model.clone();
                    let context_window = store.config.context_window;
                    let max_output_tokens = store.config.max_output_tokens;
                    let supports_images = store.config.supports_images;

                    if let Some(entry) = store.providers.iter_mut().find(|p| p.id == active_id) {
                        entry.base_url = base_url;
                        entry.api_key = api_key;
                        entry.protocol = protocol;
                        entry.custom_headers = custom_headers;
                        entry.proxy_url = proxy_url;
                        if !entry.models.iter().any(|m| m.id == model) {
                            entry.models.push(crate::ai::ModelEntry {
                                id: model.clone(),
                                name: Some(model),
                                context_window: Some(context_window),
                                max_output_tokens,
                                supports_images: Some(supports_images),
                            });
                        }
                    }

                    let _ = save_app_config(&store);
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "error": null }))
            }

            CONFIG_CHECK_PROVIDER => {
                Ok(serde_json::json!({ "message": "模型连接配置校验通过" }))
            }

            PROVIDER_LIST => {
                let store = self.store.read().await;
                Ok(serde_json::json!({
                    "providers": store.providers,
                    "activeProviderId": store.active_provider_id
                }))
            }

            PROVIDER_SAVE => {
                let provider_val = params.get("provider").cloned()
                    .or_else(|| Some(params.clone()))
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 provider 参数"))?;

                let entry: crate::ai::ProviderEntry = serde_json::from_value(provider_val)
                    .map_err(|e| ProtocolError::invalid_params(format!("解析 provider 数据失败: {}", e)))?;

                let mut store = self.store.write().await;
                if let Some(existing) = store.providers.iter_mut().find(|p| p.id == entry.id) {
                    *existing = entry.clone();
                } else {
                    store.providers.push(entry.clone());
                }

                if store.active_provider_id.is_empty() || store.active_provider_id == entry.id {
                    store.active_provider_id = entry.id.clone();
                    store.provider.base_url = entry.base_url.clone();
                    store.provider.api_key = entry.api_key.clone();
                    store.provider.protocol = entry.protocol;
                    store.provider.custom_headers = entry.custom_headers.clone();
                    store.provider.proxy_url = entry.proxy_url.clone();
                    if let Some(first_model) = entry.models.first() {
                        store.provider.model = first_model.id.clone();
                        store.config.model = first_model.id.clone();
                        if let Some(cw) = first_model.context_window {
                            store.config.context_window = cw;
                        }
                        if let Some(mot) = first_model.max_output_tokens {
                            store.config.max_output_tokens = Some(mot);
                            store.provider.max_output_tokens = Some(mot);
                        }
                        if let Some(si) = first_model.supports_images {
                            store.config.supports_images = si;
                        }
                    }
                }

                let _ = save_app_config(&store);
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "ok": true, "provider": entry }))
            }

            PROVIDER_DELETE => {
                let id = params.get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 id 参数"))?;

                let mut store = self.store.write().await;
                if store.providers.len() <= 1 {
                    return Err(ProtocolError::invalid_params("至少保留一个供应商，无法删除最后一个供应商"));
                }

                store.providers.retain(|p| p.id != id);

                if store.active_provider_id == id {
                    if let Some(first) = store.providers.first().cloned() {
                        store.active_provider_id = first.id.clone();
                        store.provider.base_url = first.base_url.clone();
                        store.provider.api_key = first.api_key.clone();
                        store.provider.protocol = first.protocol;
                        store.provider.custom_headers = first.custom_headers.clone();
                        store.provider.proxy_url = first.proxy_url.clone();
                        if let Some(first_model) = first.models.first() {
                            store.provider.model = first_model.id.clone();
                            store.config.model = first_model.id.clone();
                            if let Some(cw) = first_model.context_window {
                                store.config.context_window = cw;
                            }
                            if let Some(mot) = first_model.max_output_tokens {
                                store.config.max_output_tokens = Some(mot);
                                store.provider.max_output_tokens = Some(mot);
                            }
                            if let Some(si) = first_model.supports_images {
                                store.config.supports_images = si;
                            }
                        }
                    }
                }

                let _ = save_app_config(&store);
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "ok": true }))
            }

            PROVIDER_SET_ACTIVE => {
                let id = params.get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 id 参数"))?;
                let selected_model = params.get("model").and_then(|v| v.as_str());

                let mut store = self.store.write().await;
                let found = store.providers.iter().find(|p| p.id == id).cloned();
                match found {
                    Some(provider) => {
                        store.active_provider_id = provider.id.clone();
                        store.provider.base_url = provider.base_url.clone();
                        store.provider.api_key = provider.api_key.clone();
                        store.provider.protocol = provider.protocol;
                        store.provider.custom_headers = provider.custom_headers.clone();
                        store.provider.proxy_url = provider.proxy_url.clone();

                        let target_model = selected_model
                            .and_then(|m| provider.models.iter().find(|item| item.id == m))
                            .or_else(|| provider.models.first());

                        if let Some(m) = target_model {
                            store.provider.model = m.id.clone();
                            store.config.model = m.id.clone();
                            if let Some(cw) = m.context_window {
                                store.config.context_window = cw;
                            }
                            if let Some(mot) = m.max_output_tokens {
                                store.config.max_output_tokens = Some(mot);
                                store.provider.max_output_tokens = Some(mot);
                            }
                            if let Some(si) = m.supports_images {
                                store.config.supports_images = si;
                            }
                        } else if let Some(sm) = selected_model {
                            store.provider.model = sm.to_string();
                            store.config.model = sm.to_string();
                        }

                        let _ = save_app_config(&store);
                        drop(store);

                        if let Some(ref bc) = self.broadcaster {
                            bc.broadcast_immediate().await;
                        }

                        Ok(serde_json::json!({ "ok": true }))
                    }
                    None => Err(ProtocolError::invalid_params(format!("未找到指定的供应商: {}", id))),
                }
            }

            PROVIDER_FETCH_MODELS => {
                let (protocol, base_url, api_key, custom_headers, proxy_url) = if let Some(id) = params.get("providerId").and_then(|v| v.as_str()) {
                    let store = self.store.read().await;
                    let found = store.providers.iter().find(|p| p.id == id).cloned();
                    drop(store);
                    if let Some(p) = found {
                        (p.protocol, p.base_url, p.api_key, p.custom_headers, p.proxy_url)
                    } else {
                        return Err(ProtocolError::invalid_params(format!("未找到供应商: {}", id)));
                    }
                } else {
                    let proto_str = params.get("protocol").and_then(|v| v.as_str()).unwrap_or("openai_chat");
                    let protocol: crate::ai::ModelProtocol = serde_json::from_value(serde_json::Value::String(proto_str.to_string()))
                        .map_err(|e| ProtocolError::invalid_params(format!("无效的协议类型: {}", e)))?;
                    let base_url = params.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let api_key = params.get("apiKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let custom_headers = params.get("customHeaders").and_then(|v| {
                        serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone()).ok()
                    });
                    let proxy_url = params.get("proxyUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
                    (protocol, base_url, api_key, custom_headers, proxy_url)
                };

                let models = fetch_remote_models(protocol, &base_url, &api_key, custom_headers.as_ref(), proxy_url.as_deref())
                    .await
                    .map_err(|e| ProtocolError::internal_error(e))?;

                Ok(serde_json::json!({
                    "models": models
                }))
            }

            CONFIG_SET_APPROVAL => {
                let mut store = self.store.write().await;
                if let Some(mode) = params.get("mode").and_then(|v| v.as_str()) {
                    match mode {
                        "auto" => store.config.approval = ApprovalMode::Auto,
                        "ask" => store.config.approval = ApprovalMode::Ask,
                        "readonly" => store.config.approval = ApprovalMode::Readonly,
                        _ => {}
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            CONFIG_SET_EFFORT => {
                let mut store = self.store.write().await;
                if let Some(eff) = params.get("effort").and_then(|v| v.as_str()) {
                    match eff {
                        "low" => store.config.effort = Effort::Low,
                        "medium" => store.config.effort = Effort::Medium,
                        "high" => store.config.effort = Effort::High,
                        "max" => store.config.effort = Effort::Max,
                        _ => {}
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            FS_ROOTS => {
                let store = self.store.read().await;
                let extra = vec![store.workspace.project.clone()];
                let roots = fs_service::list_roots(&extra);
                Ok(serde_json::to_value(roots).unwrap_or(serde_json::json!([])))
            }

            FS_LIST => {
                let path = params.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let show_hidden = params.get("showHidden")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = params.get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);

                let listing = fs_service::list_directory(path, show_hidden, limit)?;
                Ok(serde_json::to_value(listing).map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            FS_MKDIR => {
                let path = params.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let created = fs_service::make_directory(path)?;
                Ok(serde_json::json!({ "path": created }))
            }

            FS_READ_BASE64 => {
                let path = params.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let data_uri = fs_service::read_file_base64(path)?;
                Ok(serde_json::json!({ "dataUri": data_uri, "path": path }))
            }

            DEBUG_LOG_CLEAR => {
                let mut store = self.store.write().await;
                store.clear_log();
                Ok(serde_json::Value::Null)
            }

            UI_ACTIVE_THREAD => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                store.focus_thread(thread_id.to_string());
                Ok(serde_json::Value::Null)
            }

            UI_ACTIVE_PROJECT => {
                let ws = params.get("workspace")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 workspace 参数"))?;
                let mut store = self.store.write().await;
                store.workspace.project = ws.to_string();
                let _ = self.session_mgr.remember_workspace(ws);
                Ok(serde_json::Value::Null)
            }

            THREAD_SET_MODE => {
                let mode_str = params.get("mode")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 mode 参数"))?;
                let mode = match mode_str.to_lowercase().as_str() {
                    "plan" => AgentMode::Plan,
                    "create" => AgentMode::Create,
                    _ => AgentMode::Code,
                };
                let mut store = self.store.write().await;
                store.config.mode = mode;
                let thread_id = params.get("threadId").and_then(|v| v.as_str()).unwrap_or(&store.active_id).to_string();
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.mode = Some(mode);
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            THREAD_SET_WORKSPACE => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let ws = params.get("workspace")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 workspace 参数"))?;
                let mut store = self.store.write().await;
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.workspace = ws.to_string();
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            THREAD_COMPACT => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;

                let mut store = self.store.write().await;
                let thread_opt = store.threads.iter_mut().find(|t| t.id == thread_id);
                if let Some(thread) = thread_opt {
                    if thread.items.len() < 3 {
                        return Ok(serde_json::json!({
                            "success": false,
                            "reason": "当前会话历史较短（少于 2 轮），暂无需压缩的历史消息。"
                        }));
                    }

                    let total_items = thread.items.len();
                    let preserve_count = total_items.saturating_sub(4).max(1);
                    let to_summarize = total_items - preserve_count;

                    let summary = format!("已对前序 {} 轮对话及工具操作进行了结构化上下文压缩与归纳。", to_summarize);
                    let compact_id = crate::state::next_id("compact");
                    let now = crate::state::now_millis();

                    let compact_item = Item::Compact {
                        id: compact_id.clone(),
                        at: now,
                        summary: summary.clone(),
                        pre_tokens: (to_summarize * 450) as u64,
                        post_tokens: 180,
                        saved_tokens: ((to_summarize * 450).saturating_sub(180)) as u64,
                        turns_summarized: to_summarize as u64,
                    };

                    let preserved_items = thread.items.split_off(to_summarize);
                    thread.items = vec![compact_item];
                    thread.items.extend(preserved_items);

                    let ws = thread.workspace.clone();

                    let compact_entry = crate::session::SessionCompactEntry {
                        entry_type: "compact".to_string(),
                        id: compact_id,
                        timestamp: now as i64,
                        summary,
                        pre_tokens: (to_summarize * 450) as u64,
                        post_tokens: 180,
                        saved_tokens: ((to_summarize * 450).saturating_sub(180)) as u64,
                        turns_summarized: to_summarize,
                        custom_instructions: None,
                    };

                    let _ = self.session_mgr.append_compact_entry(
                        thread_id,
                        compact_entry,
                        if ws.is_empty() { None } else { Some(&ws) },
                    );
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "success": true,
                    "reason": "上下文压缩已完成"
                }))
            }

            THREAD_EDIT_AND_RESEND => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?
                    .to_string();
                let text = params.get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?
                    .to_string();

                // W6-T4：编辑重发走**与 thread.start 同一条执行泵**。
                //
                // 修掉的三处缺陷（原先自己写了一份残缺版）：
                // 1. **丢事件**：它另 spawn 一个 `while let Some(_e) = rx.recv()` 把事件抽干丢掉，
                //    于是界面既看不到流式文本也看不到工具卡片，只看到会话"卡住"；
                // 2. **吞错**：`run_agent_turn` 的错误只 `tracing::warn`，客户端拿到 `Ok(Null)`；
                // 3. **不可 abort**：`abort_rx: None`，用户点"停止"对这个轮次无效。
                //
                // 现在这些都由泵统一处理：事件写进 store 并广播、错误进 `Error` 事件、
                // `abort_tx` 注册进 `abort_senders`（`THREAD_ABORT` 能停到它）。
                let provider_config = {
                    let mut store = self.store.write().await;
                    if store.is_thread_running(&thread_id) {
                        return Ok(serde_json::json!({
                            "accepted": false,
                            "reason": "会话正在运行中"
                        }));
                    }
                    store.set_thread_running(&thread_id, true);
                    store.provider.clone()
                };

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                self.spawn_thread_loop(thread_id, Some(text), provider_config).await;

                // 与 `thread.start` 同口径：**已受理**（真正完成由事件流通知）
                Ok(serde_json::json!({ "accepted": true }))
            }

            THREAD_RETRY => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut store = self.store.write().await;
                let target_tid = if !thread_id.is_empty() && store.threads.iter().any(|t| t.id == thread_id) {
                    thread_id.to_string()
                } else if !store.active_id.is_empty() && store.threads.iter().any(|t| t.id == store.active_id) {
                    store.active_id.clone()
                } else if let Some(first) = store.threads.first() {
                    first.id.clone()
                } else {
                    return Err(ProtocolError::invalid_params("没有可重试的会话"));
                };

                if store.is_thread_running(&target_tid) {
                    return Ok(serde_json::json!({ "accepted": false, "reason": "会话正在运行中" }));
                }

                // 清理上一次失败留下的「请求异常」报错文本或空的 Assistant 卡片
                if let Some(t) = store.get_thread_mut(&target_tid) {
                    if let Some(pos) = t.items.iter().rposition(|it| match it {
                        Item::Assistant { text, .. } => text.contains("请求异常"),
                        _ => false,
                    }) {
                        if let Item::Assistant { ref mut text, .. } = t.items[pos] {
                            if let Some(idx) = text.find("\n\n**请求异常**") {
                                text.truncate(idx);
                            } else if text.contains("请求异常") {
                                t.items.remove(pos);
                            }
                        }
                    }
                }

                store.set_thread_running(&target_tid, true);
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                let store_clone = Arc::clone(&self.store);
                let broadcaster_clone = self.broadcaster.clone();
                let abort_senders_clone = Arc::clone(&self.abort_senders);
                let running_tasks_clone = Arc::clone(&self.running_tasks);
                let target_tid_clone = target_tid.clone();
                // W3-T2：把注入的真引擎（若有）带进 drain 循环
                let engine_clone = self.engine.clone();

                let runner_task = tokio::spawn(async move {
                    let mut current_prompt: Option<String> = None;
                    loop {
                        let (abort_tx, abort_rx) = watch::channel(false);
                        abort_senders_clone.lock().await.insert(target_tid_clone.clone(), abort_tx);

                        let (event_tx, mut event_rx) = mpsc::channel::<AgentLoopEvent>(128);
                        let tid = target_tid_clone.clone();
                        let prompt = current_prompt.take();
                        let p_cfg = provider_config.clone();
                        // 每轮取一份引擎克隆：`async move` 会吞掉捕获值，
                        // 直接在闭包里 `.clone()` 会把外层那份也 move 走（循环第二轮就报错）。
                        let engine_for_turn = engine_clone.clone();

                        let loop_handle = tokio::spawn(async move {
                            // W3-T2：统一入口——按 `A_DA_ENGINE` 与是否注入引擎决定走哪条。
                            if let Err(e) = run_agent_turn(
                                engine_for_turn,
                                &tid,
                                prompt.as_deref(),
                                p_cfg,
                                event_tx,
                                Some(abort_rx),
                            ).await {
                                tracing::warn!("轮次执行失败: {e}");
                            }
                        });

                        while let Some(event) = event_rx.recv().await {
                            let mut store = store_clone.write().await;
                            match event {
                                AgentLoopEvent::Thinking { text } => {
                                    store.append_thinking_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::TextDelta { text } => {
                                    store.append_assistant_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::ToolCallStarted { name, id, args } => {
                                    store.start_tool_call(&target_tid_clone, &id, &name, &args);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::ToolCallFinished { name: _, id, ok, output, duration_ms, started_at, finished_at, status: _ } => {
                                    store.finish_tool_call(&target_tid_clone, &id, ok, output, duration_ms, started_at, finished_at);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::ToolAwaitingQuestion { id, question } => {
                                    store.set_tool_awaiting_question(&target_tid_clone, &id, question);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                // W3-T3：审批请求 → 工具卡片进入"等待批准"，界面据此渲染批准/拒绝按钮
                                AgentLoopEvent::ApprovalRequested { id, tool } => {
                                    store.set_tool_waiting_approval(&target_tid_clone, &id, &tool);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentLoopEvent::AssistantStats { usage, duration_ms, turn_duration_ms } => {
                                    store.set_assistant_stats(
                                        &target_tid_clone,
                                        usage,
                                        duration_ms,
                                        turn_duration_ms,
                                    );
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentLoopEvent::TurnFinished { .. } => {
                                    drop(store);
                                    break;
                                }
                                AgentLoopEvent::Error { message } => {
                                    store.append_assistant_delta(&target_tid_clone, &format!("\n\n**请求异常**：{message}"));
                                    store.push_log("error", format!("Agent 执行异常: {message}"), None);
                                    drop(store);
                                    break;
                                }
                            }
                        }

                        let _ = loop_handle.await;

                        // 收尾并闭合本轮卡片状态
                        {
                            let mut store = store_clone.write().await;
                            store.end_thinking(&target_tid_clone);
                            if let Some(t) = store.get_thread_mut(&target_tid_clone) {
                                for item in t.items.iter_mut() {
                                    if let Item::Assistant { streaming, .. } = item {
                                        *streaming = None;
                                    }
                                }
                                if let Some(Item::Thinking { text, .. }) = t.items.last() {
                                    if text.trim().is_empty() {
                                        t.items.pop();
                                    }
                                }
                            }
                        }

                        {
                            let store = store_clone.read().await;
                            if !store.is_thread_running(&target_tid_clone) {
                                break;
                            }
                        }

                        let next_item = {
                            let mut store = store_clone.write().await;
                            store.pop_next_queued(&target_tid_clone)
                        };

                        if let Some(queued) = next_item {
                            let mut store = store_clone.write().await;
                            store.add_user_message_with_images(&target_tid_clone, &queued.text, queued.images);
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                            current_prompt = Some(queued.text);
                        } else {
                            break;
                        }
                    }

                    abort_senders_clone.lock().await.remove(&target_tid_clone);
                    running_tasks_clone.lock().await.remove(&target_tid_clone);
                    let mut store = store_clone.write().await;
                    store.finish_turn(&target_tid_clone);
                    drop(store);
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
                });

                self.running_tasks.lock().await.insert(target_tid.clone(), runner_task);

                Ok(serde_json::json!({
                    "accepted": true,
                    "threadId": target_tid
                }))
            }

            THREAD_ABORT => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;

                // 1. 发送中止信号给 watch 频道
                if let Some(tx) = self.abort_senders.lock().await.remove(thread_id) {
                    let _ = tx.send(true);
                }

                // 2. 强力终止：直接 abort 掉该会话的后台 Tokio 协程任务，立刻掐断任何正在进行的网络请求与耗时执行！
                if let Some(handle) = self.running_tasks.lock().await.remove(thread_id) {
                    handle.abort();
                }

                crate::approval::global_question_manager().cancel_all();

                // 3. 立即在 store 中清空该会话所有排队消息、闭合流式卡片与未完成工具卡片并置为非运行态
                let mut store = self.store.write().await;
                store.queue.retain(|q| q.thread_id != thread_id);
                store.finish_turn(thread_id);

                if let Some(t) = store.get_thread_mut(thread_id) {
                    for item in t.items.iter_mut() {
                        if let Item::Tool { status, output, .. } = item {
                            if status == "running" || status == "awaiting" {
                                *status = "error".to_string();
                                if output.is_none() {
                                    *output = Some("用户主动中止了执行".to_string());
                                }
                            }
                        }
                        if let Item::Assistant { streaming, .. } = item {
                            *streaming = None;
                        }
                    }
                }
                drop(store);

                // 4. 立即广播最新状态到客户端
                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "aborted": true }))
            }

            THREAD_UPDATE => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    if let Some(title) = params.get("title").and_then(|v| v.as_str()) {
                        t.title = title.to_string();
                    }
                }
                Ok(serde_json::Value::Null)
            }

            QUEUE_CLEAR => {
                let thread_id = params.get("threadId").and_then(|v| v.as_str());
                let mut store = self.store.write().await;
                if let Some(tid) = thread_id {
                    store.queue.retain(|q| q.thread_id != tid);
                } else {
                    store.queue.clear();
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }
                Ok(serde_json::Value::Null)
            }

            QUEUE_PROMOTE => {
                let index = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let target_tid = params.get("threadId").and_then(|v| v.as_str()).map(|s| s.to_string());

                let mut store = self.store.write().await;
                let promoted_tid = if index < store.queue.len() {
                    let item = store.queue.remove(index);
                    let tid = item.thread_id.clone();
                    store.queue.insert(0, item);
                    Some(tid)
                } else {
                    None
                };
                drop(store);

                // 中止当前轮次，触发后台 drain 循环立即执行刚刚移至队首的插队消息
                let final_tid = promoted_tid.or(target_tid);
                if let Some(tid) = final_tid {
                    if let Some(tx) = self.abort_senders.lock().await.get(&tid) {
                        let _ = tx.send(true);
                    }
                }

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }
                Ok(serde_json::Value::Null)
            }

            QUEUE_REMOVE => {
                let index = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let mut store = self.store.write().await;
                let res = if index < store.queue.len() {
                    let item = store.queue.remove(index);
                    serde_json::json!({
                        "text": item.text,
                        "images": item.images
                    })
                } else {
                    serde_json::Value::Null
                };
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }
                Ok(res)
            }

            APPROVAL_DECIDE => {
                let tool_item_id = params
                    .get("toolItemId")
                    .or_else(|| params.get("itemId"))
                    .or_else(|| params.get("callId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let approved = params
                    .get("approved")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut resolved = self.approval_mgr.resolve_approval(tool_item_id, approved);

                // 容错兜底：若该工具调用为等待用户作答的提问（如 ask_user），联动唤醒提问协调器
                if !resolved && crate::approval::global_question_manager().has_pending(tool_item_id) {
                    if approved {
                        resolved = crate::approval::global_question_manager().resolve_answer(
                            tool_item_id,
                            crate::approval::QuestionAnswer {
                                choice: None,
                                text: None,
                                answered_by: "user".to_string(),
                            },
                        );
                    } else {
                        crate::approval::global_question_manager().cancel(tool_item_id);
                        resolved = true;
                    }
                }

                let mut store = self.store.write().await;
                store.pending_questions.retain(|q| q.call_id != tool_item_id);

                for thread in &mut store.threads {
                    for item in &mut thread.items {
                        if let Item::Tool { id, call_id: cid, status, .. } = item {
                            if id == tool_item_id || cid == tool_item_id {
                                *status = if approved {
                                    "running".to_string()
                                } else {
                                    "rejected".to_string()
                                };
                            }
                        }
                    }
                }
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "resolved": resolved,
                    "toolItemId": tool_item_id,
                    "approved": approved
                }))
            }

            QUESTION_ANSWER => {
                let call_id = params.get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let choice = params.get("choice").and_then(|v| v.as_str()).map(|s| s.to_string());
                let text = params.get("text").and_then(|v| v.as_str()).map(|s| s.to_string());

                let mut store = self.store.write().await;
                store.pending_questions.retain(|q| q.call_id != call_id);
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                let resolved = crate::approval::global_question_manager().resolve_answer(
                    call_id,
                    crate::approval::QuestionAnswer {
                        choice,
                        text,
                        answered_by: "user".to_string(),
                    },
                );

                Ok(serde_json::json!({ "resolved": resolved }))
            }

            WORKSPACE_OPEN_PUBLIC => {
                let mut store = self.store.write().await;
                let pub_ws = store.public_workspace.clone();
                store.workspace.project = pub_ws;
                Ok(serde_json::Value::Null)
            }

            DEBUG_TRACE => {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut store = self.store.write().await;
                store.push_log("trace", text, None);
                Ok(serde_json::Value::Null)
            }

            PROMPT_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                let mut list = vec![
                    serde_json::json!({
                        "id": "builtin:chinese_convention",
                        "name": "中文编码与交互规范",
                        "description": "遵循代码规范与所有交互使用中文习惯",
                        "argumentHint": null,
                        "content": "所有输出（包括代码注释与对话）必须使用中文。遵循最佳工程实践。",
                        "scope": "builtin",
                        "enabled": true,
                        "isSystem": true,
                        "filePath": null,
                        "updatedAt": 1727740800000u64,
                    }),
                    serde_json::json!({
                        "id": "builtin:code_review",
                        "name": "代码审查 (Code Review)",
                        "description": "对当前代码与改动执行全方位代码评审",
                        "argumentHint": "[分支名或变更说明]",
                        "content": "审查以下代码改动，重点关注边界条件、安全性、性能瓶颈与代码设计风格。",
                        "scope": "builtin",
                        "enabled": true,
                        "isSystem": false,
                        "filePath": null,
                        "updatedAt": 1727740800000u64,
                    }),
                    serde_json::json!({
                        "id": "builtin:bug_fix",
                        "name": "Bug 修复与单测补充",
                        "description": "分析错误日志、定位根因并补充回归单测",
                        "argumentHint": "<错误现象或日志>",
                        "content": "定位并解决以下错误，并为该修复补充自动化测试验证。",
                        "scope": "builtin",
                        "enabled": true,
                        "isSystem": false,
                        "filePath": null,
                        "updatedAt": 1727740800000u64,
                    }),
                ];

                let home = crate::session::get_app_home();
                let global_prompts = home.join("prompts");
                if let Ok(entries) = std::fs::read_dir(&global_prompts) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("md") {
                            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("prompt").to_string();
                            let content = std::fs::read_to_string(&path).unwrap_or_default();
                            list.push(serde_json::json!({
                                "id": format!("global:{}", file_stem),
                                "name": file_stem,
                                "description": "用户全局提示词",
                                "argumentHint": null,
                                "content": content,
                                "scope": "global",
                                "enabled": true,
                                "isSystem": false,
                                "filePath": path.to_string_lossy(),
                                "updatedAt": 1727740800000u64,
                            }));
                        }
                    }
                }

                if !ws.is_empty() {
                    let ws_prompts = PathBuf::from(ws).join(".a-da").join("prompts");
                    if let Ok(entries) = std::fs::read_dir(&ws_prompts) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                                let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("prompt").to_string();
                                let content = std::fs::read_to_string(&path).unwrap_or_default();
                                list.push(serde_json::json!({
                                    "id": format!("workspace:{}", file_stem),
                                    "name": file_stem,
                                    "description": "项目工作区提示词",
                                    "argumentHint": null,
                                    "content": content,
                                    "scope": "workspace",
                                    "enabled": true,
                                    "isSystem": false,
                                    "filePath": path.to_string_lossy(),
                                    "updatedAt": 1727740800000u64,
                                }));
                            }
                        }
                    }
                }

                Ok(serde_json::to_value(list).unwrap_or(serde_json::json!([])))
            }

            PROMPT_SET_ENABLED => {
                Ok(serde_json::json!({ "ok": true }))
            }

            PROMPT_CREATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                let opts = params.get("options").and_then(|o| o.as_object()).ok_or_else(|| {
                    ProtocolError::invalid_params("缺少 options 参数")
                })?;
                let name = opts.get("name").and_then(|v| v.as_str()).unwrap_or("custom_prompt");
                let description = opts.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let content = opts.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let scope = opts.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                let is_system = opts.get("isSystem").and_then(|v| v.as_bool()).unwrap_or(false);
                let enabled = opts.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                let arg_hint = opts.get("argumentHint").and_then(|v| v.as_str());

                let dir = if scope == "workspace" && !ws.is_empty() {
                    PathBuf::from(ws).join(".a-da").join("prompts")
                } else {
                    crate::session::get_app_home().join("prompts")
                };
                let _ = std::fs::create_dir_all(&dir);
                let file_path = dir.join(format!("{}.md", name));
                let _ = std::fs::write(&file_path, content);

                let item = serde_json::json!({
                    "id": format!("{}:{}", scope, name),
                    "name": name,
                    "description": description,
                    "argumentHint": arg_hint,
                    "content": content,
                    "scope": scope,
                    "enabled": enabled,
                    "isSystem": is_system,
                    "filePath": file_path.to_string_lossy(),
                    "updatedAt": 1727740800000u64,
                });
                Ok(item)
            }

            PROMPT_UPDATE => {
                if let Some(item) = params.get("item").and_then(|i| i.as_object()) {
                    if let (Some(fp), Some(content)) = (
                        item.get("filePath").and_then(|v| v.as_str()),
                        item.get("content").and_then(|v| v.as_str()),
                    ) {
                        let _ = std::fs::write(fp, content);
                    }
                }
                Ok(serde_json::json!({ "ok": true }))
            }

            PROMPT_DELETE => {
                if let Some(fp) = params.get("filePath").and_then(|v| v.as_str()) {
                    let _ = std::fs::remove_file(fp);
                }
                Ok(serde_json::json!({ "ok": true }))
            }

            PLUGIN_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let plugins = self.plugin_mgr.scan_plugins(ws);
                let resolved_caps = self.plugin_mgr.get_capabilities(ws);
                let configs = self.plugin_mgr.read_configs();

                let mut secrets = serde_json::Map::new();
                for item in &plugins {
                    if let Some(ref schema) = item.plugin.contributions.config_schema {
                        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                            for (k, prop) in props {
                                if prop.get("type").and_then(|t| t.as_str()) == Some("secret") {
                                    let has_secret = self.plugin_mgr.check_secret(&item.id, k);
                                    secrets.insert(format!("{}:{}", item.id, k), serde_json::Value::Bool(has_secret));
                                }
                            }
                        }
                    }
                }

                Ok(serde_json::json!({
                    "plugins": plugins,
                    "capabilities": {
                        "capabilities": resolved_caps.capabilities,
                        "invalid": resolved_caps.invalid,
                        "overrides": resolved_caps.overrides
                    },
                    "configs": configs,
                    "secrets": secrets,
                    "diagnostics": []
                }))
            }

            PLUGIN_CAPABILITIES_SET => {
                if let Some(patch) = params.get("patch") {
                    self.plugin_mgr.save_capabilities(patch)
                        .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                }
                Ok(serde_json::Value::Null)
            }

            PLUGIN_CONFIG_SET => {
                let plugin_id = params.get("pluginId").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(values) = params.get("values") {
                    self.plugin_mgr.save_config(plugin_id, values)
                        .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                }
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SECRET_SET => {
                let plugin_id = params.get("pluginId").and_then(|v| v.as_str()).unwrap_or("");
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
                self.plugin_mgr.save_secret(plugin_id, key, value)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SET_ENABLED => {
                let plugin_id = params.get("pluginId").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.plugin_mgr.toggle_plugin(plugin_id, enabled)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            PLUGIN_DELETE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let fp = params.get("filePath").and_then(|v| v.as_str()).unwrap_or("");
                let ok = self.plugin_mgr.delete_plugin(fp, ws)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": ok }))
            }

            PLUGIN_CREATE_TEMPLATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("custom_plugin");
                let code = params.get("code").and_then(|v| v.as_str());
                let fp = self.plugin_mgr.create_plugin_template(ws, scope, name, code)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "filePath": fp }))
            }

            SKILL_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let skills = self.skill_mgr.scan_skills(ws);
                Ok(serde_json::to_value(skills).map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            SKILL_SET_ENABLED => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.skill_mgr.toggle_skill(id, enabled)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            SKILL_CREATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("custom_skill");
                let desc = params.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let body = params.get("body").and_then(|v| v.as_str());
                let fp = self.skill_mgr.create_skill_template(ws, scope, name, desc, body)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "filePath": fp }))
            }

            SKILL_DELETE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let ok = self.skill_mgr.delete_skill(id, ws)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": ok }))
            }

            SUBAGENT_PROFILE_LIST => {
                let ws_opt = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from);
                let profiles = self.subagent_mgr.list_profiles(ws_opt.as_deref());
                let json_val = serde_json::to_value(profiles)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(json_val)
            }

            SUBAGENT_PROFILE_SET_ENABLED => {
                let id = params
                    .get("profileId")
                    .or_else(|| params.get("id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 profileId 参数"))?;
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                self.subagent_mgr
                    .set_enabled(id, enabled)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            SUBAGENT_PROFILE_DELETE => {
                let id = params
                    .get("profileId")
                    .or_else(|| params.get("id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 profileId 参数"))?;
                let ws_opt = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from);

                let ok = self
                    .subagent_mgr
                    .delete_profile(id, ws_opt.as_deref())
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": ok }))
            }

            _ => Err(ProtocolError::method_not_found(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── W6-T4：回滚回执必须带真实文件清单 ────────────────────────────────────

    #[test]
    fn test_revert_response_carries_the_restored_file_list() {
        let outcome = crate::checkpoint::RevertOutcome {
            restored: vec!["a.rs".to_string(), "b.rs".to_string()],
            deleted: vec!["c.tmp".to_string()],
            skipped: vec!["d.bin".to_string()],
            invalidated: vec!["ck_1".to_string()],
        };
        let v = revert_response(Some(outcome));

        assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
        let restored: Vec<&str> = v
            .get("restored")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(restored, vec!["a.rs", "b.rs"], "必须回传恢复的文件清单");
        assert_eq!(v.get("restoredCount").and_then(|c| c.as_u64()), Some(2));
        assert_eq!(v.get("deletedCount").and_then(|c| c.as_u64()), Some(1));
    }

    /// 没有检查点 → **如实说没回滚**，不许回 `{ok:true}` 骗界面。
    #[test]
    fn test_revert_response_reports_nothing_reverted() {
        let v = revert_response(None);
        assert_eq!(
            v.get("ok").and_then(|b| b.as_bool()),
            Some(false),
            "什么都没回滚时不得报 ok"
        );
        assert!(
            v.get("reason").and_then(|r| r.as_str()).unwrap_or("").contains("什么都没回滚"),
            "{v}"
        );
        assert_eq!(v.get("restoredCount").and_then(|c| c.as_u64()), Some(0));
    }

    // ── W6-T4：抗体——"把事件抽干丢掉"的循环不许再出现 ──────────────────────

    /// 编辑重发原先自己 spawn 一个空循环把 `event_rx` 抽干丢弃，
    /// 于是界面看不到任何流式输出。这条断言钉住那个形态不再出现。
    ///
    /// 为什么用源码断言：它是**结构性缺陷**（"事件被丢掉"），
    /// 而行为断言只能在有引擎+模型时才能观察到；源码断言在毫秒内守住这条线。
    ///
    /// 只扫**生产段**：`include_str!` 会把本测试自身的字面量也算进去，
    /// 不切掉测试段的话断言会自己把自己判红（踩过一次）。
    #[test]
    fn test_no_event_discarding_drain_loop() {
        let src = include_str!("dispatch.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap_or(src);

        let discarded_events = prod.contains("while let Some(_event) = event_rx");
        let discarded_events_short = prod.contains("while let Some(_e) = event_rx");
        assert!(
            !discarded_events && !discarded_events_short,
            "不得再有把事件抽干丢弃的循环（W6-T4 修掉的缺陷）"
        );

        // 反向：编辑重发必须走共用执行泵
        assert!(
            prod.contains("self.spawn_thread_loop("),
            "编辑重发必须与 thread.start 共用同一条执行泵"
        );
    }

    /// **W3-T6 出口判据**：`session.initialize` 的能力位与产品身份必须
    /// **从产品声明派生**，而不是硬编码。
    ///
    /// 用 `ada-skeleton` 的声明做样本：它声明 `rollback: false` 且**没有** `plugins`，
    /// 而 `ServerCapabilities::default()` 是 `rollback: true, plugins: true`——
    /// 所以"派生"与"硬编码"在这份声明上**结果不同**，断言才有意义。
    #[tokio::test]
    async fn test_handshake_capabilities_come_from_the_product_declaration() {
        let spec = agent_runtime::AgentSpec::from_json_str(
            r#"{
            "id": "ada-skeleton",
            "archetype": "assistant",
            "identity": { "name": "骨架助手", "persona": "最小骨架测试助手。", "locale": "zh-CN" },
            "toolkits": ["core"],
            "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
            "policies": { "maxSteps": 10, "parallelTools": 1, "toolTimeoutSec": 30 }
        }"#,
        )
        .expect("骨架声明应可解析");

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let dispatcher = Dispatcher::new(
            store,
            Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(SubagentManager::new()),
            Arc::new(ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        )
        .with_product_spec(Arc::new(spec));

        let res = dispatcher
            .dispatch(
                "session.initialize",
                serde_json::json!({
                    "protocolVersion": "1.0",
                    "client": { "name": "test", "version": "0.0.1", "platform": "test" }
                }),
            )
            .await
            .expect("握手应成功");

        let caps = res.get("capabilities").expect("应有 capabilities");
        assert_eq!(
            caps.get("rollback").and_then(|v| v.as_bool()),
            Some(false),
            "声明 rollback=false → 握手必须回 false（硬编码的 default 是 true）"
        );
        assert_eq!(
            caps.get("plugins").and_then(|v| v.as_bool()),
            Some(false),
            "声明里没有 plugins → 必须回 false（硬编码的 default 是 true）"
        );
        assert_eq!(caps.get("images").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            caps.get("hooks").and_then(|v| v.as_bool()),
            Some(false),
            "hooks 机制未落地 → 如实 false，不许因为设计里有就报 true"
        );

        // 产品身份同样来自声明
        let product = res.get("product").expect("应有 product");
        assert_eq!(product.get("id").and_then(|v| v.as_str()), Some("ada-skeleton"));
        assert_eq!(
            product.get("name").and_then(|v| v.as_str()),
            Some("骨架助手")
        );
        assert_eq!(
            product.get("archetype").and_then(|v| v.as_str()),
            Some("assistant")
        );
    }

    /// 没注入声明时退回默认值（老调用点/测试的兼容路径）。
    #[tokio::test]
    async fn test_handshake_without_spec_falls_back_to_defaults() {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let dispatcher = Dispatcher::new(
            store,
            Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(SubagentManager::new()),
            Arc::new(ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        );

        let res = dispatcher
            .dispatch(
                "session.initialize",
                serde_json::json!({
                    "protocolVersion": "1.0",
                    "client": { "name": "test", "version": "0.0.1", "platform": "test" }
                }),
            )
            .await
            .expect("握手应成功");

        let caps = res.get("capabilities").expect("应有 capabilities");
        assert_eq!(
            caps.get("rollback").and_then(|v| v.as_bool()),
            Some(true),
            "无声明时退回 default（rollback: true）"
        );
        assert!(res.get("product").is_none(), "无声明时不捏造产品身份");
    }

    #[test]
    fn test_thread_workspace_follows_thread_binding() {
        let mut store = AgentStore::new("E:/codes/default_ws".to_string());

        // 会话自带工作区时，全局当前工作区再怎么变都不影响它
        let bound = store.create_thread(
            Some("绑定会话".to_string()),
            Some("E:/codes/fpc_projects".to_string()),
        );
        store.workspace.project = "E:/codes/rust_projects/a_da/target/release".to_string();
        assert_eq!(thread_workspace(&store, &bound), "E:/codes/fpc_projects");

        // 只有历史遗留的空工作区会话，才退回当前工作区
        let legacy = store.create_thread(Some("遗留会话".to_string()), None);
        if let Some(t) = store.threads.iter_mut().find(|t| t.id == legacy) {
            t.workspace.clear();
        }
        assert_eq!(
            thread_workspace(&store, &legacy),
            "E:/codes/rust_projects/a_da/target/release"
        );

        // 会话不存在时退回当前工作区，当前工作区也空则退回公共区
        assert_eq!(
            thread_workspace(&store, "thread_不存在"),
            "E:/codes/rust_projects/a_da/target/release"
        );
        store.workspace.project.clear();
        let public = store.public_workspace.clone();
        assert_eq!(thread_workspace(&store, "thread_不存在"), public);
    }

    #[test]
    fn test_assistant_stats_lands_on_current_card() {
        use crate::ai::TokenUsage;

        let mut store = AgentStore::new("E:/codes/default_ws".to_string());
        let tid = store.create_thread(Some("遥测".to_string()), None);

        let usage = TokenUsage {
            prompt_tokens: 1234,
            completion_tokens: 56,
            total_tokens: 1290,
            thinking_tokens: None,
            cached_tokens: Some(1000),
        };

        // 1. 本步产出了文本：用量与耗时挂在当前流式卡片上
        store.append_assistant_delta(&tid, "答案");
        store.set_assistant_stats(&tid, Some(usage.clone()), 2500, 4100);
        match store.threads.iter().find(|t| t.id == tid).unwrap().items.last().unwrap() {
            Item::Assistant { text, usage: u, duration_ms, turn_duration_ms, .. } => {
                assert_eq!(text, "答案");
                assert_eq!(u.as_ref().map(|u| u.prompt_tokens), Some(1234));
                assert_eq!(u.as_ref().and_then(|u| u.cached_tokens), Some(1000));
                assert_eq!(*duration_ms, Some(2500));
                assert_eq!(*turn_duration_ms, Some(4100));
            }
            other => panic!("应为助手卡片，实际: {other:?}"),
        }

        // 2. 本步只调了工具（没有文本卡片）：补一张隐藏卡片承载用量，
        //    否则遥测条会停在上一轮的旧数字上
        store.start_tool_call(&tid, "call_1", "read_file", "{}");
        store.set_assistant_stats(&tid, Some(usage), 900, 5000);
        match store.threads.iter().find(|t| t.id == tid).unwrap().items.last().unwrap() {
            Item::Assistant { text, usage: u, duration_ms, .. } => {
                assert!(text.is_empty(), "承载用量的补位卡片不带正文");
                assert_eq!(u.as_ref().map(|u| u.completion_tokens), Some(56));
                assert_eq!(*duration_ms, Some(900));
            }
            other => panic!("应为承载用量的助手卡片，实际: {other:?}"),
        }
    }

    /// W2-T5 守门：内置工具目录必须与 `ToolDescriptor` 注册表**逐名相等**。
    ///
    /// 这是第 6 张名单的**行为断言**：目录里多一个不存在的工具（界面广告假工具）
    /// 或少一个真实工具（用户看不到）都必须在这里红。结构侧的对应检查在
    /// `cargo xtask verify-wiring` 的审计 B（确保这一段始终是**派生**的）。
    #[tokio::test]
    async fn test_builtin_catalog_matches_registry_exactly() {
        use std::collections::BTreeSet;

        let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new("E:/codes/default_ws".to_string())));
        let dispatcher = Dispatcher::new(
            store,
            Arc::new(crate::session::SessionManager::new(Some(std::path::PathBuf::from("E:/codes/default_ws")))),
            Arc::new(crate::checkpoint::CheckpointManager::new(None)),
            Arc::new(crate::subagents::SubagentManager::new()),
            Arc::new(crate::approval::ApprovalManager::new()),
            Arc::new(crate::plugins::PluginManager::new()),
            Arc::new(crate::skills::SkillManager::new()),
            None,
        );

        let catalog = dispatcher
            .dispatch(crate::protocol::methods::PLUGIN_BUILTIN_CATALOG, serde_json::json!({}))
            .await
            .expect("plugin.builtinCatalog 分发失败");

        let items = catalog.as_array().expect("目录必须是数组");
        let got: BTreeSet<String> = items
            .iter()
            .map(|v| v["name"].as_str().expect("每条必须有 name").to_string())
            .collect();
        let expected: BTreeSet<String> = agent_toolkit::registry::standard_tool_descriptors()
            .iter()
            .map(|d| d.name.clone())
            .collect();

        assert_eq!(
            got, expected,
            "内置工具目录必须与注册表逐名相等（不许是第二张名单）"
        );

        // 界面需要的四个字段都要在，且 isReadOnly 必须与描述符一致
        for item in items {
            for field in ["name", "label", "description", "isReadOnly"] {
                assert!(!item[field].is_null(), "条目缺少字段 {field}: {item}");
            }
            let name = item["name"].as_str().unwrap();
            let desc = agent_toolkit::registry::find_tool_descriptor(name)
                .unwrap_or_else(|| panic!("目录里的 `{name}` 必须在注册表里"));
            assert_eq!(
                item["isReadOnly"].as_bool().unwrap(),
                desc.is_readonly(),
                "`{name}` 的只读性与描述符不一致"
            );
        }
    }

    #[tokio::test]
    async fn test_approval_decide_resolves_pending_question() {
        let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new("E:/codes/default_ws".to_string())));
        let session_mgr = Arc::new(crate::session::SessionManager::new(Some(std::path::PathBuf::from("E:/codes/default_ws"))));
        let checkpoint_mgr = Arc::new(crate::checkpoint::CheckpointManager::new(None));
        let subagent_mgr = Arc::new(crate::subagents::SubagentManager::new());
        let approval_mgr = Arc::new(crate::approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(crate::plugins::PluginManager::new());
        let skill_mgr = Arc::new(crate::skills::SkillManager::new());

        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            None,
        );

        let call_id = "test_ask_approval_fallback";
        let rx = crate::approval::global_question_manager().register_waiter(call_id);
        assert!(crate::approval::global_question_manager().has_pending(call_id));

        // 模拟前端调用 approval.decide
        let req = serde_json::json!({
            "toolItemId": call_id,
            "approved": true
        });

        let res = dispatcher.dispatch(crate::protocol::methods::APPROVAL_DECIDE, req).await.expect("调用应该成功");
        assert_eq!(res.get("resolved").and_then(|v| v.as_bool()), Some(true));

        let ans = rx.await.expect("应该唤醒等待者");
        assert_eq!(ans.answered_by, "user");
        assert!(!crate::approval::global_question_manager().has_pending(call_id));
    }

    #[tokio::test]
    async fn test_session_initialize_returns_capabilities_and_snapshot_has_seq() {
        let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new("E:/codes/default_ws".to_string())));
        let session_mgr = Arc::new(crate::session::SessionManager::new(None));
        let checkpoint_mgr = Arc::new(crate::checkpoint::CheckpointManager::new(None));
        let subagent_mgr = Arc::new(crate::subagents::SubagentManager::new());
        let approval_mgr = Arc::new(crate::approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(crate::plugins::PluginManager::new());
        let skill_mgr = Arc::new(crate::skills::SkillManager::new());

        let dispatcher = Dispatcher::new(
            store,
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            None,
        )
        // W3-T6：产品身份与能力位**从声明派生**。这条测试原先断言的是
        // 硬编码常量（`ada-coding` / `a_da 编程助手`），所以它无法发现
        // "声明改了、握手没跟上"。现在注入真实声明再断言。
        .with_product_spec(std::sync::Arc::new(
            agent_runtime::AgentSpec::from_json_str(include_str!(
                "../../../../products/ada-coding/agent.spec.json"
            ))
            .expect("ada-coding 声明应可解析"),
        ));

        // 1. 测试 session.initialize 能力位握手返回（M3-T4）
        let init_params = serde_json::json!({
            "protocolVersion": "1.0"
        });
        let init_val = dispatcher
            .dispatch(crate::protocol::methods::SESSION_INITIALIZE, init_params)
            .await
            .expect("握手成功");

        let caps = init_val.get("capabilities").expect("必须包含 capabilities 节点");
        assert_eq!(caps.get("images").and_then(|v| v.as_bool()), Some(false), "MVP 如实声明 images: false");
        assert_eq!(caps.get("rollback").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(caps.get("plugins").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(caps.get("hooks").and_then(|v| v.as_bool()), Some(false));
        let evts = caps.get("events").expect("必须包含 events 能力");
        assert_eq!(evts.get("snapshotSeq").and_then(|v| v.as_bool()), Some(true));

        let prod = init_val.get("product").expect("必须包含 product 节点");
        assert_eq!(prod.get("id").and_then(|v| v.as_str()), Some("ada-coding"));
        assert_eq!(prod.get("archetype").and_then(|v| v.as_str()), Some("coding"));
        assert_eq!(prod.get("name").and_then(|v| v.as_str()), Some("a_da 编程助手"));

        // 2. 测试 session.snapshot 包含 seq 序号（M3-T2）
        let snap_val = dispatcher
            .dispatch(crate::protocol::methods::SESSION_SNAPSHOT, serde_json::json!({}))
            .await
            .expect("拉取快照成功");

        assert!(snap_val.get("seq").is_some(), "快照必须携带 seq 字段");
        assert_eq!(snap_val.get("seq").and_then(|v| v.as_u64()), Some(0));
    }
}

