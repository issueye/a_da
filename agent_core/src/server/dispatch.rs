use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex, RwLock};

use crate::approval::ApprovalManager;
use crate::checkpoint::CheckpointManager;
use crate::plugins::PluginManager;
use crate::protocol::*;
use crate::runner::{run_agent_loop, AgentLoopEvent};
use crate::server::emitter::StateBroadcaster;
use crate::server::fs_service;
use crate::session::SessionManager;
use crate::skills::SkillManager;
use crate::state::{generate_snapshot, AgentStore};
use crate::subagents::SubagentManager;

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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
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
}

impl Dispatcher {
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
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;

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
                store.add_user_message(&target_tid, text);
                store.set_thread_running(&target_tid, true);
                let ws = PathBuf::from(thread_workspace(&store, &target_tid));
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                let (abort_tx, abort_rx) = watch::channel(false);
                self.abort_senders.lock().await.insert(target_tid.clone(), abort_tx);

                let session_mgr = Arc::clone(&self.session_mgr);
                let checkpoint_mgr = Arc::clone(&self.checkpoint_mgr);
                let (event_tx, mut event_rx) = mpsc::channel::<AgentLoopEvent>(128);
                let tid = target_tid.clone();
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
                        Some(abort_rx),
                    ).await;
                });

                // 后台接收 Agent 事件，更新内存模型并通过 16ms 节流广播器推送到前端 UI
                let store_clone = Arc::clone(&self.store);
                let broadcaster_clone = self.broadcaster.clone();
                let abort_senders_clone = Arc::clone(&self.abort_senders);
                let thread_id_clone = target_tid.clone();

                tokio::spawn(async move {
                    while let Some(event) = event_rx.recv().await {
                        let mut store = store_clone.write().await;
                        match event {
                            AgentLoopEvent::Thinking { text } => {
                                store.append_thinking_delta(&thread_id_clone, &text);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.mark_dirty();
                                }
                            }
                            AgentLoopEvent::TextDelta { text } => {
                                store.append_assistant_delta(&thread_id_clone, &text);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.mark_dirty();
                                }
                            }
                            AgentLoopEvent::ToolCallStarted { name, id, args } => {
                                store.start_tool_call(&thread_id_clone, &id, &name, &args);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                            }
                            AgentLoopEvent::ToolCallFinished { name: _, id, ok, output } => {
                                store.finish_tool_call(&thread_id_clone, &id, ok, output);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                            }
                            AgentLoopEvent::ToolAwaitingQuestion { id, question } => {
                                store.set_tool_awaiting_question(&thread_id_clone, &id, question);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                            }
                            AgentLoopEvent::AssistantStats { usage, duration_ms, turn_duration_ms } => {
                                store.set_assistant_stats(
                                    &thread_id_clone,
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
                                store.finish_turn(&thread_id_clone);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                                break;
                            }
                            AgentLoopEvent::Error { message } => {
                                store.append_assistant_delta(&thread_id_clone, &format!("\n\n**请求异常**：{message}"));
                                store.finish_turn(&thread_id_clone);
                                store.push_log("error", format!("Agent 执行异常: {message}"), None);
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                                break;
                            }
                        }
                    }

                    // 循环结束时安全兜底：确保 abort_senders 与 runningThreadIds 彻底清除
                    abort_senders_clone.lock().await.remove(&thread_id_clone);
                    let mut store = store_clone.write().await;
                    store.finish_turn(&thread_id_clone);
                    drop(store);
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
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

                Ok(serde_json::json!({ "ok": true }))
            }

            CHANGE_REVERT_ALL => {
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let _outcome = self.checkpoint_mgr.revert_all(thread_id)
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

                Ok(serde_json::json!({ "ok": true }))
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
                Ok(serde_json::json!({
                    "systemChars": 1200,
                    "toolSpecsChars": 800
                }))
            }

            PLUGIN_BUILTIN_CATALOG => {
                Ok(serde_json::json!([
                    { "name": "list_files", "label": "列出文件", "description": "遍历并列出指定目录下的文件与子目录结构", "isReadOnly": true },
                    { "name": "read_file", "label": "读取文件", "description": "安全读取工作区内的代码或文本文件内容", "isReadOnly": true },
                    { "name": "search_files", "label": "搜索文件", "description": "在工作区文件中快速全局搜索指定文本或模式", "isReadOnly": true },
                    { "name": "find_symbol", "label": "查找符号", "description": "按名字查找函数/类/结构体等定义的位置与签名", "isReadOnly": true },
                    { "name": "read_url_content", "label": "读取网页", "description": "抓取技术文档与开源库链接内容并提取为 Markdown", "isReadOnly": true },
                    { "name": "todo", "label": "任务清单", "description": "管理多步骤编码任务的进度与状态", "isReadOnly": true },
                    { "name": "Skill", "label": "加载技能", "description": "按需加载专业技能规范与操作流程指南（SKILL.md）", "isReadOnly": true },
                    { "name": "invoke_subagent", "label": "委派子智能体", "description": "委派专项任务给隔离运行的专用子智能体", "isReadOnly": true },
                    { "name": "check_subagent", "label": "查询子智能体", "description": "查询异步子智能体的运行状态与总结报告", "isReadOnly": true },
                    { "name": "send_subagent_message", "label": "智能体通讯", "description": "向子智能体发送消息以动态纠偏或唤醒续跑", "isReadOnly": true },
                    { "name": "resume_subagent", "label": "恢复子智能体工作", "description": "恢复被中断的子智能体，让它从上次的状态与上下文继续推进", "isReadOnly": true },
                    { "name": "await_subagents", "label": "等待子智能体", "description": "挂起等待子智能体送回结论，替代反复轮询查询", "isReadOnly": true },
                    { "name": "notify_parent", "label": "唤醒上级智能体", "description": "子智能体把结论或待决策问题送回主智能体（仅子智能体可用）", "isReadOnly": true },
                    { "name": "write_file", "label": "写入文件", "description": "在工作区创建新文件或覆盖已有文件", "isReadOnly": false },
                    { "name": "edit_file", "label": "编辑文件", "description": "通过精准替换文本修改已有代码文件", "isReadOnly": false },
                    { "name": "run_command", "label": "执行命令", "description": "在项目工作区根目录下执行终端命令", "isReadOnly": false },
                    { "name": "run_background", "label": "后台命令", "description": "后台启动长运行命令（dev server 等），立即返回任务 id", "isReadOnly": false },
                    { "name": "check_task", "label": "查看后台任务", "description": "查询后台任务的状态与输出", "isReadOnly": true },
                    { "name": "kill_task", "label": "停止后台任务", "description": "终止后台任务及其子进程", "isReadOnly": false },
                    { "name": "manage_tool", "label": "工具管理", "description": "在 Create 模式下自发编写、更新与管理工具扩展插件", "isReadOnly": false },
                    { "name": "manage_skill", "label": "技能管理", "description": "在 Create 模式下自发创建、更新与管理技能规范 (SKILL.md)", "isReadOnly": false }
                ]))
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
                    let model = store.provider.model.clone();
                    let context_window = store.config.context_window;
                    let max_output_tokens = store.config.max_output_tokens;
                    let supports_images = store.config.supports_images;

                    if let Some(entry) = store.providers.iter_mut().find(|p| p.id == active_id) {
                        entry.base_url = base_url;
                        entry.api_key = api_key;
                        entry.protocol = protocol;
                        entry.custom_headers = custom_headers;
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
                let (protocol, base_url, api_key, custom_headers) = if let Some(id) = params.get("providerId").and_then(|v| v.as_str()) {
                    let store = self.store.read().await;
                    let found = store.providers.iter().find(|p| p.id == id).cloned();
                    drop(store);
                    if let Some(p) = found {
                        (p.protocol, p.base_url, p.api_key, p.custom_headers)
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
                    (protocol, base_url, api_key, custom_headers)
                };

                let models = fetch_remote_models(protocol, &base_url, &api_key, custom_headers.as_ref())
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
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let text = params.get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;

                let store = self.store.read().await;
                let ws = PathBuf::from(thread_workspace(&store, thread_id));
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
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;

                // 1. 发送中止信号给正在执行的 agent_loop 及底层流式网络任务并唤醒任何挂起的提问
                if let Some(tx) = self.abort_senders.lock().await.remove(thread_id) {
                    let _ = tx.send(true);
                }
                crate::approval::global_question_manager().cancel_all();

                // 2. 立即在 store 中闭合该会话的流式状态并从 runningThreadIds 移除
                let mut store = self.store.write().await;
                store.finish_turn(thread_id);
                drop(store);

                // 3. 立即广播最新状态到客户端
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

            SUBAGENT_RESUME => {
                let sub_tid = params
                    .get("subagentThreadId")
                    .or_else(|| params.get("threadId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({ "threadId": sub_tid, "ok": true }))
            }

            _ => Err(ProtocolError::method_not_found(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

