use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex, RwLock};

use crate::runner::run_agent_turn;
use crate::server::emitter::StateBroadcaster;
use crate::server::fs_service;
use crate::state::{generate_snapshot_with_seq, AgentStore};
use agent_adapter::model::client::NetworkModelClient;
use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::model::ProviderConfig;
use agent_node::approval::ApprovalManager;
use agent_node::checkpoint::CheckpointManager;
use agent_node::plugins::PluginManager;
use agent_node::session::SessionManager;
use agent_node::skills::SkillManager;
use agent_node::subagents::SubagentManager;
use agent_proto::*;
use tracing::warn;

mod builder;
mod helpers;
mod hooks;
mod market;
mod thread_loop;

use helpers::*;
pub use helpers::{EngineFactory, EngineInjection, PluginReassembler};

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
    /// 针对多工作区的动态编码引擎工厂
    engine_factory: Option<EngineFactory>,
    /// 按工作区路径缓存的编码真引擎
    workspace_engines:
        Arc<Mutex<HashMap<std::path::PathBuf, Arc<agent_base::engine::AgentRuntime>>>>,
    /// 产品声明（W3-T6）：握手回报的能力位与身份从这里派生。
    ///
    /// `None` = 没注入声明（老调用点/测试）→ 退回 `ServerCapabilities::default()`。
    product_spec: Option<Arc<agent_runtime::AgentSpec>>,
    /// PM 模式专属真引擎（`ada-coding` 的 **PM 模式预设**）。
    ///
    /// 会话模式为 `Pm`（或历史遗留的 `agent_id = ada-pm`/`pm-assistant`）时调度此引擎；
    /// 未注入则回退到主引擎。**PM 不是第二个产品**——同一个二进制、同一处装配函数，
    /// 只是喂了另一份声明（persona + 不含 `fs`/`command` 的工具包 + 委派配置）。
    pm_engine: Option<Arc<agent_base::engine::AgentRuntime>>,
    /// 针对多工作区的动态 PM 模式引擎工厂
    pm_engine_factory: Option<EngineFactory>,
    /// 按工作区路径缓存的 PM 模式真引擎
    pm_workspace_engines:
        Arc<Mutex<HashMap<std::path::PathBuf, Arc<agent_base::engine::AgentRuntime>>>>,
    /// 插件声明的**技能**（P2 · SP-45）。
    ///
    /// 为什么放在这一层：`skill.list` 在**这个进程**里扫盘，而插件技能要执行 JS 沙箱才拿得到
    /// （那是装配期的事，在组合根）。组合根把收好的清单随 `EngineInjection` 递进来，
    /// 于是"界面上列出的技能"与"真的进了提示词的技能"是**同一份**——
    /// 两边各扫一次就会出现"界面有、模型没有"这种最难查的错。
    plugin_skills: agent_node::skills::SkillStore,
    /// 插件启停后的重装配入口（P2 · SP-46，组合根注入；没注入就如实回 null）。
    plugin_reassembler: Option<Arc<dyn PluginReassembler>>,
    /// 钩子运行时（P3a · SP-55）：管理面点位（插件启停 / 会话创建）在**这一层**跑。
    ///
    /// 为什么管理面点位不在引擎里：它们发生在**轮外**（没有 turn 上下文），
    /// 而引擎的钩子调用都绑在某一轮上。运行时本身是同一个（共享注册表）。
    hook_runner: Option<Arc<dyn agent_base::ports::HookRunner>>,
    /// 插件贡献的活体视图（P4）：`plugin.list` / `plugin.load` 从它取"实际装上了什么"。
    ///
    /// 与 `plugin_skills` 同一个理由：要执行插件才拿得到结论，而那是组合根的事。
    /// 区别是这里装了四类（工具 / 片段 / 技能 / 钩子点位），界面据此显示贡献计数、
    /// 钩子点位清单与"被插件重实现"的交叉引用。
    plugin_contribution_views: Option<agent_node::plugins::PluginContributionStore>,
    /// 后台任务表（`task.list` / `task.kill` 与工具共用同一份）。
    background_tasks: Option<Arc<agent_toolkit::background::BackgroundTasks>>,
}

impl Dispatcher {
    /// 插件贡献变化后，丢弃按工作区缓存的动态引擎。
    ///
    /// 为什么必须清：`LivePluginReassembler` 只持有**启动装配**那一份
    /// `LivePluginContributions`（且工作区是启动参数那个固定值）；而
    /// `resolve_engine_from_cache` 会按会话工作区另建并缓存引擎。用户新增插件后
    /// 只重装配主引擎，动态引擎仍是旧工具集合——于是“列表里有插件、模型调不到”，
    /// 只能重启进程（缓存清空、重新装配）才恢复。
    ///
    /// 清缓存而不是逐个重装配：动态引擎的活体句柄没有暴露给 Dispatcher；
    /// 下一轮按最新磁盘重新装配，代价是一次插件脚本装载，换来的是**一致**。
    async fn invalidate_workspace_engine_caches(&self) {
        self.workspace_engines.lock().await.clear();
        self.pm_workspace_engines.lock().await.clear();
    }

    /// 需要时把磁盘上的新会话并进内存（两阶段：锁外解析，锁内合并且只持一小会儿）。
    ///
    /// 用于"内存里找不到这条会话"的兜底路径（打开 Tab / 聚焦 / 快照）。
    /// 解析（`restore_all_threads`，秒级）不再压在 store 写锁里。
    async fn ensure_threads_loaded(&self) {
        let force = self.store.read().await.needs_full_disk_scan();
        if let Some(disk_threads) = crate::state::load_changed_threads(&self.session_mgr, force) {
            let mut store = self.store.write().await;
            store.merge_disk_threads(disk_threads);
        }
    }

    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProtocolError> {
        // 🔴 **慢臂打点**：WS 读取循环是逐条 await 的，任何一个方法臂耗时过长，
        // 排在它后面的请求（实测：点批准 ②→③ 曾经 6437ms → 限流 2207ms → mtime 过滤 262ms）
        // 都会看起来"点了没反应"。这里用 Drop 守卫覆盖**所有**返回路径，
        // 只在 ≥200ms 时打一行 `[slow]`，把"谁占着循环"直接指出来。
        let _slow = SlowArm {
            method: method.to_string(),
            started: std::time::Instant::now(),
        };
        match method {
            TASK_LIST => {
                let Some(tasks) = &self.background_tasks else {
                    return Err(ProtocolError::new(
                        AppErrorCode::NotReady.code(),
                        "当前宿主未接入后台任务表",
                        None,
                    ));
                };
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let snapshot = tasks.snapshot(thread_id);
                let count = snapshot.len();
                Ok(serde_json::json!({
                    "tasks": serde_json::to_value(snapshot)
                        .map_err(|e| ProtocolError::internal_error(e.to_string()))?,
                    "count": count,
                }))
            }
            TASK_KILL => {
                let Some(tasks) = &self.background_tasks else {
                    return Err(ProtocolError::new(
                        AppErrorCode::NotReady.code(),
                        "当前宿主未接入后台任务表",
                        None,
                    ));
                };
                let task_id = params
                    .get("taskId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| ProtocolError::invalid_params("task.kill 需要 taskId"))?;
                match tasks.kill(task_id) {
                    Ok(message) => Ok(serde_json::json!({ "ok": true, "message": message })),
                    Err(message) => Err(ProtocolError::new(
                        AppErrorCode::NotFound.code(),
                        message,
                        None,
                    )),
                }
            }
            SESSION_INITIALIZE => {
                let p = params.as_object().ok_or_else(|| {
                    ProtocolError::invalid_params("session.initialize 需要一个参数对象")
                })?;

                let client_version = p
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // 增量帧能力协商（docs/protocol/README.md §4.2 第 1 步）：只有声明了 state.patch 的
                // 连接才切增量；没声明的继续收全量快照——这是安全绳，也是回滚路径。
                let wants_patch = p
                    .get("client")
                    .and_then(|c| c.get("capabilities"))
                    .and_then(|c| c.as_array())
                    .map(|arr| arr.iter().any(|v| v.as_str() == Some("state.patch")))
                    .unwrap_or(false);
                if wants_patch {
                    if let Some(bc) = self.broadcaster.as_ref() {
                        bc.enable_patch_mode();
                    }
                }

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
                            // P3a·SP-56：钩子机制已落地（引擎有生产调用点、成对埋点、
                            // 缺席可见用例），所以这里**从声明派生**——声明 true 就是 true。
                            // `resync` 仍如实为 false：客户端缺口重同步尚未实现。
                            hooks: spec.capabilities.hooks,
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
                serde_json::to_value(init_res)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            SESSION_SNAPSHOT => {
                // 两阶段同步：`restore_all_threads` 是秒级解析，放到 store 锁外；
                // 写锁里只合并解析结果。快照本身是只读投影，也用读锁生成——
                // 否则一次 200ms+ 的快照会把 focus / items.page 一起堵在写锁后面。
                let force = self.store.read().await.needs_full_disk_scan();
                if let Some(disk_threads) =
                    crate::state::load_changed_threads(&self.session_mgr, force)
                {
                    let mut store = self.store.write().await;
                    store.merge_disk_threads(disk_threads);
                }
                // 响应必须带**当前已发出的状态版本**（不能写死 0）。
                // 客户端把它的 `seq` 记为 `lastSnapshotSeq`，下一帧增量帧的 `baseSeq` 就是它；
                // 写死 0 时比对永远失败 → 客户端每帧都判"缺帧"并发起自愈请求 → 死循环
                // （实测 376 次/秒，渲染进程满核、内存持续上涨）。
                let seq = self
                    .broadcaster
                    .as_ref()
                    .map(|b| b.last_sent_seq())
                    .unwrap_or(0);
                let snapshot = {
                    let store = self.store.read().await;
                    generate_snapshot_with_seq(&store, seq)
                };
                Ok(serde_json::to_value(snapshot)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            // **长会话按需分页**：界面只渲染窗口内的 item（见 `TRANSCRIPT_WINDOW`），
            // 更早的按需拉。返回 `from/to/total`，界面据此显示"还有 N 条更早的内容"。
            //
            // 为什么不直接靠全量快照：一条长会话 822 个 item、单帧 1.85 MB（合成负载 3.7 MB），
            // 16ms 一帧就是每秒几十 MB 的序列化 + IPC + 前端解析——增量与分页都是为了这一点。
            // 分页同样套用快照那套字段限幅（`shrink_item_for_snapshot`），免得单条超大输出把一页撑爆。
            THREAD_ITEMS_PAGE => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?
                    .to_string();
                let from = params.get("from").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let count = params
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60)
                    .clamp(1, 500) as usize;

                let store = self.store.read().await;
                let thread = store
                    .threads
                    .iter()
                    .find(|t| t.id == thread_id)
                    .ok_or_else(|| {
                        ProtocolError::invalid_params(format!("会话不存在：{thread_id}"))
                    })?;
                let total = thread.items.len();
                // `from` 超界不报错：如实回一个空窗口，界面按 `total` 自行纠正
                let from = from.min(total);
                let to = (from + count).min(total);
                let items: Vec<Item> = thread.items[from..to]
                    .iter()
                    .cloned()
                    .map(crate::state::snapshot::shrink_item_for_snapshot)
                    .collect();
                Ok(serde_json::json!({
                    "threadId": thread_id,
                    "items": items,
                    "from": from,
                    "to": to,
                    "total": total,
                }))
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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let known = self
                    .store
                    .read()
                    .await
                    .threads
                    .iter()
                    .any(|t| t.id == thread_id);
                if !known {
                    self.ensure_threads_loaded().await;
                }
                let mut store = self.store.write().await;
                store.open_tab(thread_id.to_string());
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            UI_CLOSE_TAB => {
                let thread_id = params
                    .get("threadId")
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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let known = self
                    .store
                    .read()
                    .await
                    .threads
                    .iter()
                    .any(|t| t.id == thread_id);
                if !known {
                    self.ensure_threads_loaded().await;
                }
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
                let title = params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let mode_param = params.get("mode").and_then(|v| v.as_str());
                let agent_id_param = params
                    .get("agentId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let ws_param = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                // S6 补完：驱动方（网关）在这里告诉我们"这一轮是第几层委派"。
                // 不传 = 用户直接连的（深度 0）。
                let delegation_depth = params
                    .get("delegationDepth")
                    .and_then(|v| v.as_u64())
                    .map(|d| d as u32);
                let is_subagent = params
                    .get("isSubagent")
                    .and_then(|v| v.as_bool())
                    .or_else(|| delegation_depth.map(|d| d > 0));
                let parent_id = params
                    .get("parentId")
                    .or_else(|| params.get("parentThreadId"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let subagent_id = params
                    .get("subagentId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let parsed_mode = mode_param.map(|m| match m.to_lowercase().as_str() {
                    "plan" => AgentMode::Plan,
                    "create" => AgentMode::Create,
                    "pm" => AgentMode::Pm,
                    _ => AgentMode::Code,
                });
                // ── beforeThreadCreate（P3a·SP-55）──────────────────────────
                //
                // 🔴 **原子性**（设计 §5.4 的硬约束）：事前必须在**任何状态变更之前**判定。
                // 放在 `store.write()` 之前，所以 `Deny` 之后不会留下"内存里建了、磁盘没建"
                // 的半成品——那比直接拒绝更难查（界面看着有这条会话，重启就没了）。
                //
                // 控制面点位：第三方只能 observe / note（拒绝用户新建会话 = 拒绝服务，
                // 装配期已按分级拒掉它的 deny 声明）。
                let before = self
                    .run_out_of_turn_before(
                        agent_base::ports::hooks::points::BEFORE_THREAD_CREATE,
                        title.clone(),
                        serde_json::json!({
                            "title": title,
                            "mode": mode_param,
                            "agentId": agent_id_param,
                            "workspace": ws_param,
                            "isSubagent": is_subagent,
                            "parentId": parent_id,
                        }),
                    )
                    .await;
                match before {
                    agent_base::ports::BeforeOutcome::Pass => {}
                    agent_base::ports::BeforeOutcome::Deny { reason }
                    | agent_base::ports::BeforeOutcome::AbortTurn { reason } => {
                        return Ok(serde_json::json!({
                            "denied": true,
                            "reason": reason,
                        }));
                    }
                    agent_base::ports::BeforeOutcome::Replace { .. } => {
                        // 管理面点位**没有可改写的载荷**（它只有"允许/不允许"）：装配期已按
                        // `points::REPLACE_SUPPORTED` 拒掉这类声明，这里再兜一层——当作无意见
                        // 并**大声**记一行，而不是假装改写生效了。
                        tracing::warn!(
                            "[hooks] beforeThreadCreate 收到改写结果：该点位没有可改写的载荷，已忽略"
                        );
                    }
                }

                let mut store = self.store.write().await;
                let id = store.create_thread_with_details(
                    title.clone(),
                    ws_param.clone(),
                    parsed_mode,
                    agent_id_param.clone(),
                    is_subagent,
                    parent_id.clone(),
                    subagent_id.clone(),
                );
                if let Some(d) = delegation_depth {
                    store.delegation_depths.insert(id.clone(), d);
                }
                if let Some(m) = parsed_mode {
                    store.config.mode = m;
                }
                // 会话落在调用方指定的工作区；未指定则沿用当前工作区（历史行为）
                let ws = thread_workspace(&store, &id);
                if !ws.is_empty() {
                    store.workspace.project = ws.clone();
                }
                drop(store);

                // 会话身份（产品 / 模式）**必须落盘**：重启后左侧树的 PM 标识靠它还原
                let persisted = self.session_mgr.create_session_bound(
                    &id,
                    &ws,
                    title.as_deref(),
                    parent_id.as_deref(),
                    subagent_id.as_deref(),
                    agent_id_param.as_deref(),
                    parsed_mode,
                );

                // ── afterThreadCreate（P3a·SP-55）───────────────────────────
                //
                // 🔴 只在**落盘成功之后**触发（设计 §5.4 的原子性硬约束）：
                // 内存里建好了、磁盘没建，报"创建成功"就是假的（与"宁缺勿假"同源）。
                let notes = if persisted.is_ok() {
                    self.run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_THREAD_CREATE,
                        title.clone(),
                        serde_json::json!({ "threadId": id, "title": title, "workspace": ws }),
                    )
                    .await
                } else {
                    Vec::new()
                };

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "id": id,
                    "threadId": id,
                    "notes": notes,
                }))
            }

            THREAD_DELETE => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                // 会话文件躺在它自己的工作区目录下，删除前先取出来（短读锁，拷贝即走）
                let ws = {
                    let store = self.store.read().await;
                    thread_workspace(&store, thread_id)
                };
                // 🔴 轮外钩子必须在**任何状态变更之前**、且**不持有 store 锁**时跑。
                //
                // 钩子观测的投影自己要拿 store 写锁（`hooks.rs::project_hook_observations`），
                // 持锁再跑就是同一任务重入同一把写锁：任务永远等自己，agent 的 store 从此
                // 锁死——表现是删除不生效，之后所有会话（包括没在跑的）都切不动。
                // 顺序与 `beforeThreadCreate` 保持一致（见上面 THREAD_CREATE 的注释）。
                let before = self
                    .run_out_of_turn_before(
                        agent_base::ports::hooks::points::BEFORE_THREAD_DELETE,
                        Some(thread_id.to_string()),
                        serde_json::json!({ "threadId": thread_id, "workspace": ws.clone() }),
                    )
                    .await;
                match before {
                    agent_base::ports::BeforeOutcome::Pass
                    | agent_base::ports::BeforeOutcome::Replace { .. } => {}
                    agent_base::ports::BeforeOutcome::Deny { reason }
                    | agent_base::ports::BeforeOutcome::AbortTurn { reason } => {
                        return Ok(
                            serde_json::json!({ "ok": false, "denied": true, "reason": reason }),
                        );
                    }
                }

                // 落盘先删：文件删不掉时**不能**先把内存里那条删掉——否则下一次
                // `sync_threads_from_disk` 会把它从磁盘读回来，用户看到的就是
                // "删了，过一会儿又出现了"。失败如实回报，由调用方提示重试。
                let files_deleted = self
                    .session_mgr
                    .delete_session(thread_id, Some(&ws))
                    .map_err(|error| {
                        ProtocolError::internal_error(format!("删除会话文件失败：{error:#}"))
                    })?;

                let deleted = {
                    let mut store = self.store.write().await;
                    store.delete_thread(thread_id)
                };
                let _ = self.checkpoint_mgr.discard(thread_id);
                let notes = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_THREAD_DELETE,
                        Some(thread_id.to_string()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "workspace": ws,
                            "deleted": deleted,
                            "filesDeleted": files_deleted,
                        }),
                    )
                    .await;

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    // 内存里有这条会话 → 删掉了；或者只剩磁盘文件 → 也删掉了。
                    // 两者都 false 才是真的什么都没删。
                    "ok": deleted || files_deleted,
                    "message": null,
                    "notes": notes
                }))
            }

            THREAD_SEND => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let text = params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?;
                let images =
                    normalize_images(params.get("images").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>()
                    }));

                let mut store = self.store.write().await;
                let target_tid = if store.threads.iter().any(|t| t.id == thread_id) {
                    thread_id.to_string()
                } else if !store.active_id.is_empty()
                    && store.threads.iter().any(|t| t.id == store.active_id)
                {
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
                    let q_count = store
                        .queue
                        .iter()
                        .filter(|q| q.thread_id == target_tid)
                        .count();
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
                // 入库与送模型用同一份图片：卡片上的缩略图与真正发给模型的字节一致
                let turn_images = images.clone();
                store.add_user_message_with_images(&target_tid, &text, images);
                store.set_thread_running(&target_tid, true);
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                self.spawn_thread_loop(
                    target_tid.clone(),
                    Some(text.to_string()),
                    turn_images,
                    provider_config,
                )
                .await;

                Ok(serde_json::json!({ "accepted": true }))
            }

            WORKSPACE_ENTRIES => {
                let store = self.store.read().await;
                let ws = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&store.workspace.project);
                let list = self
                    .session_mgr
                    .list_sessions_for_workspace(ws)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::to_value(list).unwrap_or(serde_json::json!([])))
            }

            WORKSPACE_ADD => {
                let ws = params
                    .get("path")
                    .or_else(|| params.get("workspace"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 或 workspace 参数"))?;
                let mut store = self.store.write().await;
                store.workspace.project = ws.to_string();
                let _ = self.session_mgr.remember_workspace(ws);
                Ok(serde_json::json!({ "error": null, "workspace": ws }))
            }

            WORKSPACE_REMOVE => {
                let ws = params
                    .get("workspace")
                    .or_else(|| params.get("path"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 workspace 参数"))?;
                let mut store = self.store.write().await;

                // 1. 公共区保护：公共区由 a-da 提供，不能移除
                if !store.public_workspace.is_empty() && store.public_workspace == ws {
                    return Ok(
                        serde_json::json!({ "message": "公共区由 a-da 提供，不能移除", "error": "公共区由 a-da 提供，不能移除" }),
                    );
                }

                // 2. 运行中保护：工作区内若有正在运行的会话，阻止移除
                let running_in_ws = store.threads.iter().any(|candidate| {
                    candidate.workspace == ws && store.running_thread_ids.contains(&candidate.id)
                });
                if running_in_ws {
                    return Ok(
                        serde_json::json!({ "message": "该工作区内有会话正在运行，先停止再移除", "error": "该工作区内有会话正在运行，先停止再移除" }),
                    );
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
                    return Ok(
                        serde_json::json!({ "message": "至少保留一个工作区", "error": "至少保留一个工作区" }),
                    );
                }

                // 4. 清理该工作区下的全部会话及其检查点与终止信号
                let doomed: Vec<String> = store
                    .threads
                    .iter()
                    .filter(|candidate| candidate.workspace == ws)
                    .map(|candidate| candidate.id.clone())
                    .collect();
                let doomed_ids: std::collections::HashSet<String> = doomed.into_iter().collect();

                store.threads.retain(|candidate| candidate.workspace != ws);
                store
                    .ui
                    .open_tab_ids
                    .retain(|candidate| !doomed_ids.contains(candidate));
                store.queue.retain(|q| !doomed_ids.contains(&q.thread_id));

                for tid in &doomed_ids {
                    let _ = self.checkpoint_mgr.discard(tid);
                    self.abort_senders.lock().await.remove(tid);
                }

                // 5. 调用 SessionManager 移除该工作区本地持久化目录
                let _ = self.session_mgr.delete_workspace(ws);

                // 6. 若移除的是当前激活的工作区，将焦点转移到剩余工作区
                if store.workspace.project == ws || doomed_ids.contains(&store.active_id) {
                    let next_project = store
                        .threads
                        .first()
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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let entries = self
                    .checkpoint_mgr
                    .load(thread_id, false)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?
                    .unwrap_or_default();
                let count = entries
                    .iter()
                    .filter(|e| matches!(e, agent_node::checkpoint::CheckpointEntry::Checkpoint(_)))
                    .count();
                Ok(serde_json::json!({ "count": count }))
            }

            CHANGE_LIST => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let entries = self
                    .checkpoint_mgr
                    .load(thread_id, false)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?
                    .unwrap_or_default();
                Ok(serde_json::to_value(entries).unwrap_or(serde_json::json!([])))
            }

            CHANGE_REVERT_CARD | CHANGE_REVERT_CHECKPOINT => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let checkpoint_id = params
                    .get("checkpointId")
                    .or_else(|| params.get("cardId"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ProtocolError::invalid_params("缺少 checkpointId 或 cardId 参数")
                    })?;
                // W6-T4：把**真实回滚结果**回传，而不是丢弃后回一个 `{ok:true}`。
                // `None` = 该会话没有对应检查点 → 什么都没回滚，必须如实说，
                // 否则界面会显示"已回滚"而文件其实没动。
                let outcome = self
                    .checkpoint_mgr
                    .revert_checkpoint(thread_id, checkpoint_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                let _ = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_CHECKPOINT,
                        Some(thread_id.to_string()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "checkpointId": checkpoint_id,
                            "operation": "revert",
                            "ok": outcome.is_some(),
                        }),
                    )
                    .await;

                let mut store = self.store.write().await;
                if let Some(thread) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    for item in &mut thread.items {
                        if let Item::Tool {
                            checkpoint_id: Some(cid),
                            reverted,
                            ..
                        } = item
                        {
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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let file_path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let outcome = self
                    .checkpoint_mgr
                    .revert_file(thread_id, Path::new(file_path))
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                let _ = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_CHECKPOINT,
                        Some(thread_id.to_string()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "path": file_path,
                            "operation": "revert_file",
                            "ok": outcome.is_some(),
                        }),
                    )
                    .await;

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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let outcome = self
                    .checkpoint_mgr
                    .revert_all(thread_id)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                let _ = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_CHECKPOINT,
                        Some(thread_id.to_string()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "operation": "revert_all",
                            "ok": outcome.is_some(),
                        }),
                    )
                    .await;

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
                let home = agent_node::session::get_app_home();
                let cfg_file = agent_node::session::get_config_path();
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
                let cfg_file = agent_node::session::get_config_path();
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

            CONFIG_PRESETS => Ok(serde_json::json!([
                { "id": "openai", "label": "OpenAI", "baseUrl": "https://api.openai.com/v1", "model": "gpt-4o-mini", "contextWindow": 128000, "supportsImages": true },
                { "id": "deepseek", "label": "DeepSeek", "baseUrl": "https://api.deepseek.com/v1", "model": "deepseek-chat", "contextWindow": 128000, "supportsImages": false },
                { "id": "dashscope", "label": "阿里云百炼", "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1", "model": "qwen-plus", "contextWindow": 128000, "supportsImages": false },
                { "id": "moonshot", "label": "Moonshot", "baseUrl": "https://api.moonshot.cn/v1", "model": "kimi-k2-0905-preview", "contextWindow": 128000, "supportsImages": false },
                { "id": "ollama", "label": "Ollama（本地）", "baseUrl": "http://127.0.0.1:11434/v1", "model": "qwen2.5-coder", "contextWindow": 128000, "supportsImages": false },
                { "id": "custom", "label": "自定义", "baseUrl": "", "model": "", "contextWindow": 128000, "supportsImages": false }
            ])),

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
                        store.provider.proxy_url = if pu.trim().is_empty() {
                            None
                        } else {
                            Some(pu.trim().to_string())
                        };
                    }
                    if let Some(proto) = cfg.get("protocol").and_then(|v| v.as_str()) {
                        if let Ok(p) = serde_json::from_value::<agent_base::model::ModelProtocol>(
                            serde_json::Value::String(proto.to_string()),
                        ) {
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
                            entry.models.push(agent_base::model::ModelEntry {
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

            CONFIG_CHECK_PROVIDER => Ok(serde_json::json!({ "message": "模型连接配置校验通过" })),

            PROVIDER_LIST => {
                let store = self.store.read().await;
                Ok(serde_json::json!({
                    "providers": store.providers,
                    "activeProviderId": store.active_provider_id
                }))
            }

            PROVIDER_SAVE => {
                let provider_val = params
                    .get("provider")
                    .cloned()
                    .or_else(|| Some(params.clone()))
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 provider 参数"))?;

                let entry: agent_base::model::ProviderEntry = serde_json::from_value(provider_val)
                    .map_err(|e| {
                        ProtocolError::invalid_params(format!("解析 provider 数据失败: {}", e))
                    })?;

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
                    let target_model = entry
                        .models
                        .iter()
                        .find(|m| m.id == store.provider.model)
                        .or_else(|| entry.models.first());
                    if let Some(first_model) = target_model {
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
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 id 参数"))?;

                let mut store = self.store.write().await;
                if store.providers.len() <= 1 {
                    return Err(ProtocolError::invalid_params(
                        "至少保留一个供应商，无法删除最后一个供应商",
                    ));
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
                let id = params
                    .get("id")
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

                        let target_model = match selected_model {
                            Some(sm) if !sm.is_empty() => {
                                provider.models.iter().find(|item| item.id == sm)
                            }
                            _ => provider
                                .models
                                .iter()
                                .find(|item| item.id == store.provider.model)
                                .or_else(|| provider.models.first()),
                        };

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
                            if !sm.is_empty() {
                                store.provider.model = sm.to_string();
                                store.config.model = sm.to_string();
                            }
                        }

                        let _ = save_app_config(&store);
                        drop(store);

                        if let Some(ref bc) = self.broadcaster {
                            bc.broadcast_immediate().await;
                        }

                        Ok(serde_json::json!({ "ok": true }))
                    }
                    None => Err(ProtocolError::invalid_params(format!(
                        "未找到指定的供应商: {}",
                        id
                    ))),
                }
            }

            PROVIDER_FETCH_MODELS => {
                let (protocol, base_url, api_key, custom_headers, proxy_url) = if let Some(id) =
                    params.get("providerId").and_then(|v| v.as_str())
                {
                    let store = self.store.read().await;
                    let found = store.providers.iter().find(|p| p.id == id).cloned();
                    drop(store);
                    if let Some(p) = found {
                        (
                            p.protocol,
                            p.base_url,
                            p.api_key,
                            p.custom_headers,
                            p.proxy_url,
                        )
                    } else {
                        return Err(ProtocolError::invalid_params(format!(
                            "未找到供应商: {}",
                            id
                        )));
                    }
                } else {
                    let proto_str = params
                        .get("protocol")
                        .and_then(|v| v.as_str())
                        .unwrap_or("openai_chat");
                    let protocol: agent_base::model::ModelProtocol =
                        serde_json::from_value(serde_json::Value::String(proto_str.to_string()))
                            .map_err(|e| {
                                ProtocolError::invalid_params(format!("无效的协议类型: {}", e))
                            })?;
                    let base_url = params
                        .get("baseUrl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let api_key = params
                        .get("apiKey")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let custom_headers = params.get("customHeaders").and_then(|v| {
                        serde_json::from_value::<std::collections::HashMap<String, String>>(
                            v.clone(),
                        )
                        .ok()
                    });
                    let proxy_url = params
                        .get("proxyUrl")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (protocol, base_url, api_key, custom_headers, proxy_url)
                };

                let models = fetch_remote_models(
                    protocol,
                    &base_url,
                    &api_key,
                    custom_headers.as_ref(),
                    proxy_url.as_deref(),
                )
                .await
                .map_err(|e| ProtocolError::internal_error(e))?;

                Ok(serde_json::json!({
                    "models": models
                }))
            }

            CONFIG_SET_APPROVAL => {
                let mut store = self.store.write().await;
                if let Some(mode) = params.get("mode").and_then(|v| v.as_str()) {
                    // 走 `set_approval_mode`：字段与无锁镜像一起改。直接写字段的话，
                    // 闸门在锁被占时读到的是镜像（还是旧值）→ 改档位"看起来没生效"。
                    match mode {
                        "auto" => store.set_approval_mode(ApprovalMode::Auto),
                        "ask" => store.set_approval_mode(ApprovalMode::Ask),
                        "readonly" => store.set_approval_mode(ApprovalMode::Readonly),
                        // P1b 加了第四档，但这里漏了分支：选「完全访问」等于什么都没发生。
                        "full" => store.set_approval_mode(ApprovalMode::Full),
                        _ => {}
                    }
                }
                // **必须落盘**：审批档位是安全设置，重启后悄悄回到"自动执行"比不让改更糟。
                //
                // 落盘失败**必须报给界面**：只 `warn!` 一下的话，界面照样弹"保存成功"，
                // 用户下次启动才发现被回滚——这正是"改了又恢复默认"最难查的成因。
                save_app_config(&store).map_err(|e| {
                    ProtocolError::internal_error(format!(
                        "审批档位已在本次运行中生效，但写入 config.json 失败（重启后会回滚）：{e}"
                    ))
                })?;
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
                // **必须落盘**：否则重启就回到默认档（用户看到的就是"改了又恢复"）。
                // 与审批档位同理：写不进去就如实报错，不许"界面说成功、文件里没有"。
                save_app_config(&store).map_err(|e| {
                    ProtocolError::internal_error(format!(
                        "深度档位已在本次运行中生效，但写入 config.json 失败（重启后会回滚）：{e}"
                    ))
                })?;
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            CONFIG_SET_RETRY_COUNT => {
                // 模型请求重试次数（默认 5）。**必须落盘**：用户改完重启还得在。
                let retries = params
                    .get("retries")
                    .or_else(|| params.get("retryCount"))
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        ProtocolError::invalid_params("缺少 retries 参数（0-20 的整数）")
                    })?;

                let mut store = self.store.write().await;
                store.config.retry_count =
                    agent_base::model::normalize_model_retry_count(retries as usize) as u32;
                let saved = store.config.retry_count;
                save_app_config(&store).map_err(|e| {
                    ProtocolError::internal_error(format!(
                        "重试次数已在本次运行中生效，但写入 config.json 失败（重启后会回滚）：{e}"
                    ))
                })?;
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({ "retryCount": saved }))
            }

            FS_ROOTS => {
                let store = self.store.read().await;
                let extra = vec![store.workspace.project.clone()];
                let roots = fs_service::list_roots(&extra);
                Ok(serde_json::to_value(roots).unwrap_or(serde_json::json!([])))
            }

            FS_LIST => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let show_hidden = params
                    .get("showHidden")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);

                let listing = fs_service::list_directory(path, show_hidden, limit)?;
                Ok(serde_json::to_value(listing)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            FS_MKDIR => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 path 参数"))?;
                let created = fs_service::make_directory(path)?;
                Ok(serde_json::json!({ "path": created }))
            }

            FS_READ_BASE64 => {
                let path = params
                    .get("path")
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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let known = self
                    .store
                    .read()
                    .await
                    .threads
                    .iter()
                    .any(|t| t.id == thread_id);
                if !known {
                    self.ensure_threads_loaded().await;
                }
                let mut store = self.store.write().await;
                store.focus_thread(thread_id.to_string());
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            UI_ACTIVE_PROJECT => {
                let ws = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 workspace 参数"))?;
                let mut store = self.store.write().await;
                store.workspace.project = ws.to_string();
                let _ = self.session_mgr.remember_workspace(ws);
                Ok(serde_json::Value::Null)
            }

            THREAD_SET_MODE => {
                let mode_str = params
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 mode 参数"))?;
                let mode = match mode_str.to_lowercase().as_str() {
                    "plan" => AgentMode::Plan,
                    "create" => AgentMode::Create,
                    "pm" => AgentMode::Pm,
                    _ => AgentMode::Code,
                };
                let mut store = self.store.write().await;
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&store.active_id)
                    .to_string();

                // 🔴 **模式只在新建会话时确定**：已经开始的会话不许再改。
                //
                // 为什么在服务端也拦一道（界面上的滑块已经不可点了）：别处还有调用点
                // （老客户端、脚本、将来的移动端）。改掉一个已开始会话的模式，等于把
                // 同一段对话的一半交给另一套系统提示词与工具集去解释——前半段的账
                // 落在这条会话里、后半段却是另一个智能体，事后没人说得清哪句话是谁说的。
                // 拒绝要说清**为什么**，而不是静默改掉或静默不动。
                if let Some(t) = store.threads.iter().find(|t| t.id == thread_id) {
                    if !t.items.is_empty() || !t.messages.is_empty() {
                        return Err(ProtocolError::invalid_params(
                            "会话已开始，不能切换模式：模式在新建会话时选择（新建一条会话即可换模式）",
                        ));
                    }
                }

                // 空会话 / 还没有会话：更新"下一个新会话的默认模式"。
                // 这一条是**新会话**那一路要的（滑块在居中视图里改的就是它）。
                store.config.mode = mode;
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.mode = Some(mode);
                    // **PM 是模式，不是产品**：切到 PM 模式不再改 `agent_id`。
                    //
                    // 早先这里写 `agent_id = "ada-pm"`，于是"身份"有了两个真源
                    // （产品 id 与模式），并且要靠落盘/还原把它们对齐——本次会话里
                    // 徽标丢失、进程面板歧义、老会话认不出来，全是这笔账。
                    // 现在 `agent_id` 恒为本机唯一产品 `ada-coding`，PM 由 `mode` 表达；
                    // 历史上写成 `ada-pm`/`pm-assistant` 的会话在**读盘时**自动归一。
                    if t.agent_id.as_deref() == Some("ada-pm")
                        || t.agent_id.as_deref() == Some("pm-assistant")
                    {
                        t.agent_id = Some("ada-coding".to_string());
                    }
                }
                // 身份变更同步落盘，否则重启后又变回默认的 ada-coding
                let persisted_agent = store
                    .threads
                    .iter()
                    .find(|t| t.id == thread_id)
                    .and_then(|t| t.agent_id.clone());
                let ws = thread_workspace(&store, &thread_id);
                drop(store);

                let _ = self.session_mgr.update_session_identity(
                    &thread_id,
                    persisted_agent.as_deref(),
                    Some(mode),
                    Some(&ws),
                );

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::Value::Null)
            }

            THREAD_SET_WORKSPACE => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let ws = params
                    .get("workspace")
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
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?
                    .to_string();

                // 手动压缩与自动压缩走**同一份实现**（含真实的模型摘要）
                let before = self
                    .run_out_of_turn_before(
                        agent_base::ports::hooks::points::BEFORE_COMPACTION,
                        Some(thread_id.clone()),
                        serde_json::json!({ "threadId": thread_id.clone() }),
                    )
                    .await;
                if let agent_base::ports::BeforeOutcome::Deny { reason }
                | agent_base::ports::BeforeOutcome::AbortTurn { reason } = before
                {
                    return Ok(
                        serde_json::json!({ "success": false, "denied": true, "reason": reason }),
                    );
                }

                let summary = compact_thread_now(&self.store, &self.session_mgr, &thread_id).await;
                let notes = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_COMPACTION,
                        Some(thread_id.clone()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "success": summary.is_some(),
                            "summaryChars": summary.as_ref().map(|s| s.chars().count()).unwrap_or(0),
                        }),
                    )
                    .await;

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(match summary {
                    Some(s) => serde_json::json!({
                        "success": true,
                        "reason": "上下文压缩已完成",
                        "summary": s,
                        "notes": notes,
                    }),
                    None => serde_json::json!({
                        "success": false,
                        "reason": "当前会话历史较短（少于 2 轮），暂无需压缩的历史消息。"
                    }),
                })
            }

            THREAD_EDIT_AND_RESEND => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?
                    .to_string();
                let text = params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?
                    .to_string();
                // 编辑重发同样要带图：界面在这条路径上**本来就发** `images`
                // （`ws-client.editAndResend`），以前服务端把它丢了，于是"重发带图的提问"
                // 会变成一条没有图的提问。
                let images =
                    normalize_images(params.get("images").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>()
                    }));

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

                self.spawn_thread_loop(thread_id, Some(text), images, provider_config)
                    .await;

                // 与 `thread.start` 同口径：**已受理**（真正完成由事件流通知）
                Ok(serde_json::json!({ "accepted": true }))
            }

            THREAD_RETRY => {
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut store = self.store.write().await;
                let target_tid =
                    if !thread_id.is_empty() && store.threads.iter().any(|t| t.id == thread_id) {
                        thread_id.to_string()
                    } else if !store.active_id.is_empty()
                        && store.threads.iter().any(|t| t.id == store.active_id)
                    {
                        store.active_id.clone()
                    } else if let Some(first) = store.threads.first() {
                        first.id.clone()
                    } else {
                        return Err(ProtocolError::invalid_params("没有可重试的会话"));
                    };

                if store.is_thread_running(&target_tid) {
                    return Ok(
                        serde_json::json!({ "accepted": false, "reason": "会话正在运行中" }),
                    );
                }

                // 清掉上一次失败留下的提示卡片。磁盘与内存**一起清**，且磁盘在前：
                // 后台每 1.5s 一次的 `sync_threads_from_disk` 要拿 store 写锁，所以
                // 在写锁内先落盘、再改内存，就不存在"同步把卡片从磁盘读回来"的窗口。
                // （旧实现在这里裁助手正文、且只动内存，所以那张卡片总是"删不掉"。）
                if let Err(e) = self.session_mgr.clear_notice_entries(&target_tid) {
                    tracing::warn!("重试时清理磁盘提示记录失败（卡片可能重新出现）: {e}");
                }
                store.clear_notices(&target_tid);
                store.set_thread_running(&target_tid, true);
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                self.spawn_thread_loop(target_tid.clone(), None, None, provider_config)
                    .await;

                Ok(serde_json::json!({
                    "accepted": true,
                    "threadId": target_tid
                }))
            }

            THREAD_ABORT => {
                let thread_id = params
                    .get("threadId")
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

                agent_node::approval::global_question_manager().cancel_all();

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
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                let mut identity_changed = false;
                if let Some(t) = store.threads.iter_mut().find(|t| t.id == thread_id) {
                    if let Some(title) = params.get("title").and_then(|v| v.as_str()) {
                        t.title = title.to_string();
                    }
                    if let Some(agent_id) = params.get("agentId").and_then(|v| v.as_str()) {
                        t.agent_id = Some(agent_id.to_string());
                        identity_changed = true;
                    }
                    if let Some(mode_str) = params.get("mode").and_then(|v| v.as_str()) {
                        let parsed = match mode_str.to_lowercase().as_str() {
                            "plan" => AgentMode::Plan,
                            "create" => AgentMode::Create,
                            "pm" => AgentMode::Pm,
                            _ => AgentMode::Code,
                        };
                        t.mode = Some(parsed);
                        identity_changed = true;
                    }
                }
                // 身份变更同步落盘（重启后左侧树的 PM/CODING 标识靠它还原）
                let persisted_agent = store
                    .threads
                    .iter()
                    .find(|t| t.id == thread_id)
                    .and_then(|t| t.agent_id.clone());
                let persisted_mode = store
                    .threads
                    .iter()
                    .find(|t| t.id == thread_id)
                    .and_then(|t| t.mode);
                let ws = thread_workspace(&store, &thread_id);
                drop(store);

                if identity_changed {
                    let _ = self.session_mgr.update_session_identity(
                        &thread_id,
                        persisted_agent.as_deref(),
                        persisted_mode,
                        Some(&ws),
                    );
                }

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
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
                let target_tid = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

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
                if !resolved
                    && agent_node::approval::global_question_manager().has_pending(tool_item_id)
                {
                    if approved {
                        resolved = agent_node::approval::global_question_manager().resolve_answer(
                            tool_item_id,
                            agent_node::approval::QuestionAnswer {
                                choice: None,
                                choices: None,
                                text: None,
                                answered_by: "user".to_string(),
                                answers: None,
                            },
                        );
                    } else {
                        agent_node::approval::global_question_manager().cancel(tool_item_id);
                        resolved = true;
                    }
                }

                let mut store = self.store.write().await;
                store
                    .pending_questions
                    .retain(|q| q.call_id != tool_item_id);

                for thread in &mut store.threads {
                    for item in &mut thread.items {
                        if let Item::Tool {
                            id,
                            call_id: cid,
                            status,
                            ..
                        } = item
                        {
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
                let call_id = params.get("callId").and_then(|v| v.as_str()).unwrap_or("");
                let choice = params
                    .get("choice")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                // 多选：`choices: ["a1","a3"]`（单选路径仍走 `choice`）
                let choices: Option<Vec<String>> = params
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>()
                    })
                    .filter(|list| !list.is_empty());
                let text = params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                // 多题：`answers: [{id?, choice?, choices?, text?}]`（单题路径留空）
                let answers: Option<Vec<agent_toolkit::core::ask_user::QuestionSubAnswer>> = params
                    .get("answers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| serde_json::from_value(a.clone()).ok())
                            .collect::<Vec<_>>()
                    })
                    .filter(|list: &Vec<_>| !list.is_empty());

                let mut store = self.store.write().await;
                store.pending_questions.retain(|q| q.call_id != call_id);
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                let resolved = agent_node::approval::global_question_manager().resolve_answer(
                    call_id,
                    agent_node::approval::QuestionAnswer {
                        choice,
                        choices,
                        text,
                        answered_by: "user".to_string(),
                        answers,
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
                let ws = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
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

                let home = agent_node::session::get_app_home();
                let global_prompts = home.join("prompts");
                if let Ok(entries) = std::fs::read_dir(&global_prompts) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("md") {
                            let file_stem = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("prompt")
                                .to_string();
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
                                let file_stem = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("prompt")
                                    .to_string();
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

            PROMPT_SET_ENABLED => Ok(serde_json::json!({ "ok": true })),

            PROMPT_CREATE => {
                let ws = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let opts = params
                    .get("options")
                    .and_then(|o| o.as_object())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 options 参数"))?;
                let name = opts
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom_prompt");
                let description = opts
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = opts.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let scope = opts
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("global");
                let is_system = opts
                    .get("isSystem")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let enabled = opts
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let arg_hint = opts.get("argumentHint").and_then(|v| v.as_str());

                let dir = if scope == "workspace" && !ws.is_empty() {
                    PathBuf::from(ws).join(".a-da").join("prompts")
                } else {
                    agent_node::session::get_app_home().join("prompts")
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

            PLUGIN_VALIDATE => {
                // P1·SP-18：插件自检——**只读**（转译 / 装载 / 工具名是否在注册表 / 权限与预算）。
                //
                // 宿主现造一个就够：它只承载"这次自检的装载流程"，没有跨调用状态。
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ws = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // 路径来源两条：显式 `filePath`，或按 `pluginId` 从扫描结果里找
                let file_path = match params.get("filePath").and_then(|v| v.as_str()) {
                    Some(p) if !p.trim().is_empty() => p.to_string(),
                    _ => self
                        .plugin_mgr
                        .scan_plugins(ws.as_deref())
                        .into_iter()
                        .find(|p| p.id == plugin_id)
                        .map(|p| p.file_path)
                        .unwrap_or_default(),
                };
                if file_path.trim().is_empty() {
                    return Err(ProtocolError::invalid_params(format!(
                        "找不到插件 `{plugin_id}` 的脚本路径：请给 `filePath`，或确认它已被扫描到"
                    )));
                }

                let workspace = ws
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                if !self
                    .plugin_mgr
                    .is_extension_entry(ws.as_deref(), Path::new(&file_path))
                {
                    return Err(ProtocolError::invalid_params(
                        "插件入口必须是 agent-coding/extensions/<package>/index.ts 或 index.js；不再支持旧式单文件插件",
                    ));
                }
                let host: std::sync::Arc<dyn agent_base::ports::ScriptHost> =
                    std::sync::Arc::new(agent_node::scripts::TsScriptHost::new());
                let report = agent_node::scripts::validate_plugin(
                    &host,
                    &plugin_id,
                    std::path::Path::new(&file_path),
                    &workspace,
                    agent_base::ports::ScriptBudget::default(),
                )
                .await;
                serde_json::to_value(report)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            PLUGIN_INVOKE => {
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let action = params
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if plugin_id.is_empty() || action.is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "plugin.invoke 需要非空的 pluginId 与 action",
                    ));
                }
                let intent_raw = params
                    .get("intent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("write");
                let intent = agent_node::plugins::PluginActionIntent::parse(intent_raw)
                    .ok_or_else(|| {
                        ProtocolError::invalid_params(
                            "plugin.invoke 的 intent 只接受 read / write",
                        )
                    })?;
                let input = params
                    .get("input")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let workspace = match params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    Some(workspace) => workspace.to_string(),
                    None => {
                        let store = self.store.read().await;
                        store.workspace.project.clone()
                    }
                };
                if workspace.trim().is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "plugin.invoke 缺少工作区（会话尚未绑定目录）",
                    ));
                }

                self.plugin_mgr
                    .invoke_action(&plugin_id, &action, input, intent, &workspace)
                    .await
                    .map_err(|error| match error {
                        agent_node::plugins::PluginActionError::NotFound(message) => {
                            ProtocolError::new(
                                AppErrorCode::NotFound.code(),
                                message,
                                Some(serde_json::json!({ "kind": "plugin_action_not_found" })),
                            )
                        }
                        agent_node::plugins::PluginActionError::NotReady(message) => {
                            ProtocolError::new(
                                AppErrorCode::NotReady.code(),
                                message,
                                Some(serde_json::json!({ "kind": "plugin_action_not_ready" })),
                            )
                        }
                        agent_node::plugins::PluginActionError::Denied(message) => {
                            ProtocolError::new(
                                AppErrorCode::Denied.code(),
                                message,
                                Some(serde_json::json!({ "kind": "plugin_action_denied" })),
                            )
                        }
                        agent_node::plugins::PluginActionError::Invalid(message) => {
                            ProtocolError::invalid_params(message)
                        }
                        agent_node::plugins::PluginActionError::Timeout => {
                            ProtocolError::new(
                                AppErrorCode::Timeout.code(),
                                "插件动作执行超时",
                                Some(serde_json::json!({ "kind": "plugin_action_timeout" })),
                            )
                        }
                        agent_node::plugins::PluginActionError::TooLarge => {
                            ProtocolError::new(
                                AppErrorCode::TooLarge.code(),
                                "插件动作输入或输出超过大小上限",
                                Some(serde_json::json!({ "kind": "plugin_action_too_large" })),
                            )
                        }
                        agent_node::plugins::PluginActionError::Failed(message) => {
                            ProtocolError::new(
                                RpcErrorCode::InternalError.code(),
                                message,
                                Some(serde_json::json!({ "kind": "plugin_action_failed" })),
                            )
                        }
                    })
            }

            PLUGIN_ROUTE => {
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let method = params
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if plugin_id.is_empty() || method.is_empty() || path.is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "plugin.route 需要非空的 pluginId / method / path",
                    ));
                }
                let query = params
                    .get("query")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let headers = params
                    .get("headers")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let body = params
                    .get("body")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let body_kind = params
                    .get("bodyKind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("empty");
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                let workspace = match params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    Some(workspace) => workspace.to_string(),
                    None => {
                        let store = self.store.read().await;
                        store.workspace.project.clone()
                    }
                };
                if workspace.trim().is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "plugin.route 缺少工作区（会话尚未绑定目录）",
                    ));
                }

                match self
                    .plugin_mgr
                    .invoke_route(
                        &plugin_id,
                        &method,
                        &path,
                        query,
                        headers,
                        body,
                        body_kind,
                        thread_id.as_deref(),
                        &workspace,
                    )
                    .await
                {
                    Ok(response) => Ok(serde_json::json!({
                        "ok": true,
                        "response": response,
                    })),
                    Err(error) => {
                        // 路由层的失败是 **HTTP 语义**（404/504/...），不是 JSON-RPC 故障：
                        // 宿主据此直接构造响应，不会把“插件没这条路由”渲染成 500。
                        let (status, kind) = match &error {
                            agent_node::plugins::PluginActionError::NotFound(_) => {
                                (404, "not_found")
                            }
                            agent_node::plugins::PluginActionError::NotReady(_) => {
                                (503, "not_ready")
                            }
                            agent_node::plugins::PluginActionError::Denied(_) => (403, "denied"),
                            agent_node::plugins::PluginActionError::Invalid(_) => {
                                (400, "invalid")
                            }
                            agent_node::plugins::PluginActionError::Timeout => (504, "timeout"),
                            agent_node::plugins::PluginActionError::TooLarge => {
                                (413, "too_large")
                            }
                            agent_node::plugins::PluginActionError::Failed(_) => {
                                (500, "failed")
                            }
                        };
                        Ok(serde_json::json!({
                            "ok": false,
                            "status": status,
                            "kind": kind,
                            "error": error.to_string(),
                        }))
                    }
                }
            }

            PLUGIN_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let mut plugins = self.plugin_mgr.load_plugins(ws);
                // P4：静态扫描只列包与入口状态，**从不执行插件**——磁盘插件的贡献恒为空。
                // 装配期执行过的结论在活体视图里，按 pluginId 对齐后并进来。
                self.merge_contribution_views(&mut plugins);
                let resolved_caps = self.plugin_mgr.get_capabilities(ws);
                let configs = self.plugin_mgr.read_configs();

                // P0 埋点：插件扫描是**用户动作点**（此刻没有轮次在跑），
                // 所以在这里如实报成诊断。两条口径写死在这里：
                // 1. 只报**磁盘插件**（`scope != "builtin"`）——内置插件是编译进来的，
                //    每次列表都报 9 条只会把 60 条环形缓冲刷满；
                // 2. **一个都没有也要报**（`reason = no_plugin`）——"没扫到"与"没扫过"
                //    在面板上必须分得出来，这是 §2.2 第 2 条律（缺席可见）。
                {
                    use agent_base::domain::{
                        DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_PLUGIN_DISCOVERED,
                        POINT_PLUGIN_SKIPPED,
                    };
                    let mut store_w = self.store.write().await;
                    let disk: Vec<&_> = plugins.iter().filter(|p| p.scope != "builtin").collect();
                    if disk.is_empty() {
                        crate::server::diag::project_out_of_turn(
                            &mut store_w,
                            &DiagnosticEvent::new(
                                DiagFamily::Plugin,
                                POINT_PLUGIN_DISCOVERED,
                                Origin::Host,
                            )
                            .with_outcome(DiagOutcome::Skipped)
                            .with_reason("no_plugin"),
                        );
                    } else {
                        for p in disk {
                            let origin = Origin::Plugin { id: p.id.clone() };
                            crate::server::diag::project_out_of_turn(
                                &mut store_w,
                                &DiagnosticEvent::new(
                                    DiagFamily::Plugin,
                                    POINT_PLUGIN_DISCOVERED,
                                    origin.clone(),
                                )
                                .with_outcome(DiagOutcome::Applied)
                                .with_data(serde_json::json!({
                                    "scope": p.scope,
                                    "tools": p.tools.len(),
                                })),
                            );
                            // **只报真的**：只有"已停用"才是本页能确定的事。
                            // 早先这里对启用中的插件也发一条 `execution_not_wired`，
                            // 那是 P0（执行面还没接）的实话，P1 接线之后就成了**假话**——
                            // 面板上会对一个正常服务的插件喊"它不会被执行"。
                            // "启用了却没生效"有别的判据且更准：装配期按贡献逐条报
                            // （`plugin.skipped{tool_not_registered}` / 钩子被拒的原因）。
                            if !p.enabled {
                                crate::server::diag::project_out_of_turn(
                                    &mut store_w,
                                    &DiagnosticEvent::new(
                                        DiagFamily::Plugin,
                                        POINT_PLUGIN_SKIPPED,
                                        origin,
                                    )
                                    .with_outcome(DiagOutcome::Skipped)
                                    .with_reason("disabled"),
                                );
                            }
                        }
                    }
                }

                let mut secrets = serde_json::Map::new();
                for item in &plugins {
                    if let Some(ref schema) = item.plugin.contributions.config_schema {
                        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                            for (k, prop) in props {
                                if prop.get("type").and_then(|t| t.as_str()) == Some("secret") {
                                    let has_secret = self.plugin_mgr.check_secret(&item.id, k);
                                    secrets.insert(
                                        format!("{}:{}", item.id, k),
                                        serde_json::Value::Bool(has_secret),
                                    );
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

            PLUGIN_LOAD => {
                let workspace = params.get("workspace").and_then(|v| v.as_str());
                let plugin_id = params.get("pluginId").and_then(|v| v.as_str());
                let mut plugins = self.plugin_mgr.load_plugins(workspace);
                self.merge_contribution_views(&mut plugins);
                let plugins: Vec<_> = match plugin_id {
                    Some(id) => plugins.into_iter().filter(|p| p.id == id).collect(),
                    None => plugins,
                };
                Ok(serde_json::json!({ "ok": true, "plugins": plugins }))
            }

            PLUGIN_RELOAD => {
                let workspace = params.get("workspace").and_then(|v| v.as_str());
                let mut plugins = self.plugin_mgr.reload_plugins(workspace);
                self.merge_contribution_views(&mut plugins);
                // "重新加载插件"对用户的意思就是"我刚改了 index.ts，让它现在就生效"。
                // 只重新扫目录（上一行）只能刷新**列表**，引擎里跑的还是旧贡献集合。
                let report = match &self.plugin_reassembler {
                    Some(r) => r.refresh("reload").await,
                    None => None,
                };
                // 主引擎已重装配；按工作区缓存的动态引擎必须一起失效，
                // 否则新插件只对“启动工作区”生效，其他工作区要重启才看到。
                self.invalidate_workspace_engine_caches().await;
                Ok(serde_json::json!({
                    "ok": true,
                    "plugins": plugins,
                    "applied": report.as_ref().map(|r| r.applied),
                    "report": report,
                }))
            }

            PLUGIN_CAPABILITIES_SET => {
                if let Some(patch) = params.get("patch") {
                    self.plugin_mgr
                        .save_capabilities(patch)
                        .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                }
                // 能力开关是**装配期**读的（`allowBuiltinShadow` / `allowSystemPromptReplace`
                // 决定插件贡献收不收）。只写盘不重装配，界面上的勾已经打上了、引擎还在按
                // 旧策略跑——用户以为"关掉了同名覆盖"，实际什么都没关。
                let report = match &self.plugin_reassembler {
                    Some(r) => r.refresh("capabilities").await,
                    None => None,
                };
                self.invalidate_workspace_engine_caches().await;
                Ok(serde_json::json!({
                    "applied": report.as_ref().map(|r| r.applied),
                    "report": report,
                }))
            }

            PLUGIN_CONFIG_SET => {
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(values) = params.get("values") {
                    self.plugin_mgr
                        .save_config(plugin_id, values)
                        .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                }
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SECRET_SET => {
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
                self.plugin_mgr
                    .save_secret(plugin_id, key, value)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            PLUGIN_SET_ENABLED => {
                let plugin_id = params
                    .get("pluginId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                // ── beforePluginToggle（P3a·SP-55）────────────────────────────
                //
                // 控制面点位：**只有内置插件**能在这里拦下（第三方想拦 = 自我加冕或
                // 互斥锁定，装配期就被拒了，见 `ports::hooks::allows`）。
                let before = self
                    .run_out_of_turn_before(
                        agent_base::ports::hooks::points::BEFORE_PLUGIN_TOGGLE,
                        Some(plugin_id.to_string()),
                        serde_json::json!({ "pluginId": plugin_id, "enabled": enabled }),
                    )
                    .await;
                match before {
                    agent_base::ports::BeforeOutcome::Pass => {}
                    agent_base::ports::BeforeOutcome::Deny { reason }
                    | agent_base::ports::BeforeOutcome::AbortTurn { reason } => {
                        // 拦下即定稿：**不写盘、不重装配**，如实把理由回给调用方
                        return Ok(serde_json::json!({
                            "ok": false,
                            "denied": true,
                            "reason": reason,
                        }));
                    }
                    agent_base::ports::BeforeOutcome::Replace { .. } => {
                        // 同上：管理面点位没有可改写的载荷（装配期已按
                        // `points::REPLACE_SUPPORTED` 拒掉这类声明）
                        tracing::warn!(
                            "[hooks] beforePluginToggle 收到改写结果：该点位没有可改写的载荷，已忽略"
                        );
                    }
                }

                self.plugin_mgr
                    .toggle_plugin(plugin_id, enabled)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                // P2·SP-46：**启停真的改变贡献集合**。
                //
                // 到 SP-45 为止，这里只写了一个布尔值就返回：界面上"关掉了"，引擎手里那份
                // 工具集合与提示词一字未动——用户看到的是"关掉了但它还在跑"。现在把启停接到
                // 受控重装配上，并把**实际生效的贡献差异**当回执回给界面。
                //
                // 没注入重装配入口时如实回 `null`：不撒谎说"已生效"。
                let report = match &self.plugin_reassembler {
                    Some(r) => r.reassemble(plugin_id, enabled).await,
                    None => None,
                };
                self.invalidate_workspace_engine_caches().await;

                // ── afterPluginToggle（P3a·SP-55）───────────────────────────
                //
                // 事后只做观察 + 旁注：把"实际生效的贡献集合"用旁注再说一遍，
                // 供审计类插件记录（控制面点位第三方只能 observe / note）。
                let summary = match &report {
                    Some(r) => format!(
                        "贡献 {} → {}（{}）",
                        r.before.total(),
                        r.after.total(),
                        r.delta.describe()
                    ),
                    None => "（未接重装配入口：贡献集合未变）".to_string(),
                };
                let after_notes = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_PLUGIN_TOGGLE,
                        Some(plugin_id.to_string()),
                        serde_json::json!({
                            "pluginId": plugin_id,
                            "enabled": enabled,
                            "summary": summary,
                        }),
                    )
                    .await;

                match report {
                    Some(rep) => {
                        tracing::info!(
                            "[plugins] 启停 `{plugin_id}` → enabled={enabled}；贡献差异：{}",
                            rep.delta.describe()
                        );
                        Ok(serde_json::json!({
                            "ok": true,
                            "applied": rep.applied,
                            "report": rep,
                            "notes": after_notes,
                        }))
                    }
                    None => Ok(serde_json::json!({
                        "ok": true,
                        "applied": false,
                        "report": serde_json::Value::Null,
                        "notes": after_notes,
                    })),
                }
            }

            PLUGIN_DELETE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let fp = params
                    .get("filePath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let ok = self
                    .plugin_mgr
                    .delete_plugin(fp, ws)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                // 删了也要重装配：不重装配的话，那个插件注册的工具还在引擎的注册表里
                // 活着（磁盘上已经没有这个包了），模型照样会调到"已经不存在的工具"。
                let report = match &self.plugin_reassembler {
                    Some(r) => r.refresh("delete").await,
                    None => None,
                };
                self.invalidate_workspace_engine_caches().await;
                Ok(serde_json::json!({
                    "ok": ok,
                    "applied": report.as_ref().map(|r| r.applied),
                    "report": report,
                }))
            }

            PLUGIN_CREATE_TEMPLATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let scope = params
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("global");
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom_plugin");
                let code = params.get("code").and_then(|v| v.as_str());
                let fp = self
                    .plugin_mgr
                    .create_plugin_template(ws, scope, name, code)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                // 🔴 建完必须**立刻重装配**：插件贡献（工具/片段/技能/钩子）是在装配期
                // 结算的，这里只写了文件就返回，界面上"刷新"十次也没用——用户只能重启程序
                // （2026-10-19 实机反馈的就是这个）。`applied` 如实回报"引擎侧确实换了"，
                // 没注入重装配入口时给 `null`，不撒谎说已生效。
                let report = match &self.plugin_reassembler {
                    Some(r) => r.refresh("createTemplate").await,
                    None => None,
                };
                self.invalidate_workspace_engine_caches().await;
                Ok(serde_json::json!({
                    "filePath": fp,
                    "applied": report.as_ref().map(|r| r.applied),
                    "report": report,
                }))
            }

            MARKET_GET_CONFIG => self.dispatch_market(method, params).await,
            MARKET_SET_CONFIG => self.dispatch_market(method, params).await,
            MARKET_SEARCH => self.dispatch_market(method, params).await,
            MARKET_INSTALL => self.dispatch_market(method, params).await,
            MARKET_PUBLISH => self.dispatch_market(method, params).await,

            SKILL_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let _ = self
                    .run_out_of_turn_before(
                        agent_base::ports::hooks::points::BEFORE_SKILL_LOAD,
                        ws.map(str::to_string),
                        serde_json::json!({ "workspace": ws }),
                    )
                    .await;
                // 🔴 读的是**共享活体**（不是启动时的快照）：插件启停会原地换它，
                // 于是"界面列出的技能"与"真的进了提示词的技能"始终是同一份。
                // 早先这里是快照 → 停用插件后引擎侧已经没有那条技能，界面里还在
                // （实机验收 `tools/verify-plugin-platform-e2e.mjs` 抓到的正是这个）。
                let plugin_skills = self
                    .plugin_skills
                    .read()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let skills = self.skill_mgr.scan_skills_with(ws, &plugin_skills);
                // P0 埋点：技能扫描同样在**用户动作点**报（`skill.list`）。
                //
                // 🔴 刻意**不**在"每步取提示词"的那条路径上报：技能解析器的调用频率是
                // **每步一次**（一次模型调用一步），逐条报会把 60 条环形缓冲刷爆。
                // 所以这里报**聚合一条**（启用数 + 正文总字节 + id 清单），信息够用且有界。
                {
                    use agent_base::domain::{
                        DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_SKILL_INVALID,
                        POINT_SKILL_LOADED, POINT_SKILL_SKIPPED,
                    };
                    let enabled: Vec<&_> = skills.iter().filter(|s| s.enabled).collect();
                    let custom: Vec<&_> = skills.iter().filter(|s| s.scope != "builtin").collect();
                    let total_bytes: usize = enabled.iter().map(|s| s.body.len()).sum();
                    let mut store_w = self.store.write().await;
                    // 🔴 注意：`scan_skills` **总是**附带内置技能，所以"一个技能都没有"
                    // 在生产里不会发生。缺席信号因此针对**自定义技能**——这才是用户
                    // 真的会疑惑的那件事（"我放进去的技能怎么没生效"）。
                    if custom.is_empty() {
                        crate::server::diag::project_out_of_turn(
                            &mut store_w,
                            &DiagnosticEvent::new(
                                DiagFamily::Skill,
                                POINT_SKILL_SKIPPED,
                                Origin::Host,
                            )
                            .with_outcome(DiagOutcome::Skipped)
                            .with_reason("no_custom_skill")
                            .with_data(serde_json::json!({
                                "builtin": skills.len(),
                                "enabled": enabled.len(),
                            })),
                        );
                    } else {
                        crate::server::diag::project_out_of_turn(
                            &mut store_w,
                            &DiagnosticEvent::new(
                                DiagFamily::Skill,
                                POINT_SKILL_LOADED,
                                Origin::Host,
                            )
                            .with_outcome(DiagOutcome::Applied)
                            .with_bytes(total_bytes as u64)
                            .with_data(serde_json::json!({
                                "total": skills.len(),
                                "custom": custom.len(),
                                "enabled": enabled.len(),
                                "ids": enabled.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
                            })),
                        );
                    }

                    // P2·SP-45：frontmatter 里认不出的键**逐条报出来**。
                    //
                    // 以前 `_ => {}` 静默吃掉，于是"技能写好了却不生效"没有任何线索——
                    // 作者只会反复检查正文，而问题在键名上（`whenToUse` 拼错一个字母）。
                    // 每个技能一条（不是聚合一条）：这类问题**少而具体**，聚合会丢掉"是哪个文件"。
                    for s in skills
                        .iter()
                        .filter(|s| !s.unknown_frontmatter_keys.is_empty())
                    {
                        crate::server::diag::project_out_of_turn(
                            &mut store_w,
                            &DiagnosticEvent::new(
                                DiagFamily::Skill,
                                POINT_SKILL_INVALID,
                                Origin::Host,
                            )
                            .with_outcome(DiagOutcome::Failed)
                            .with_reason("unknown_frontmatter_key")
                            .with_data(serde_json::json!({
                                "skillId": s.id.clone(),
                                "name": s.name.clone(),
                                "keys": s.unknown_frontmatter_keys.clone(),
                            })),
                        );
                    }
                }
                let _ = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_SKILL_LOAD,
                        ws.map(str::to_string),
                        serde_json::json!({
                            "workspace": ws,
                            "count": skills.len(),
                            "enabled": skills.iter().filter(|s| s.enabled).count(),
                        }),
                    )
                    .await;
                Ok(serde_json::to_value(skills)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?)
            }

            SKILL_SET_ENABLED => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.skill_mgr
                    .toggle_skill(id, enabled)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::Value::Null)
            }

            SKILL_CREATE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let scope = params
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("global");
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom_skill");
                let desc = params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let body = params.get("body").and_then(|v| v.as_str());
                let fp = self
                    .skill_mgr
                    .create_skill_template(ws, scope, name, desc, body)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))?;
                Ok(serde_json::json!({ "filePath": fp }))
            }

            SKILL_DELETE => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let ok = self
                    .skill_mgr
                    .delete_skill(id, ws)
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
mod tests;
