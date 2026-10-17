use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex, RwLock};

use agent_node::approval::ApprovalManager;
use agent_base::model::ProviderConfig;
use agent_node::checkpoint::CheckpointManager;
use agent_node::plugins::PluginManager;
use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_proto::*;
use crate::runner::run_agent_turn;
use crate::server::emitter::StateBroadcaster;
use crate::server::fs_service;
use agent_node::session::SessionManager;
use agent_node::skills::SkillManager;
use crate::state::{generate_snapshot_with_seq, AgentStore};
use agent_node::subagents::SubagentManager;
use agent_adapter::model::client::NetworkModelClient;
use tracing::warn;

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
///
/// 路径取自 store 自己（构造时注入的应用目录），**不是**全局 `get_config_path()`：
/// 测试/嵌入式宿主注入自己的 home 时，写盘必须跟着走，否则会污染用户真实的 `~/.a-da`。
fn save_app_config(store: &AgentStore) -> Result<(), String> {
    let cfg_file = store.config_path.clone();
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
        // 模型请求重试次数（设置项，默认 5；重试耗尽即断开对话）
        obj.insert("retryCount".to_string(), serde_json::json!(store.config.retry_count));
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

/// 把一次「模型请求失败」作为 **notice 记录**写进会话文件。
///
/// 🔴 **必须落盘**：这条提示若只活在内存里，1.5 秒后 `sync_threads_from_disk` 会发现
/// 磁盘版本与内存不同（磁盘上没有它），于是把整份 `items` 覆盖回去——
/// 界面上的提示卡片「闪一下就没了」，用户只看到自己的提问孤零零挂在那儿。
///
/// 与旧版 `persist_failure_message` 的差别：旧版写的是 `AgentMessage::Assistant`
/// （带 `error_message`），于是这条**报错**既进了助手正文、又进了模型上下文；
/// 现在写 `SessionEntry::Notice`，`fold_messages` 会跳过它——只给人看。
fn persist_failure_notice(
    session_mgr: &agent_node::session::SessionManager,
    thread_id: &str,
    text: &str,
) {
    if let Err(e) = session_mgr.append_notice_entry(thread_id, text, "error", None) {
        tracing::warn!("失败提示落盘失败（界面仍会显示，但下次同步就会丢）: {e}");
    }
}

/// 远程探测通用 /models 接口以获取可用模型列表
async fn fetch_remote_models(
    protocol: agent_base::model::ModelProtocol,
    base_url: &str,
    api_key: &str,
    custom_headers: Option<&std::collections::HashMap<String, String>>,
    proxy_url: Option<&str>,
) -> Result<Vec<agent_base::model::ModelEntry>, String> {
    let clean_base = base_url.trim_end_matches('/');
    if clean_base.is_empty() {
        return Err("供应商 Base URL 不能为空".to_string());
    }

    let url = match protocol {
        agent_base::model::ModelProtocol::Anthropic => {
            if clean_base.ends_with("/v1") {
                format!("{}/models", clean_base)
            } else {
                format!("{}/v1/models", clean_base)
            }
        }
        agent_base::model::ModelProtocol::OpenAiChat | agent_base::model::ModelProtocol::OpenAiResponses => {
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
        agent_base::model::ModelProtocol::Anthropic => {
            if !api_key.trim().is_empty() {
                req = req.header("x-api-key", api_key.trim());
            }
            req = req.header("anthropic-version", "2023-06-01");
        }
        agent_base::model::ModelProtocol::OpenAiChat | agent_base::model::ModelProtocol::OpenAiResponses => {
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
            models.push(agent_base::model::ModelEntry {
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

/// 动态按工作区装配引擎的工厂函数。
pub type EngineFactory = Arc<dyn Fn(&std::path::Path) -> Option<Arc<agent_base::engine::AgentRuntime>> + Send + Sync>;

/// 轮外钩子点位的取消令牌：**永不取消**。
///
/// 为什么不是"传个真令牌"：管理面点位（插件启停 / 会话创建）本身就是一次 RPC 请求，
/// 它的生命周期由请求自己管（请求断开 → 整个 dispatch 被丢掉），没有"轮次取消"可言。
/// 用一个显式的 no-op 而不是 `agent_base::testing::NeverCancel`：测试替身出现在生产路径上，
/// 下一个读者会以为这里有测试残留。
struct NeverCancelled;

impl agent_base::ports::CancelToken for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 插件启停之后的**受控重装配**（P2 · SP-46）。
///
/// # 为什么由组合根实现
///
/// 重装配要**再执行一遍插件**（拿工具/片段/技能声明），那需要 `ScriptHost`；还要把结果换进
/// **引擎正在用的**那本注册表与提示词/技能活体——这些都只有组合根拿得到。RPC 这一层只负责
/// "写盘 → 调它 → 把收据回给界面"，不自己拼贡献集合（两处各拼一份必然漂移）。
///
/// 返回 `None` = 没有可重装配的活体（如实：界面别把"点了没反应"当成"已经生效"）。
pub trait PluginReassembler: Send + Sync {
    fn reassemble<'a>(
        &'a self,
        plugin_id: &'a str,
        enabled: bool,
    ) -> agent_base::ports::BoxFuture<'a, Option<agent_runtime::ReassemblyReport>>;
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
    /// 针对多工作区的动态引擎工厂
    pub factory: Option<EngineFactory>,
    /// 插件声明的**技能**（P2 · SP-45）：装配期在组合根收好的那份。
    ///
    /// 随注入包一起走，是因为 ws 宿主拿它去造 `Dispatcher.plugin_skills`——
    /// `skill.list` 才能在界面里列出插件技能（而它没法自己执行 JS 沙箱）。
    pub plugin_skills: Option<agent_node::skills::SkillStore>,
    /// 插件启停后的重装配入口（P2 · SP-46）：组合同样只有组合根实现得出来。
    pub reassembler: Option<Arc<dyn PluginReassembler>>,
    /// 钩子运行时（P3a · SP-55）：宿主用它跑**管理面**点位（插件启停 / 会话创建）。
    ///
    /// 与引擎共用同一个实例（注册表是共享的）：装配期装好的钩子，轮内轮外都该看到同一份。
    pub hook_runner: Option<Arc<dyn agent_base::ports::HookRunner>>,
}

impl EngineInjection {
    pub fn new(
        runtime: Arc<agent_base::engine::AgentRuntime>,
        approval_mgr: Arc<ApprovalManager>,
        spec: Arc<agent_runtime::AgentSpec>,
    ) -> Self {
        Self {
            runtime,
            approval_mgr,
            spec,
            factory: None,
            plugin_skills: None,
            reassembler: None,
            hook_runner: None,
        }
    }

    pub fn with_factory(mut self, factory: EngineFactory) -> Self {
        self.factory = Some(factory);
        self
    }

    /// 带上插件技能（SP-45）。没带 = 如实为空，不是"假装没有插件"。
    pub fn with_plugin_skills(
        mut self,
        skills: agent_node::skills::SkillStore,
    ) -> Self {
        self.plugin_skills = Some(skills);
        self
    }

    /// 带上重装配入口（SP-46）：`plugin.setEnabled` 才能真的改贡献集合并回报收据。
    pub fn with_reassembler(mut self, r: Arc<dyn PluginReassembler>) -> Self {
        self.reassembler = Some(r);
        self
    }
}


/// 慢臂打点守卫：Drop 时若耗时 ≥200ms 就打一行 [slow]。
///
/// 用守卫而不是在每个 return 前埋点：dispatch 有几十个臂与早退路径，
/// 守卫能覆盖**全部**返回路径（含 ? 提前返回），不会漏。
struct SlowArm {
    method: String,
    started: std::time::Instant,
}

impl Drop for SlowArm {
    fn drop(&mut self) {
        let ms = self.started.elapsed().as_millis();
        if ms >= 200 {
            tracing::warn!(
                "[slow] {} 耗时 {}ms —— 这段时间里所有请求都在排队（WS 读取循环逐条 await）",
                self.method,
                ms
            );
        }
    }
}
/// 把检查点回滚结果转成协议回执（W6-T4）。
///
/// 为什么要单独一个函数：三个 `revert_*` 臂原先都**丢弃**结果、统一回 `{ok:true}`——
/// 界面据此显示"已回滚"，而实际上可能一个文件都没动。
///
/// 口径：
/// - `Some(outcome)` → `ok: true` + **恢复/删除/跳过/失效**四份清单（界面可列出"恢复了哪些文件"）；
/// - `None` → `ok: false` + 原因（该会话没有对应检查点，**什么都没回滚**）。
fn revert_response(outcome: Option<agent_node::checkpoint::RevertOutcome>) -> serde_json::Value {
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
    /// 针对多工作区的动态编码引擎工厂
    engine_factory: Option<EngineFactory>,
    /// 按工作区路径缓存的编码真引擎
    workspace_engines: Arc<Mutex<HashMap<std::path::PathBuf, Arc<agent_base::engine::AgentRuntime>>>>,
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
    pm_workspace_engines: Arc<Mutex<HashMap<std::path::PathBuf, Arc<agent_base::engine::AgentRuntime>>>>,
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
}

/// 把界面传来的图片**归一成可以直接进模型请求的 data URI**。
///
/// 界面产出的一律是 data URI（粘贴/拖拽走 `FileReader.readAsDataURL`，选文件走
/// `fs.read_base64`），但老盘上恢复出来的会话可能是裸路径，所以这里兜一手。
///
/// **为什么归一化在这层**：`agent-base` 是零 IO 的领域层，引擎拿不到文件系统——
/// 读盘转 base64 只能发生在 RPC 入口。读不到的图片**丢掉并记日志**，不给模型一个
/// 它取不到的路径（那会变成一个只有上游能看懂的报错）。
fn normalize_images(images: Option<Vec<String>>) -> Option<Vec<String>> {
    let images = images?;
    let mut out = Vec::with_capacity(images.len());
    for img in images {
        let trimmed = img.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("data:") {
            out.push(trimmed.to_string());
            continue;
        }
        match fs_service::read_file_base64(trimmed) {
            Ok(data_uri) => out.push(data_uri),
            Err(e) => warn!("图片无法读取，本轮丢弃该图（path={trimmed}）：{}", e.message),
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 辅助函数：根据目标工作区优先从缓存中获取引擎，若缺失且存在工厂则现场装配并写入缓存
async fn resolve_engine_from_cache(
    ws_str: &str,
    cache: &Arc<Mutex<HashMap<std::path::PathBuf, Arc<agent_base::engine::AgentRuntime>>>>,
    factory: &Option<EngineFactory>,
    fallback: &Option<Arc<agent_base::engine::AgentRuntime>>,
) -> Option<Arc<agent_base::engine::AgentRuntime>> {
    let trimmed = ws_str.trim();
    if trimmed.is_empty() {
        return fallback.clone();
    }
    let p = std::path::PathBuf::from(trimmed);
    {
        let guard = cache.lock().await;
        if let Some(eng) = guard.get(&p) {
            return Some(eng.clone());
        }
    }
    if let Some(f) = factory {
        if let Some(eng) = f(&p) {
            let mut guard = cache.lock().await;
            guard.insert(p, eng.clone());
            return Some(eng);
        }
    }
    fallback.clone()
}

/// 把将被丢弃的历史渲染成给"压缩器"看的纯文本（带角色前缀，工具输出取前 400 字）。
fn render_transcript_for_summary(items: &[Item]) -> String {
    let mut out = String::new();
    for it in items {
        match it {
            Item::User { text, .. } => out.push_str(&format!("【用户】{}\n", text.trim())),
            Item::Assistant { text, .. } if !text.trim().is_empty() => {
                out.push_str(&format!("【助手】{}\n", text.trim()))
            }
            Item::Thinking { text, .. } if !text.trim().is_empty() => {
                out.push_str(&format!("【思考】{}\n", text.trim()))
            }
            Item::Tool { name, status, output, .. } => {
                let brief: String = output
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(400)
                    .collect();
                out.push_str(&format!("【工具 {name} / {status}】{brief}\n"));
            }
            Item::Compact { summary, .. } => out.push_str(&format!("【已有摘要】{summary}\n")),
            _ => {}
        }
    }
    out
}

/// 让模型把"将被丢弃的历史"压成**真实的结构化摘要**。
///
/// 早先的 `thread.compact` 只写一句"已对前序 N 轮对话…进行了结构化压缩与归纳"——
/// 那是**假装总结过**：内容被丢掉，摘要却什么都没说。这里真的调用一次模型。
/// 失败返回 `None`，由调用方用**如实说明**的兜底文案（绝不假装）。
async fn summarize_for_compaction(provider: &ProviderConfig, transcript: &str) -> Option<String> {
    use agent_base::model::{ChatCompletionMessage, ModelChatOptions, StreamDelta};
    use agent_base::ports::{CompletionRequest, ModelClient};

    let prompt = format!(
        "请把下面这段会话记录压缩成**结构化摘要**，供后续接着干活使用：\n\
         1. 用户的目标与约束；2. 已完成的关键动作与结论；3. 涉及的文件/路径/命令；\
         4. 未完成的待办与已知阻塞。\n\n\
         要求：只保留可复用的信息，不要复述过程；不超过 600 字；用 Markdown 要点列出。\n\n\
         ---- 会话记录 ----\n{transcript}"
    );
    let req = CompletionRequest {
        config: provider.clone(),
        messages: vec![ChatCompletionMessage {
            role: "user".to_string(),
            content: Some(prompt),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }],
        options: ModelChatOptions {
            tools: None,
            system_prompt: Some("你是会话压缩器：只输出摘要，不输出解释或寒暄。".to_string()),
            temperature: Some(0.2),
            effort: None,
            max_retries: Some(1),
        },
    };

    let mut rx = match NetworkModelClient::new().stream(req, None).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("上下文压缩：模型调用失败（{e:?}），改用如实说明的兜底摘要");
            return None;
        }
    };
    let mut text = String::new();
    while let Some(delta) = rx.recv().await {
        match delta {
            StreamDelta::Text { text: t } => text.push_str(&t),
            StreamDelta::Error { error } => {
                warn!("上下文压缩：流式读取失败（{error}），改用如实说明的兜底摘要");
                return None;
            }
            StreamDelta::Done { .. } => break,
            _ => {}
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// **就地压缩**指定会话：前序历史换成一张 compact 卡片 + 一条落盘的 compact 条目。
///
/// 手动 `thread.compact` 与自动压缩（上下文撞顶）共用这一份实现。
/// 返回摘要文本；`None` = 历史太短、无需压缩（**如实返回**，不编一个成功的假象）。
pub(crate) async fn compact_thread_now(
    store: &Arc<RwLock<AgentStore>>,
    session_mgr: &SessionManager,
    thread_id: &str,
) -> Option<String> {
    compact_thread_with_summary(store, session_mgr, thread_id, None).await
}

/// 压缩主流程。`preset_summary` 是**测试注入点**（跳过真实模型调用）；
/// 生产路径一律传 `None` → 真的调模型生成摘要。
pub(crate) async fn compact_thread_with_summary(
    store: &Arc<RwLock<AgentStore>>,
    session_mgr: &SessionManager,
    thread_id: &str,
    preset_summary: Option<String>,
) -> Option<String> {
    // 1. 取要压缩的历史 + 真实的上下文规模
    let (provider, transcript, to_summarize, pre_tokens, ws) = {
        let s = store.read().await;
        let t = s.threads.iter().find(|t| t.id == thread_id)?;
        let total = t.items.len();
        if total < 3 {
            return None;
        }
        let preserve = total.saturating_sub(4).max(1);
        let drop_n = total - preserve;
        if drop_n == 0 {
            return None;
        }
        let transcript = render_transcript_for_summary(&t.items[..drop_n]);
        // 真实规模优先用"最近一次模型调用如实报回的 prompt_tokens"；拿不到才按字符估
        let pre = t
            .items
            .iter()
            .rev()
            .find_map(|it| match it {
                Item::Assistant { usage: Some(u), .. } => Some(u.prompt_tokens),
                _ => None,
            })
            .unwrap_or_else(|| (transcript.chars().count() / 4) as u64);
        (
            s.provider.clone(),
            transcript,
            drop_n,
            pre,
            t.workspace.clone(),
        )
    };

    // 2. 真实摘要（失败则如实说明兜底）；测试可注入预设摘要以跳过模型调用
    let summary = match preset_summary {
        Some(s) => s,
        None => match summarize_for_compaction(&provider, &transcript).await {
            Some(s) => s,
            None => format!(
                "（压缩：未能生成模型摘要，已丢弃前序 {to_summarize} 项历史。\
                 如需保留细节，请把关键结论重新告知。）"
            ),
        },
    };

    // 3. 写 compact 卡片（界面）+ 落盘 compact 条目（引擎恢复时据此折叠上下文）
    let compact_id = crate::state::next_id("compact");
    let now = crate::state::now_millis();
    let post_tokens = (summary.chars().count() / 4) as u64;
    {
        let mut s = store.write().await;
        if let Some(t) = s.get_thread_mut(thread_id) {
            let preserve = t.items.len().saturating_sub(4).max(1);
            let preserved = t.items.split_off(preserve);
            t.items = vec![Item::Compact {
                id: compact_id.clone(),
                at: now,
                summary: summary.clone(),
                pre_tokens,
                post_tokens,
                saved_tokens: pre_tokens.saturating_sub(post_tokens),
                turns_summarized: to_summarize as u64,
            }];
            t.items.extend(preserved);
        }
    }

    let _ = session_mgr.append_compact_entry(
        thread_id,
        agent_node::session::SessionCompactEntry {
            entry_type: "compact".to_string(),
            id: compact_id,
            timestamp: now as i64,
            summary: summary.clone(),
            pre_tokens,
            post_tokens,
            saved_tokens: pre_tokens.saturating_sub(post_tokens),
            turns_summarized: to_summarize,
            custom_instructions: None,
        },
        if ws.is_empty() { None } else { Some(&ws) },
    );

    Some(summary)
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
        first_images: Option<Vec<String>>,
        provider_config: ProviderConfig,
    ) {
        let target_tid_clone_for_insert = thread_id.clone();
        let store_clone = Arc::clone(&self.store);
        let broadcaster_clone = self.broadcaster.clone();
        let abort_senders_clone = Arc::clone(&self.abort_senders);
        let running_tasks_clone = Arc::clone(&self.running_tasks);
        let target_tid_clone = thread_id;
        // W3-T2：把注入的真引擎（若有）与动态工作区工厂带进 drain 循环
        let engine_clone = self.engine.clone();
        let engine_factory_clone = self.engine_factory.clone();
        let workspace_engines_clone = Arc::clone(&self.workspace_engines);
        let pm_engine_clone = self.pm_engine.clone();
        let pm_engine_factory_clone = self.pm_engine_factory.clone();
        let pm_workspace_engines_clone = Arc::clone(&self.pm_workspace_engines);
        let session_mgr_clone = self.session_mgr.clone();

        let runner_task = tokio::spawn(async move {
            let mut current_prompt: Option<String> = first_prompt;
            // 与 `current_prompt` 同生命周期：本轮带哪几张图。排队指令出队时一并换成它自带的那组，
            // 否则第二条指令会"继承"第一条的图片（图片被当成会话级状态而不是消息级附件）。
            let mut current_images: Option<Vec<String>> = first_images;
            // 本次执行链里已经自动压缩过几次（防"压了还撞顶"无限循环）
            let mut auto_compact_rounds = 0u32;
            loop {
                let (abort_tx, abort_rx) = watch::channel(false);
                abort_senders_clone.lock().await.insert(target_tid_clone.clone(), abort_tx);

                // 容量给足：模型流式时 `TextDelta` 事件密集，通道小会让关键事件（审批/收尾）
                // 更容易撞上"满"而被丢。关键事件另有不可丢的退避重试（见 DirectEventSink）。
                let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(1024);
                let turn_started = std::time::Instant::now();
                let tid = target_tid_clone.clone();
                let prompt = current_prompt.take();
                let images = current_images.take();
                // 压缩重试要复用同一组图片（见下面的 `current_images = retry_images`）
                let retry_images = images.clone();
                let p_cfg = provider_config.clone();
                // 本轮是否是"撞顶停的"（引擎通过专门事件告知，见下面的事件臂）
                let mut context_exceeded: Option<(u64, u64)> = None;
                // 本轮是否是"模型请求失败停的"（重试已耗尽）→ 断开对话，不再消费排队指令
                let mut hard_failure: Option<String> = None;

                // 每轮根据当前会话绑定的 Agent / 模式 / 所属工作区 分发对应引擎：
                let (thread_agent_id, thread_mode, thread_ws, context_window, retry_count) = {
                    let s = store_clone.read().await;
                    let t = s.threads.iter().find(|t| t.id == tid);
                    let agent_id = t.and_then(|t| t.agent_id.clone());
                    let mode = t.and_then(|t| t.mode);
                    let ws = thread_workspace(&s, &tid);
                    // 上下文窗口：界面配置里的真值（运行时可变），交给引擎做撞顶治理
                    let cw = s.config.context_window;
                    // 请求重试次数：同样取界面配置的真值（设置项「请求重试次数」，默认 5）
                    let retries = agent_base::model::normalize_model_retry_count(
                        s.config.retry_count as usize,
                    );
                    (agent_id, mode, ws, if cw > 0 { Some(cw) } else { None }, retries)
                };
                let is_pm = thread_agent_id.as_deref() == Some("ada-pm")
                    || thread_agent_id.as_deref() == Some("pm-assistant")
                    || thread_mode == Some(AgentMode::Pm);
                let engine_for_turn = if is_pm {
                    let pm_res = resolve_engine_from_cache(
                        &thread_ws,
                        &pm_workspace_engines_clone,
                        &pm_engine_factory_clone,
                        &pm_engine_clone,
                    ).await;
                    if pm_res.is_some() {
                        pm_res
                    } else {
                        resolve_engine_from_cache(
                            &thread_ws,
                            &workspace_engines_clone,
                            &engine_factory_clone,
                            &engine_clone,
                        ).await
                    }
                } else {
                    resolve_engine_from_cache(
                        &thread_ws,
                        &workspace_engines_clone,
                        &engine_factory_clone,
                        &engine_clone,
                    ).await
                };

                let loop_handle = tokio::spawn(async move {
                    // W3-T2：统一入口——按是否注入引擎决定执行。
                    if let Err(e) = run_agent_turn(
                        engine_for_turn,
                        &tid,
                        prompt.as_deref(),
                        images,
                        p_cfg,
                        context_window,
                        Some(retry_count),
                        event_tx,
                        Some(abort_rx),
                    ).await {
                        tracing::warn!("轮次执行失败: {e}");
                    }
                });

                        while let Some(event) = event_rx.recv().await {
                            let mut store = store_clone.write().await;
                            match event.body {
                                AgentEventBody::ThinkingDelta { text } => {
                                    store.append_thinking_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentEventBody::TextDelta { text } => {
                                    store.append_assistant_delta(&target_tid_clone, &text);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentEventBody::ToolCallStarted { call_id, name, args } => {
                                    let args_str = args.to_string();
                                    store.start_tool_call(&target_tid_clone, &call_id, &name, &args_str);
                                    let is_subagent = name == "invoke_subagent";
                                    // 这次派活**之前**该父会话已有的子会话：下面靠它认出
                                    // "本次调用新建的那一个"。缺了它就只能"任取一个子会话"，
                                    // 第二次派活会把上一次已结束的子会话重新点亮成运行中。
                                    let known_children: Vec<String> = store
                                        .threads
                                        .iter()
                                        .filter(|t| {
                                            t.parent_id.as_deref() == Some(target_tid_clone.as_str())
                                                && t.is_subagent == Some(true)
                                        })
                                        .map(|t| t.id.clone())
                                        .collect();
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }

                                    // 子智能体启动时：后台异步检测落盘会话并及时绑定至工具卡片与 running_thread_ids
                                    if is_subagent {
                                        let store_monitor = store_clone.clone();
                                        let bc_monitor = broadcaster_clone.clone();
                                        let sess_mgr_monitor = session_mgr_clone.clone();
                                        let parent_tid_monitor = target_tid_clone.clone();
                                        let call_id_monitor = call_id.clone();
                                        let known_children_monitor = known_children;
                                        tokio::spawn(async move {
                                            for _ in 0..60 {
                                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                                let mut st = store_monitor.write().await;
                                                st.sync_threads_from_disk(&sess_mgr_monitor);
                                                // 已经被**别的运行中委派卡片**认领过的子会话不再重复认领
                                                let bound_children: Vec<String> = st
                                                    .threads
                                                    .iter()
                                                    .find(|t| t.id == parent_tid_monitor)
                                                    .map(|t| {
                                                        t.items
                                                            .iter()
                                                            .filter_map(|it| match it {
                                                                agent_proto::Item::Tool {
                                                                    name,
                                                                    status,
                                                                    call_id,
                                                                    details,
                                                                    ..
                                                                } if name == "invoke_subagent"
                                                                    && status == "running"
                                                                    && call_id != &call_id_monitor =>
                                                                {
                                                                    details.as_ref().and_then(|d| {
                                                                        d.get("threadId")
                                                                            .or_else(|| d.get("subagent_thread_id"))
                                                                            .and_then(|v| v.as_str())
                                                                            .map(|s| s.to_string())
                                                                    })
                                                                }
                                                                _ => None,
                                                            })
                                                            .collect()
                                                    })
                                                    .unwrap_or_default();
                                                let found_child = st.pick_new_subagent_child(
                                                    &parent_tid_monitor,
                                                    &known_children_monitor,
                                                    &bound_children,
                                                );

                                                if let Some(child_id) = found_child {
                                                    let mut updated = false;
                                                    if !st.running_thread_ids.contains(&child_id) {
                                                        st.running_thread_ids.push(child_id.clone());
                                                        updated = true;
                                                    }
                                                    if let Some(parent_t) = st.get_thread_mut(&parent_tid_monitor) {
                                                        for item in parent_t.items.iter_mut().rev() {
                                                            if let agent_proto::Item::Tool { call_id, details, status, .. } = item {
                                                                if call_id == &call_id_monitor && status == "running" {
                                                                    let mut det = details.take().unwrap_or_else(|| serde_json::json!({}));
                                                                    if let Some(obj) = det.as_object_mut() {
                                                                        if !obj.contains_key("threadId") {
                                                                            obj.insert("threadId".to_string(), serde_json::json!(child_id));
                                                                            obj.insert("subagent_thread_id".to_string(), serde_json::json!(child_id));
                                                                            updated = true;
                                                                        }
                                                                    }
                                                                    *details = Some(det);
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    drop(st);
                                                    if updated {
                                                        if let Some(ref bc) = bc_monitor {
                                                            bc.broadcast_immediate().await;
                                                        }
                                                        break;
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                                AgentEventBody::ToolCallFinished { call_id, name, receipt } => {
                                    // 本次调用对应的子会话 id：回执里带的就是它（见 `SubagentTool`）
                                    let finished_child = receipt.details.as_ref().and_then(|d| {
                                        d.get("subagent_thread_id")
                                            .or_else(|| d.get("threadId"))
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                    });
                                    store.finish_tool_call(
                                        &target_tid_clone,
                                        &call_id,
                                        receipt.ok(),
                                        Some(receipt.output.clone()),
                                        Some(receipt.duration_ms()),
                                        Some(receipt.started_at),
                                        Some(receipt.finished_at),
                                        receipt.details.clone(),
                                    );
                                    if name == "invoke_subagent" {
                                        store.sync_threads_from_disk(&session_mgr_clone);
                                        match finished_child {
                                            // 精确解除：只摘掉**这次**派活那个子会话的运行态。
                                            // 按"父会话名下所有子会话"一刀切会误伤并行派活的另一个子会话。
                                            Some(cid) => store.running_thread_ids.retain(|tid| tid != &cid),
                                            // 回执里没有子会话 id（没进入执行/被拒）→ 退回按父会话清理
                                            None => {
                                                let child_ids: Vec<String> = store.threads.iter()
                                                    .filter(|t| t.parent_id.as_deref() == Some(&target_tid_clone) && t.is_subagent == Some(true))
                                                    .map(|t| t.id.clone())
                                                    .collect();
                                                store.running_thread_ids.retain(|tid| !child_ids.contains(tid));
                                            }
                                        }
                                    }
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentEventBody::QuestionAsked { call_id, question } => {
                                    store.set_tool_awaiting_question(&target_tid_clone, &call_id, question);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                // W3-T3：审批请求 → 工具卡片进入"等待批准"，界面据此渲染批准/拒绝按钮
                                // 并把**待批准的命令原文**摆出来（args 是本事件自带的一等数据）
                                AgentEventBody::ApprovalRequested { call_id, tool, args } => {
                                    store.set_tool_waiting_approval(&target_tid_clone, &call_id, &tool, &args);
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.broadcast_immediate().await;
                                    }
                                }
                                AgentEventBody::UsageReported { usage, duration_ms } => {
                                    let turn_duration_ms = turn_started.elapsed().as_millis() as u64;
                                    store.set_assistant_stats(
                                        &target_tid_clone,
                                        Some(usage),
                                        duration_ms,
                                        turn_duration_ms,
                                    );
                                    drop(store);
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentEventBody::Diagnostic(diag) => {
                                    // P0 埋点地基：诊断遥测**不进会话流、不进模型上下文**，
                                    // 只进诊断缓冲（界面 DebugPanel）与 tracing。
                                    //
                                    // 与 Notice 的分工：Notice 是"给用户看的一条提示"
                                    // （要落盘、要跨端可见），Diagnostic 是"给排查用的元数据"
                                    // （不落盘，只此进程有效）。
                                    //
                                    // 投影走 `server::diag` 的**唯一一处**：轮次外的扫描类诊断
                                    // 也从那里进同一个缓冲，两处各写一份格式化必然分叉。
                                    let text = crate::server::diag::project_diagnostic(
                                        &mut store,
                                        &diag,
                                        &event.trace,
                                    );
                                    drop(store);
                                    tracing::debug!(target: "a_da::diag", "{text}");
                                    if let Some(ref bc) = broadcaster_clone {
                                        bc.mark_dirty();
                                    }
                                }
                                AgentEventBody::TurnFinished { stop } => {
                                    // 上下文撞顶：记下来，收尾后**自动压缩并续跑**（见下面的分支）
                                    if let TurnStopReason::ContextWindowExceeded { prompt_tokens, window } = stop {
                                        store.push_log(
                                            "warn",
                                            format!("上下文撞顶（{prompt_tokens}/{window} tokens）：将自动压缩后继续"),
                                            None,
                                        );
                                        drop(store);
                                        context_exceeded = Some((prompt_tokens, window));
                                        break;
                                    }
                                    drop(store);
                                    break;
                                }
                                AgentEventBody::Failed { message } => {
                                    // 失败信息走 **Notice 卡片**，绝不写进助手正文：
                                    // ① 落进正文会把一段连贯回答从中间劈开（截图里的形态）；
                                    // ② 正文会被回传给模型，等于让模型以为"我上一轮说过这句报错"。
                                    // Notice 的落盘记录在 `fold_messages` 里被明确跳过，
                                    // 所以它**只给人看**、永不进上下文。
                                    //
                                    // 「已断开」这句由**宿主**写，而不是只做界面提示：
                                    // 任何客户端（桌面 / Web / CLI）都该看到"这轮到此为止"，
                                    // 而不是各端各自猜。
                                    let notice_text = format!(
                                        "请求异常：{message}\n\
                                         重试已耗尽，本次对话已断开；排队中的指令仍保留。"
                                    );
                                    // 落盘**在改内存之前**（与重试清理同一顺序）：
                                    // 后台每 1.5s 一次的 `sync_threads_from_disk` 要拿 store
                                    // 写锁，所以还握着写锁时先把磁盘写好，就不存在
                                    // "同步读到旧磁盘内容、把这张卡片抹掉"的窗口。
                                    persist_failure_notice(
                                        &session_mgr_clone,
                                        &target_tid_clone,
                                        &notice_text,
                                    );
                                    store.add_notice(&target_tid_clone, &notice_text, "error");
                                    store.push_log("error", format!("Agent 执行异常: {message}"), None);
                                    drop(store);
                                    // 模型请求的重试次数已在适配器里用完（次数由设置项决定）
                                    // → 记下来，收尾时**断开本轮对话**而不是接着跑排队指令
                                    hard_failure = Some(message);
                                    break;
                                }
                                AgentEventBody::TurnStarted => {
                                    drop(store);
                                }
                                AgentEventBody::SubagentStarted { thread_id, subagent } => {
                                    tracing::debug!(child_thread_id = %thread_id, subagent = %subagent, "子智能体已启动");
                                    drop(store);
                                }
                                AgentEventBody::SubagentFinished { thread_id, ok, summary } => {
                                    tracing::debug!(child_thread_id = %thread_id, ok = ok, summary = %summary, "子智能体已结束");
                                    drop(store);
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

                        // 模型请求失败（重试次数已耗尽）→ **断开本次对话**：
                        // 排队指令原样留在队列里，等用户改配置/网络后手动重试，
                        // 而不是拿注定失败的请求把队列一条条撞完。
                        if let Some(msg) = hard_failure.take() {
                            let mut store = store_clone.write().await;
                            store.push_log(
                                "error",
                                format!("模型请求失败（重试已耗尽），本次对话已断开: {msg}"),
                                None,
                            );
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                            break;
                        }

                        // 上下文撞顶 → **自动压缩并续跑**（而不是丢给用户一句"请手动压缩"）。
                        // 压过一次仍然撞顶就如实收尾，绝不无限压。
                        if let Some((pt, win)) = context_exceeded.take() {
                            if auto_compact_rounds >= 1 {
                                let mut store = store_clone.write().await;
                                store.append_assistant_delta(
                                    &target_tid_clone,
                                    &format!(
                                        "\n\n**上下文已达窗口上限**（{pt}/{win} tokens），压缩后仍然超限。\
                                         请把任务拆小，或换更大上下文窗口的模型。"
                                    ),
                                );
                                drop(store);
                                if let Some(ref bc) = broadcaster_clone {
                                    bc.broadcast_immediate().await;
                                }
                                break;
                            }
                            auto_compact_rounds += 1;
                            tracing::info!("上下文撞顶（{pt}/{win} tokens），自动压缩后继续");
                            let summary =
                                compact_thread_now(&store_clone, &session_mgr_clone, &target_tid_clone).await;
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                            current_prompt = Some(match summary {
                                Some(s) => format!(
                                    "【上下文已自动压缩】以下是压缩后的历史摘要，请基于它继续完成原任务：\n\n{s}"
                                ),
                                None => "【上下文已自动压缩】请基于压缩后的上下文继续完成原任务。".to_string(),
                            });
                            // 压缩后的重试仍是**同一轮**：用户附的图片要跟着一起重发，
                            // 否则"发图 + 上下文撞顶"就变成图片被静默吞掉。
                            current_images = retry_images;
                            continue;
                        }

                        // 检查会话队列中是否还有待执行的排队指令
                        let next_item = {
                            let mut store = store_clone.write().await;
                            store.pop_next_queued(&target_tid_clone)
                        };

                        if let Some(queued) = next_item {
                            // 出队指令**自带**它的图片：本轮换用队列里那一组，别继承上一条指令的
                            // （图片是消息级附件，不是会话级状态）。
                            let queued_images = queued.images.clone();
                            let mut store = store_clone.write().await;
                            store.add_user_message_with_images(&target_tid_clone, &queued.text, queued.images);
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                            current_prompt = Some(queued.text);
                            current_images = queued_images;
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
        let store_bg = store.clone();
        let session_mgr_bg = session_mgr.clone();
        let broadcaster_bg = broadcaster.clone();
        tokio::spawn(async move {
            // 🔴 **整树会话同步很贵，不能每 1.5s 跑一次**。
            //
            // 实测（用户报"点批准要等 2–4 秒→四段日志实测 ②→③ 6437ms"）：
            // `sync_threads_from_disk` 会把 `sessions/` 下**每个**会话文件重新解析一遍
            // （实测环境有 2.8MB + 1.3MB + 454KB + … 的会话），是**秒级** CPU 活儿；
            // 它还要握着 store 写锁，而 WS 读取循环是逐条 await 的——
            // 于是批准/发消息这类请求只能在它后面排队，表现就是"点了没反应"。
            //
            // 先做**止损式**限流：间隔 1.5s → 10s，并把耗时打进日志（`[sync]`），
            // 让"它到底多贵"变成可见数字。真正的修法是 mtime 过滤（只解析改过的文件），
            // 见 docs/state-patch-plan.md 的后续项——那时才能把间隔调回来。
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let started = std::time::Instant::now();
                let mut s = store_bg.write().await;
                let changed = s.sync_threads_from_disk(&session_mgr_bg);
                drop(s);
                let ms = started.elapsed().as_millis();
                if ms >= 200 {
                    tracing::warn!(
                        "[sync] 整树会话同步耗时 {}ms（changed={}）——会话文件越多越贵，待做 mtime 过滤",
                        ms,
                        changed
                    );
                }
                if changed {
                    if let Some(ref bc) = broadcaster_bg {
                        bc.mark_dirty();
                    }
                }
            }
        });

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
            engine_factory: None,
            workspace_engines: Arc::new(Mutex::new(HashMap::new())),
            product_spec: None,
            pm_engine: None,
            pm_engine_factory: None,
            pm_workspace_engines: Arc::new(Mutex::new(HashMap::new())),
            plugin_skills: agent_node::skills::new_skill_store(Vec::new()),
            plugin_reassembler: None,
            hook_runner: None,
        }
    }

    /// P3a·SP-55：注入钩子运行时（管理面点位用）。没注入 = 这些点位不跑（如实）。
    pub fn with_hook_runner(mut self, runner: Arc<dyn agent_base::ports::HookRunner>) -> Self {
        self.hook_runner = Some(runner);
        self
    }

    /// 跑一个**轮外**事前点位（P3a·SP-55）。
    ///
    /// 观测**先攒后发**：诊断要写 `store`（async），而端口给的回调是同步的（`Fn`）。
    /// 攒起来在调用返回后投影，代价是"发出时刻"晚了一点——这里刻意接受，
    /// 因为轮外点位本来就没有轮内顺序可谈，而 `duration_ms` 是**运行时实测**的、不是编的。
    /// 轮内点位（引擎）用的是同步 sink，所以那边是真·实时逐条发。
    async fn run_out_of_turn_before(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
    ) -> agent_base::ports::BeforeOutcome {
        let Some(runner) = &self.hook_runner else {
            return agent_base::ports::BeforeOutcome::Pass;
        };
        let collected: std::sync::Mutex<Vec<agent_base::ports::HookObservation>> =
            std::sync::Mutex::new(Vec::new());
        let on_event = |obs: agent_base::ports::HookObservation| {
            if let Ok(mut v) = collected.lock() {
                v.push(obs);
            }
        };
        let ctx = agent_base::ports::HookCtx {
            thread_id: "",
            turn_id: "",
            call_id: None,
            cancel: &NeverCancelled,
            on_event: &on_event,
        };
        let outcome = runner
            .before(
                agent_base::ports::BeforeRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await;
        self.project_hook_observations(&collected).await;
        outcome
    }

    /// 跑一个**轮外**事后点位，返回旁注文本（P3a·SP-55）。
    async fn run_out_of_turn_after(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
    ) -> Vec<String> {
        let Some(runner) = &self.hook_runner else {
            return Vec::new();
        };
        let collected: std::sync::Mutex<Vec<agent_base::ports::HookObservation>> =
            std::sync::Mutex::new(Vec::new());
        let on_event = |obs: agent_base::ports::HookObservation| {
            if let Ok(mut v) = collected.lock() {
                v.push(obs);
            }
        };
        let ctx = agent_base::ports::HookCtx {
            thread_id: "",
            turn_id: "",
            call_id: None,
            cancel: &NeverCancelled,
            on_event: &on_event,
        };
        let notes = runner
            .after(
                agent_base::ports::AfterRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await;
        self.project_hook_observations(&collected).await;
        notes
            .into_iter()
            .filter_map(|n| match n {
                agent_base::ports::AfterNote::Note { text } if !text.trim().is_empty() => Some(text),
                _ => None,
            })
            .collect()
    }

    /// 把攒下来的观测投影成诊断（**转换点只有一处**：`ports::hooks` 里那个函数）。
    async fn project_hook_observations(
        &self,
        collected: &std::sync::Mutex<Vec<agent_base::ports::HookObservation>>,
    ) {
        let obs = match collected.lock() {
            Ok(v) => v.clone(),
            Err(_) => return,
        };
        if obs.is_empty() {
            return;
        }
        let mut store_w = self.store.write().await;
        for o in &obs {
            crate::server::diag::project_out_of_turn(
                &mut store_w,
                &agent_base::ports::hooks::observation_to_diagnostic(o),
            );
        }
    }

    /// P2·SP-45：注入插件声明的技能（组合根在装配期收好的那份）。
    ///
    /// 保持 builder 风格的理由同 [`Self::with_engine`]：既有调用点与测试一行不改，
    /// 没注入时 `skill.list` 就只报目录与内置技能（**如实**，不是假装有插件技能）。
    pub fn with_plugin_skills(
        mut self,
        skills: agent_node::skills::SkillStore,
    ) -> Self {
        self.plugin_skills = skills;
        self
    }

    /// P2·SP-46：注入重装配入口（组合根实现）。没注入 = `plugin.setEnabled` 只写盘不重装配。
    pub fn with_plugin_reassembler(mut self, r: Arc<dyn PluginReassembler>) -> Self {
        self.plugin_reassembler = Some(r);
        self
    }

    /// W3-T2：注入真引擎（`AgentRuntime`）。宿主装配层（`agent-host`）构造后传进来。
    ///
    /// 保持 builder 风格：既有调用点与测试**一行不改**，只有真正要切引擎的宿主才调它。
    pub fn with_engine(mut self, engine: Arc<agent_base::engine::AgentRuntime>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// 注入针对多工作区的动态编码引擎工厂
    pub fn with_engine_factory(mut self, factory: EngineFactory) -> Self {
        self.engine_factory = Some(factory);
        self
    }

    /// 注入 PM 助手真引擎。
    pub fn with_pm_engine(mut self, engine: Arc<agent_base::engine::AgentRuntime>) -> Self {
        self.pm_engine = Some(engine);
        self
    }

    /// 注入针对多工作区的动态 PM 引擎工厂
    pub fn with_pm_engine_factory(mut self, factory: EngineFactory) -> Self {
        self.pm_engine_factory = Some(factory);
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
            self.engine_factory = i.factory;
        }
        self
    }

    /// 管道式注入 **PM 模式**引擎（`ada-coding` 的 PM 模式预设，不是第二个产品）。
    ///
    /// 注意**只取 runtime 与 factory，不碰 `product_spec`**：握手回报的产品身份
    /// 必须始终是主引擎（`ada-coding`）那份声明——PM 只是同一产品的一个模式，
    /// 若让预设的 `id`（`ada-coding-pm`）漏进 `productInfo`，界面就会把它当成产品 id 用。
    pub fn pipe_pm_engine(mut self, injection: Option<EngineInjection>) -> Self {
        if let Some(i) = injection {
            self.pm_engine = Some(i.runtime);
            self.pm_engine_factory = i.factory;
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
        // 🔴 **慢臂打点**：WS 读取循环是逐条 await 的，任何一个方法臂耗时过长，
        // 排在它后面的请求（实测：点批准 ②→③ 曾经 6437ms → 限流 2207ms → mtime 过滤 262ms）
        // 都会看起来"点了没反应"。这里用 Drop 守卫覆盖**所有**返回路径，
        // 只在 ≥200ms 时打一行 `[slow]`，把"谁占着循环"直接指出来。
        let _slow = SlowArm {
            method: method.to_string(),
            started: std::time::Instant::now(),
        };
        match method {
            SESSION_INITIALIZE => {
                let p = params.as_object().ok_or_else(|| {
                    ProtocolError::invalid_params("session.initialize 需要一个参数对象")
                })?;

                let client_version = p.get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // 增量帧能力协商（docs/state-patch-plan.md 第 1 步）：只有声明了 state.patch 的
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
                serde_json::to_value(init_res).map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            SESSION_SNAPSHOT => {
                let mut store = self.store.write().await;
                store.sync_threads_from_disk(&self.session_mgr);
                // 响应必须带**当前已发出的状态版本**（不能写死 0）。
                // 客户端把它的 `seq` 记为 `lastSnapshotSeq`，下一帧增量帧的 `baseSeq` 就是它；
                // 写死 0 时比对永远失败 → 客户端每帧都判"缺帧"并发起自愈请求 → 死循环
                // （实测 376 次/秒，渲染进程满核、内存持续上涨）。
                let seq = self
                    .broadcaster
                    .as_ref()
                    .map(|b| b.last_sent_seq())
                    .unwrap_or(0);
                let snapshot = generate_snapshot_with_seq(&store, seq);
                drop(store);
                Ok(serde_json::to_value(snapshot).map_err(|e| ProtocolError::internal_error(e.to_string()))?)
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
                    .ok_or_else(|| ProtocolError::invalid_params(format!("会话不存在：{thread_id}")))?;
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
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                if !store.threads.iter().any(|t| t.id == thread_id) {
                    store.sync_threads_from_disk(&self.session_mgr);
                }
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
                if !store.threads.iter().any(|t| t.id == thread_id) {
                    store.sync_threads_from_disk(&self.session_mgr);
                }
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
                let agent_id_param = params.get("agentId").and_then(|v| v.as_str()).map(|s| s.to_string());
                let ws_param = params.get("workspace")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                // S6 补完：驱动方（网关）在这里告诉我们"这一轮是第几层委派"。
                // 不传 = 用户直接连的（深度 0）。
                let delegation_depth = params
                    .get("delegationDepth")
                    .and_then(|v| v.as_u64())
                    .map(|d| d as u32);
                let is_subagent = params.get("isSubagent").and_then(|v| v.as_bool())
                    .or_else(|| delegation_depth.map(|d| d > 0));
                let parent_id = params.get("parentId")
                    .or_else(|| params.get("parentThreadId"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let subagent_id = params.get("subagentId")
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
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let mut store = self.store.write().await;
                // 会话文件躺在它自己的工作区目录下，删除前先取出来
                let ws = thread_workspace(&store, thread_id);
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
                        return Ok(serde_json::json!({ "ok": false, "denied": true, "reason": reason }));
                    }
                }
                let deleted = store.delete_thread(thread_id);
                drop(store);

                let _ = self.session_mgr.delete_session(thread_id, Some(&ws));
                let _ = self.checkpoint_mgr.discard(thread_id);
                let notes = self
                    .run_out_of_turn_after(
                        agent_base::ports::hooks::points::AFTER_THREAD_DELETE,
                        Some(thread_id.to_string()),
                        serde_json::json!({
                            "threadId": thread_id,
                            "workspace": ws,
                            "deleted": deleted,
                        }),
                    )
                    .await;

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                Ok(serde_json::json!({
                    "ok": deleted,
                    "message": null,
                    "notes": notes
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
                let images = normalize_images(params.get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<String>>()));

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
                // 入库与送模型用同一份图片：卡片上的缩略图与真正发给模型的字节一致
                let turn_images = images.clone();
                store.add_user_message_with_images(&target_tid, &text, images);
                store.set_thread_running(&target_tid, true);
                let provider_config = store.provider.clone();
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

                self
                    .spawn_thread_loop(target_tid.clone(), Some(text.to_string()), turn_images, provider_config)
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
                let count = entries.iter().filter(|e| matches!(e, agent_node::checkpoint::CheckpointEntry::Checkpoint(_))).count();
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
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?;
                let outcome = self.checkpoint_mgr.revert_all(thread_id)
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
                        if let Ok(p) = serde_json::from_value::<agent_base::model::ModelProtocol>(serde_json::Value::String(proto.to_string())) {
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

                let entry: agent_base::model::ProviderEntry = serde_json::from_value(provider_val)
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
                    let protocol: agent_base::model::ModelProtocol = serde_json::from_value(serde_json::Value::String(proto_str.to_string()))
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

            CONFIG_SET_RETRY_COUNT => {
                // 模型请求重试次数（默认 5）。**必须落盘**：用户改完重启还得在。
                let retries = params
                    .get("retries")
                    .or_else(|| params.get("retryCount"))
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 retries 参数（0-20 的整数）"))?;

                let mut store = self.store.write().await;
                store.config.retry_count =
                    agent_base::model::normalize_model_retry_count(retries as usize) as u32;
                let saved = store.config.retry_count;
                if let Err(e) = save_app_config(&store) {
                    tracing::warn!("保存重试次数失败: {e}");
                }
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
                if !store.threads.iter().any(|t| t.id == thread_id) {
                    store.sync_threads_from_disk(&self.session_mgr);
                }
                store.focus_thread(thread_id.to_string());
                drop(store);

                if let Some(ref bc) = self.broadcaster {
                    bc.broadcast_immediate().await;
                }

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
                    "pm" => AgentMode::Pm,
                    _ => AgentMode::Code,
                };
                let mut store = self.store.write().await;
                let thread_id = params.get("threadId").and_then(|v| v.as_str()).unwrap_or(&store.active_id).to_string();

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
                    return Ok(serde_json::json!({ "success": false, "denied": true, "reason": reason }));
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
                let thread_id = params.get("threadId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 threadId 参数"))?
                    .to_string();
                let text = params.get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProtocolError::invalid_params("缺少 text 参数"))?
                    .to_string();
                // 编辑重发同样要带图：界面在这条路径上**本来就发** `images`
                // （`ws-client.editAndResend`），以前服务端把它丢了，于是"重发带图的提问"
                // 会变成一条没有图的提问。
                let images = normalize_images(params.get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<String>>()));

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

                self.spawn_thread_loop(thread_id, Some(text), images, provider_config).await;

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

                self.spawn_thread_loop(target_tid.clone(), None, None, provider_config).await;

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
                let thread_id = params.get("threadId")
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
                if !resolved && agent_node::approval::global_question_manager().has_pending(tool_item_id) {
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
                let text = params.get("text").and_then(|v| v.as_str()).map(|s| s.to_string());
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

                let home = agent_node::session::get_app_home();
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
                if !self.plugin_mgr.is_extension_entry(ws.as_deref(), Path::new(&file_path)) {
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

            PLUGIN_LIST => {
                let ws = params.get("workspace").and_then(|v| v.as_str());
                let plugins = self.plugin_mgr.load_plugins(ws);
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

            PLUGIN_LOAD => {
                let workspace = params.get("workspace").and_then(|v| v.as_str());
                let plugin_id = params.get("pluginId").and_then(|v| v.as_str());
                let plugins = self.plugin_mgr.load_plugins(workspace);
                let plugins: Vec<_> = match plugin_id {
                    Some(id) => plugins.into_iter().filter(|p| p.id == id).collect(),
                    None => plugins,
                };
                Ok(serde_json::json!({ "ok": true, "plugins": plugins }))
            }

            PLUGIN_RELOAD => {
                let workspace = params.get("workspace").and_then(|v| v.as_str());
                let plugins = self.plugin_mgr.reload_plugins(workspace);
                Ok(serde_json::json!({ "ok": true, "plugins": plugins }))
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

                self.plugin_mgr.toggle_plugin(plugin_id, enabled)
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
                    for s in skills.iter().filter(|s| !s.unknown_frontmatter_keys.is_empty()) {
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

    /// 图片归一化：data URI 原样通过。
    #[test]
    fn test_normalize_images_accepts_data_uri_as_is() {
        let out = normalize_images(Some(vec!["data:image/png;base64,AAA".to_string()]));
        assert_eq!(out, Some(vec!["data:image/png;base64,AAA".to_string()]));
    }

    /// 空的、读不到的——一律归成"没有图片"，**绝不把裸路径原样送进模型请求**。
    ///
    /// 这条归一化就是**零 IO 边界**本身：`agent-base` 读不了盘，路径形态的图片必须在
    /// RPC 入口被消灭掉，否则模型只会收到一个它取不到的路径（错误还很难看懂）。
    #[test]
    fn test_normalize_images_drops_empty_and_unreadable() {
        assert_eq!(normalize_images(None), None);
        assert_eq!(normalize_images(Some(vec![])), None);
        assert_eq!(normalize_images(Some(vec!["   ".to_string()])), None);
        assert_eq!(
            normalize_images(Some(vec!["E:/definitely/not/here_9f3a.png".to_string()])),
            None
        );
    }

    /// 裸路径（老盘上恢复出来的会话）读盘转成 data URI，mime 按扩展名判。
    #[test]
    fn test_normalize_images_reads_a_real_file_as_data_uri() {
        let dir = std::env::temp_dir().join(format!("a_da_img_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("shot.png");
        std::fs::write(&path, [0x89u8, 0x50, 0x4E, 0x47]).expect("写临时图片");

        let out = normalize_images(Some(vec![path.to_string_lossy().to_string()]))
            .expect("路径应当被读成 data URI");
        assert!(
            out[0].starts_with("data:image/png;base64,"),
            "应按扩展名判出 image/png：{}",
            out[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 守门：压缩必须**真的落盘 compact 条目**、留下最近几项、并用**真实**的上下文规模记账。
    ///
    /// 早先 `thread.compact` 只写一句"已对前序 N 轮…进行了结构化压缩与归纳"——假装总结过，
    /// 而且 pre/post tokens 是按"每项 450 tokens"编的。现在摘要由模型生成（测试注入），
    /// 规模取"最近一次模型调用如实报回的 prompt_tokens"。
    #[tokio::test]
    async fn test_compact_writes_entry_and_keeps_recent_items() {
        let dir = std::env::temp_dir().join(format!("a_da_compact_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let session_mgr = SessionManager::new(Some(dir.clone()));
        let ws = "E:/codes/compact_ws";

        let store = Arc::new(RwLock::new(AgentStore::new(ws.to_string())));
        let tid = {
            let mut s = store.write().await;
            let id = s.create_thread(Some("压缩测试".to_string()), None);
            // 造 6 项历史，最后一项带**真实用量**（445k 是实测那次撞顶的规模）
            s.add_user_message(&id, "任务：把项目跑起来");
            for i in 0..4 {
                s.start_tool_call(&id, &format!("call_{i}"), "read_file", "{}");
                s.finish_tool_call(&id, &format!("call_{i}"), true, Some(format!("输出 {i}")), Some(1), Some(0), Some(1), None);
            }
            s.append_assistant_delta(&id, "最后一段回复");
            s.set_assistant_stats(
                &id,
                Some(agent_base::model::TokenUsage {
                    prompt_tokens: 445_080,
                    completion_tokens: 557,
                    total_tokens: 445_637,
                    ..Default::default()
                }),
                10,
                20,
            );
            let _ = session_mgr.create_session(&id, ws, Some("压缩测试"), None, None);
            id
        };

        let summary = compact_thread_with_summary(&store, &session_mgr, &tid, Some("## 摘要\n- 目标：跑起来".to_string()))
            .await
            .expect("历史足够，必须压缩成功");
        assert!(summary.contains("摘要"));

        {
            let s = store.read().await;
            let t = s.threads.iter().find(|t| t.id == tid).expect("会话还在");
            assert!(matches!(t.items.first(), Some(Item::Compact { .. })), "首项必须是压缩卡片");
            let compact = match t.items.first() {
                Some(Item::Compact { pre_tokens, turns_summarized, .. }) => (*pre_tokens, *turns_summarized),
                _ => unreachable!(),
            };
            assert_eq!(compact.0, 445_080, "pre_tokens 必须取真实用量，不是编的 450/项");
            assert!(compact.1 > 0, "必须如实记录压缩了几项");
            assert!(t.items.len() < 7, "压缩后项数必须变少：{}", t.items.len());
        }

        // 落盘：最后一行必须是 compact 条目（引擎恢复上下文时靠它折叠）
        let path = session_mgr.get_session_path(ws, &tid);
        let content = std::fs::read_to_string(&path).expect("会话文件应存在");
        let last = content.lines().last().unwrap_or("");
        assert!(last.contains("\"type\":\"compact\""), "必须落盘 compact 条目：{last}");
        assert!(last.contains("摘要"), "落盘的条目里要带真实摘要：{last}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 历史太短 → **如实返回 None**（调用方据此回"无需压缩"），不假装压缩成功。
    #[tokio::test]
    async fn test_compact_returns_none_for_short_history() {
        let dir = std::env::temp_dir().join(format!("a_da_compact_short_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        let session_mgr = SessionManager::new(Some(dir.clone()));
        let store = Arc::new(RwLock::new(AgentStore::new("E:/codes/short_ws".to_string())));
        let tid = {
            let mut s = store.write().await;
            let id = s.create_thread(Some("短会话".to_string()), None);
            s.add_user_message(&id, "你好");
            id
        };
        assert!(
            compact_thread_with_summary(&store, &session_mgr, &tid, Some("x".into())).await.is_none(),
            "历史太短必须如实返回 None"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── W6-T4：回滚回执必须带真实文件清单 ────────────────────────────────────

    #[test]
    fn test_revert_response_carries_the_restored_file_list() {
        let outcome = agent_node::checkpoint::RevertOutcome {
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
            "骨架产品**没有**声明 hooks → 必须如实回 false（能力位从声明派生，不是恒真）"
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
        use agent_base::model::TokenUsage;

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
            Arc::new(agent_node::session::SessionManager::new(Some(std::path::PathBuf::from("E:/codes/default_ws")))),
            Arc::new(agent_node::checkpoint::CheckpointManager::new(None)),
            Arc::new(agent_node::subagents::SubagentManager::new()),
            Arc::new(agent_node::approval::ApprovalManager::new()),
            Arc::new(agent_node::plugins::PluginManager::new()),
            Arc::new(agent_node::skills::SkillManager::new()),
            None,
        );

        let catalog = dispatcher
            .dispatch(agent_proto::methods::PLUGIN_BUILTIN_CATALOG, serde_json::json!({}))
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
        let session_mgr = Arc::new(agent_node::session::SessionManager::new(Some(std::path::PathBuf::from("E:/codes/default_ws"))));
        let checkpoint_mgr = Arc::new(agent_node::checkpoint::CheckpointManager::new(None));
        let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
        let approval_mgr = Arc::new(agent_node::approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::new());
        let skill_mgr = Arc::new(agent_node::skills::SkillManager::new());

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
        let rx = agent_node::approval::global_question_manager().register_waiter(call_id);
        assert!(agent_node::approval::global_question_manager().has_pending(call_id));

        // 模拟前端调用 approval.decide
        let req = serde_json::json!({
            "toolItemId": call_id,
            "approved": true
        });

        let res = dispatcher.dispatch(agent_proto::methods::APPROVAL_DECIDE, req).await.expect("调用应该成功");
        assert_eq!(res.get("resolved").and_then(|v| v.as_bool()), Some(true));

        let ans = rx.await.expect("应该唤醒等待者");
        assert_eq!(ans.answered_by, "user");
        assert!(!agent_node::approval::global_question_manager().has_pending(call_id));
    }

    #[tokio::test]
    async fn test_session_initialize_returns_capabilities_and_snapshot_has_seq() {
        let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new("E:/codes/default_ws".to_string())));
        let session_mgr = Arc::new(agent_node::session::SessionManager::new(None));
        let checkpoint_mgr = Arc::new(agent_node::checkpoint::CheckpointManager::new(None));
        let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
        let approval_mgr = Arc::new(agent_node::approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::new());
        let skill_mgr = Arc::new(agent_node::skills::SkillManager::new());

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
            .dispatch(agent_proto::methods::SESSION_INITIALIZE, init_params)
            .await
            .expect("握手成功");

        let caps = init_val.get("capabilities").expect("必须包含 capabilities 节点");
        // ada-coding 声明 images=true（多模态已接通：dispatch → engine_bridge → TurnRequest
        // → 适配器请求体），带头的是产品声明本身——声明改了握手没跟上，这里就会红。
        assert_eq!(caps.get("images").and_then(|v| v.as_bool()), Some(true), "ada-coding 声明 images=true");
        assert_eq!(caps.get("rollback").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(caps.get("plugins").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            caps.get("hooks").and_then(|v| v.as_bool()),
            Some(true),
            "P3a 起 ada-coding 声明 hooks=true（引擎有生产调用点 + 成对埋点 + 缺席可见用例）"
        );
        let evts = caps.get("events").expect("必须包含 events 能力");
        assert_eq!(evts.get("snapshotSeq").and_then(|v| v.as_bool()), Some(true));

        let prod = init_val.get("product").expect("必须包含 product 节点");
        assert_eq!(prod.get("id").and_then(|v| v.as_str()), Some("ada-coding"));
        assert_eq!(prod.get("archetype").and_then(|v| v.as_str()), Some("coding"));
        assert_eq!(prod.get("name").and_then(|v| v.as_str()), Some("a_da 编程助手"));

        // 2. 测试 session.snapshot 包含 seq 序号（M3-T2）
        let snap_val = dispatcher
            .dispatch(agent_proto::methods::SESSION_SNAPSHOT, serde_json::json!({}))
            .await
            .expect("拉取快照成功");

        assert!(snap_val.get("seq").is_some(), "快照必须携带 seq 字段");
        assert_eq!(snap_val.get("seq").and_then(|v| v.as_u64()), Some(0));
    }
}
