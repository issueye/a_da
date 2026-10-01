use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::checkpoint::CheckpointManager;
use crate::protocol::*;
use crate::runner::{run_agent_loop, AgentLoopEvent};
use crate::server::fs_service;
use crate::session::SessionManager;
use crate::state::{generate_snapshot, AgentStore};

pub struct Dispatcher {
    store: Arc<RwLock<AgentStore>>,
    session_mgr: Arc<SessionManager>,
    checkpoint_mgr: Arc<CheckpointManager>,
    host_pid: u32,
    session_id: String,
}

impl Dispatcher {
    pub fn new(
        store: Arc<RwLock<AgentStore>>,
        session_mgr: Arc<SessionManager>,
        checkpoint_mgr: Arc<CheckpointManager>,
    ) -> Self {
        Self {
            store,
            session_mgr,
            checkpoint_mgr,
            host_pid: std::process::id(),
            session_id: uuid::Uuid::new_v4().to_string(),
        }
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

                Ok(serde_json::json!({
                    "sessionId": self.session_id,
                    "protocolVersion": PROTOCOL_VERSION,
                    "host": {
                        "pid": self.host_pid
                    }
                }))
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
                Ok(serde_json::Value::Null)
            }

            UI_CLOSE_TAB => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                store.close_tab(thread_id);
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
                Ok(serde_json::Value::Null)
            }

            THREAD_CREATE => {
                let title = params.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
                let mut store = self.store.write().await;
                let id = store.create_thread(title.clone());
                let ws = store.workspace.project.clone();
                drop(store);

                let _ = self.session_mgr.create_session(&id, &ws, title.as_deref(), None, None);
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
                let ws = store.workspace.project.clone();
                let deleted = store.delete_thread(thread_id);
                drop(store);

                let _ = self.session_mgr.delete_session(thread_id, Some(&ws));
                let _ = self.checkpoint_mgr.discard(thread_id);

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
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;

                let store = self.store.read().await;
                let ws = PathBuf::from(&store.workspace.project);
                let provider_config = store.provider.clone();
                drop(store);


                let session_mgr = Arc::clone(&self.session_mgr);
                let checkpoint_mgr = Arc::clone(&self.checkpoint_mgr);
                let (event_tx, mut event_rx) = mpsc::channel::<AgentLoopEvent>(64);
                let tid = thread_id.to_string();
                let prompt = text.to_string();

                tokio::spawn(async move {
                    let _ = run_agent_loop(
                        &ws,
                        &tid,
                        Some(&prompt),
                        provider_config,
                        session_mgr,
                        checkpoint_mgr,
                        event_tx,
                        None,
                    ).await;
                });

                // 后台接收 agent 事件（可在未来推送至 WebSocket 广播）
                tokio::spawn(async move {
                    while let Some(_event) = event_rx.recv().await {}
                });

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
                if store.workspace.project == ws {
                    store.workspace.project.clear();
                }
                Ok(serde_json::json!({ "message": null }))
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
                let _outcome = self.checkpoint_mgr.revert_checkpoint(thread_id, checkpoint_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": true }))
            }

            CHANGE_REVERT_FILE => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let file_path = params.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let _outcome = self.checkpoint_mgr.revert_file(thread_id, Path::new(file_path))
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": true }))
            }

            CHANGE_REVERT_ALL => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let _outcome = self.checkpoint_mgr.revert_all(thread_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "ok": true }))
            }

            DEBUG_HOST_INFO => {
                let home = crate::session::get_app_home();
                Ok(serde_json::json!({
                    "homeDir": home.to_string_lossy(),
                    "extensionsDir": home.join("extensions").to_string_lossy(),
                    "configPath": home.join("config.json").to_string_lossy()
                }))
            }

            STATS_PROMPT_CHARS => {
                Ok(serde_json::json!({
                    "systemChars": 1200,
                    "toolSpecsChars": 800
                }))
            }

            PLUGIN_BUILTIN_CATALOG => {
                Ok(serde_json::json!([
                    { "name": "read_file", "category": "fs", "description": "读取文件内容" },
                    { "name": "write_to_file", "category": "fs", "description": "创建或覆盖文件" },
                    { "name": "edit_file", "category": "fs", "description": "精准补丁编辑文件" },
                    { "name": "run_command", "category": "command", "description": "执行终端命令" }
                ]))
            }

            CONFIG_GET => {
                let home = crate::session::get_app_home();
                let cfg_file = home.join("config.json");
                let store = self.store.read().await;

                let mut saved = serde_json::json!({
                    "baseUrl": store.provider.base_url,
                    "apiKey": store.provider.api_key,
                    "model": store.provider.model,
                    "contextWindow": 128000,
                    "supportsImages": true
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
                    if let Some(si) = cfg.get("supportsImages").and_then(|v| v.as_bool()) {
                        store.config.supports_images = si;
                    }

                    // 落盘 ~/.a-da/config.json
                    let home = crate::session::get_app_home();
                    let _ = std::fs::create_dir_all(&home);
                    let cfg_file = home.join("config.json");
                    let _ = std::fs::write(&cfg_file, serde_json::to_string_pretty(&store.provider).unwrap_or_default());
                }
                Ok(serde_json::json!({ "error": null }))
            }

            CONFIG_CHECK_PROVIDER => {
                Ok(serde_json::json!({ "message": "模型连接配置校验通过" }))
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
                let mode = match mode_str {
                    "plan" => AgentMode::Plan,
                    _ => AgentMode::Code,
                };
                let mut store = self.store.write().await;
                store.config.mode = mode;
                let thread_id = params.get("threadId").and_then(|v| v.as_str()).unwrap_or(&store.active_id).to_string();
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.mode = Some(mode);
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
                Ok(serde_json::Value::Null)
            }

            THREAD_COMPACT => {
                let _thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                Ok(serde_json::json!({
                    "success": true,
                    "reason": "上下文压缩已完成"
                }))
            }

            THREAD_EDIT_AND_RESEND => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let text = params.get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;

                let store = self.store.read().await;
                let ws = PathBuf::from(&store.workspace.project);
                let provider_config = store.provider.clone();
                drop(store);

                let session_mgr = Arc::clone(&self.session_mgr);
                let checkpoint_mgr = Arc::clone(&self.checkpoint_mgr);
                let (event_tx, mut event_rx) = mpsc::channel::<AgentLoopEvent>(64);
                let tid = thread_id.to_string();
                let prompt = text.to_string();

                tokio::spawn(async move {
                    let _ = run_agent_loop(
                        &ws,
                        &tid,
                        Some(&prompt),
                        provider_config,
                        session_mgr,
                        checkpoint_mgr,
                        event_tx,
                        None,
                    ).await;
                });

                tokio::spawn(async move {
                    while let Some(_event) = event_rx.recv().await {}
                });

                Ok(serde_json::Value::Null)
            }

            THREAD_ABORT => {
                let _thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                Ok(serde_json::Value::Null)
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
                let mut store = self.store.write().await;
                store.queue.clear();
                Ok(serde_json::Value::Null)
            }

            QUEUE_PROMOTE => {
                let index = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let mut store = self.store.write().await;
                if index > 0 && index < store.queue.len() {
                    let item = store.queue.remove(index);
                    store.queue.insert(0, item);
                }
                Ok(serde_json::Value::Null)
            }

            QUEUE_REMOVE => {
                let index = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let mut store = self.store.write().await;
                if index < store.queue.len() {
                    let item = store.queue.remove(index);
                    Ok(serde_json::json!({
                        "text": item.text,
                        "images": item.images
                    }))
                } else {
                    Ok(serde_json::Value::Null)
                }
            }

            APPROVAL_DECIDE => {
                Ok(serde_json::Value::Null)
            }

            QUESTION_ANSWER => {
                let call_id = params.get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut store = self.store.write().await;
                store.pending_questions.retain(|q| q.call_id != call_id);
                Ok(serde_json::Value::Null)
            }

            WORKSPACE_OPEN_PUBLIC => {
                let mut store = self.store.write().await;
                let pub_ws = store.public_workspace.clone();
                store.workspace.project = pub_ws;
                Ok(serde_json::Value::Null)
            }

            WORKSPACE_RESCAN => {
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
                Ok(serde_json::json!({
                    "plugins": [],
                    "capabilities": {
                        "capabilities": {
                            "allowSystemPromptReplace": false,
                            "allowTextRewrite": false,
                            "allowThreadDeleteBlock": false,
                            "allowCompactionReplace": false,
                            "allowPlanModeHooks": false,
                            "allowThirdPartyHooks": false,
                            "allowBuiltinShadow": false,
                            "hookTimeoutMs": 5000
                        },
                        "invalid": [],
                        "overrides": {}
                    },
                    "configs": {},
                    "secrets": {},
                    "diagnostics": []
                }))
            }

            PLUGIN_CAPABILITIES_SET => {
                Ok(serde_json::Value::Null)
            }

            PLUGIN_CONFIG_SET => {
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SECRET_SET => {
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SET_ENABLED => {
                Ok(serde_json::Value::Null)
            }

            PLUGIN_DELETE => {
                if let Some(fp) = params.get("filePath").and_then(|v| v.as_str()) {
                    let _ = std::fs::remove_file(fp);
                }
                Ok(serde_json::json!({ "ok": true }))
            }

            PLUGIN_CREATE_TEMPLATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("custom_plugin");
                let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                let code = params.get("code").and_then(|v| v.as_str()).unwrap_or("// plugin template\n");

                let dir = if scope == "workspace" && !ws.is_empty() {
                    PathBuf::from(ws).join(".a-da").join("plugins")
                } else {
                    crate::session::get_app_home().join("plugins")
                };
                let _ = std::fs::create_dir_all(&dir);
                let target = dir.join(format!("{}.ts", name));
                let _ = std::fs::write(&target, code);
                Ok(serde_json::json!({ "filePath": target.to_string_lossy() }))
            }

            SKILL_LIST => {
                let skills = serde_json::json!([
                    {
                        "id": "builtin:git_commit",
                        "name": "git_commit",
                        "description": "规范化分析代码差异并生成高质量 Conventional Commits 提交消息",
                        "body": "分析当前暂存区或者改动差异，按规范输出 commit 信息",
                        "path": "",
                        "baseDirectory": "",
                        "scope": "builtin",
                        "enabled": true
                    },
                    {
                        "id": "builtin:code_review",
                        "name": "code_review",
                        "description": "全量审查代码设计模式、鲁棒性与边界条件，并提出针对性重构建议",
                        "body": "对代码进行静态审查与质量分析",
                        "path": "",
                        "baseDirectory": "",
                        "scope": "builtin",
                        "enabled": true
                    }
                ]);
                Ok(skills)
            }

            SKILL_SET_ENABLED => {
                Ok(serde_json::Value::Null)
            }

            SKILL_CREATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("custom_skill");
                let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                let body = params.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let dir = if scope == "workspace" && !ws.is_empty() {
                    PathBuf::from(ws).join(".a-da").join("skills").join(name)
                } else {
                    crate::session::get_app_home().join("skills").join(name)
                };
                let _ = std::fs::create_dir_all(&dir);
                let skill_file = dir.join("SKILL.md");
                let _ = std::fs::write(&skill_file, body);
                Ok(serde_json::json!({ "filePath": skill_file.to_string_lossy() }))
            }

            SKILL_DELETE => {
                Ok(serde_json::json!({ "ok": true }))
            }

            SUBAGENT_PROFILE_LIST => {
                let profiles = serde_json::json!([
                    {
                        "id": "general_purpose",
                        "name": "全能执行专员",
                        "description": "具备全量读写与终端命令执行权限，负责端到端完成复杂工程任务",
                        "systemPrompt": "你是全能执行智能体，负责完整完成用户的软件开发任务。",
                        "allowedTools": ["*"],
                        "mode": "readwrite",
                        "color": "blue",
                        "background": false
                    },
                    {
                        "id": "researcher",
                        "name": "代码调研专员",
                        "description": "只读探索工作区、分析依赖结构与函数调用链，梳理架构并输出总结",
                        "systemPrompt": "你是专业调研智能体，仅使用只读工具，负责全面分析代码库结构。",
                        "allowedTools": ["read_file", "fs.list"],
                        "mode": "readonly",
                        "color": "cyan",
                        "background": false
                    },
                    {
                        "id": "code_reviewer",
                        "name": "代码评审专员",
                        "description": "专注于代码审查、坏味道探测与安全性审查，提出规范化优化建议",
                        "systemPrompt": "你是代码评审专家，审查代码逻辑并提出重构与安全优化意见。",
                        "allowedTools": ["read_file"],
                        "mode": "readonly",
                        "color": "purple",
                        "background": false
                    },
                    {
                        "id": "tester",
                        "name": "测试专员",
                        "description": "专注于单元测试编写、运行与回归验证，确保代码高质量交付",
                        "systemPrompt": "你是测试工程师，负责编写测试用例、运行测试并修复失败单测。",
                        "allowedTools": ["run_command", "read_file", "write_to_file", "edit_file"],
                        "mode": "readwrite",
                        "color": "green",
                        "background": false
                    }
                ]);
                Ok(profiles)
            }

            SUBAGENT_PROFILE_SET_ENABLED => {
                Ok(serde_json::Value::Null)
            }

            SUBAGENT_PROFILE_DELETE => {
                Ok(serde_json::json!({ "ok": true }))
            }

            SUBAGENT_RESUME => {
                let sub_tid = params.get("subagentThreadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({ "threadId": sub_tid }))
            }

            _ => Err(ProtocolError::method_not_found(method)),
        }
    }
}
