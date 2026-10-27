use super::*;

pub(super) fn builtin_tool_info(d: &agent_base::domain::ToolDescriptor) -> serde_json::Value {
    use agent_base::domain::{ApprovalPolicy, Execution};

    let kind = if d.is_readonly() {
        "只读工具"
    } else {
        "写入工具"
    };
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
pub(super) fn thread_workspace(store: &AgentStore, thread_id: &str) -> String {
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
pub(super) fn save_app_config(store: &AgentStore) -> Result<(), String> {
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
        obj.insert(
            "baseUrl".to_string(),
            serde_json::json!(store.provider.base_url),
        );
        obj.insert(
            "apiKey".to_string(),
            serde_json::json!(store.provider.api_key),
        );
        obj.insert("model".to_string(), serde_json::json!(store.provider.model));
        obj.insert(
            "protocol".to_string(),
            serde_json::to_value(&store.provider.protocol)
                .unwrap_or(serde_json::json!("openai_chat")),
        );
        obj.insert(
            "contextWindow".to_string(),
            serde_json::json!(store.config.context_window),
        );
        if let Some(mot) = store.config.max_output_tokens {
            obj.insert("maxOutputTokens".to_string(), serde_json::json!(mot));
        } else {
            obj.remove("maxOutputTokens");
        }
        obj.insert(
            "supportsImages".to_string(),
            serde_json::json!(store.config.supports_images),
        );
        // 模型请求重试次数（设置项，默认 5；重试耗尽即断开对话）
        obj.insert(
            "retryCount".to_string(),
            serde_json::json!(store.config.retry_count),
        );
        // 审批档位 / 深度档位 / 默认模式 / 外观：启动时都会从这份文件读回来
        // （`AgentStore` 的加载段），所以**必须写**。
        // 早先这几项只改内存、字段表里也没有它们，于是每次重启都回到默认值——
        // 表现就是"改完设置、重启又变回去了"。
        obj.insert(
            "approval".to_string(),
            serde_json::to_value(store.config.approval).unwrap_or(serde_json::json!("ask")),
        );
        obj.insert(
            "effort".to_string(),
            serde_json::to_value(store.config.effort).unwrap_or(serde_json::json!("medium")),
        );
        obj.insert(
            "mode".to_string(),
            serde_json::to_value(store.config.mode).unwrap_or(serde_json::json!("code")),
        );
        obj.insert(
            "appearance".to_string(),
            serde_json::json!(store.appearance),
        );
        if let Some(ref ch) = store.provider.custom_headers {
            obj.insert("customHeaders".to_string(), serde_json::json!(ch));
        } else {
            obj.remove("customHeaders");
        }
        obj.insert(
            "providers".to_string(),
            serde_json::to_value(&store.providers).unwrap_or(serde_json::json!([])),
        );
        obj.insert(
            "activeProviderId".to_string(),
            serde_json::json!(store.active_provider_id),
        );
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
pub(super) fn persist_failure_notice(
    session_mgr: &agent_node::session::SessionManager,
    thread_id: &str,
    text: &str,
) {
    if let Err(e) = session_mgr.append_notice_entry(thread_id, text, "error", None) {
        tracing::warn!("失败提示落盘失败（界面仍会显示，但下次同步就会丢）: {e}");
    }
}

/// 远程探测通用 /models 接口以获取可用模型列表
pub(super) async fn fetch_remote_models(
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
        agent_base::model::ModelProtocol::OpenAiChat
        | agent_base::model::ModelProtocol::OpenAiResponses => {
            if clean_base.ends_with("/v1") {
                format!("{}/models", clean_base)
            } else {
                format!("{}/v1/models", clean_base)
            }
        }
    };

    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10));

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
        agent_base::model::ModelProtocol::OpenAiChat
        | agent_base::model::ModelProtocol::OpenAiResponses => {
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

    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求模型列表失败 ({}): {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        return Err(format!("接口返回错误码 {}: {}", status, err_body));
    }

    let val: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析模型列表 JSON 失败: {}", e))?;

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
            let id = obj
                .get("id")
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = obj
                .get("display_name")
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
/// 按工作区装配引擎的工厂。
///
/// **必须是异步的**：插件工具/技能/提示词要靠 `collect_plugin_tools` 真的执行一遍插件
/// 才能拿到（Boa 是异步的），同步闭包只能装出一份**没有插件贡献**的孪生引擎——
/// 那正是"右侧面板列着 weather_query、模型却说没有这个工具"的根因。
pub type EngineFactory = Arc<
    dyn Fn(
            std::path::PathBuf,
        )
            -> agent_base::ports::BoxFuture<'static, Option<Arc<agent_base::engine::AgentRuntime>>>
        + Send
        + Sync,
>;

/// 轮外钩子点位的取消令牌：**永不取消**。
///
/// 为什么不是"传个真令牌"：管理面点位（插件启停 / 会话创建）本身就是一次 RPC 请求，
/// 它的生命周期由请求自己管（请求断开 → 整个 dispatch 被丢掉），没有"轮次取消"可言。
/// 用一个显式的 no-op 而不是 `agent_base::testing::NeverCancel`：测试替身出现在生产路径上，
/// 下一个读者会以为这里有测试残留。
pub(super) struct NeverCancelled;

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

    /// 磁盘侧变了（新建插件包 / 删除插件包 / 外部改了 `index.ts`）之后重建贡献集合。
    ///
    /// # 为什么需要单独一个入口，而不是复用 `reassemble`
    ///
    /// `reassemble` 的语义是"某个插件的**启停档位**变了"（收据里 `fromEnabled → toEnabled`
    /// 是要给用户看的事实）。而"新建一个插件"没有旧档位：它一开始就是启用的，讲
    /// "关闭 → 启用"是假话。两者**走同一条装配管线**，但收据的措辞必须各说各的，
    /// 否则界面按 `fromEnabled` 渲染时会出现"我没关过它"的困惑。
    ///
    /// 默认实现退回启停路径（`reason` 当插件标识）：只有启停一种触发源时语义仍然正确，
    /// 组合根按需覆写成 `scope: "refresh"`。
    fn refresh<'a>(
        &'a self,
        reason: &'a str,
    ) -> agent_base::ports::BoxFuture<'a, Option<agent_runtime::ReassemblyReport>> {
        self.reassemble(reason, true)
    }
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
    /// 插件贡献的**活体视图**（P4）：`plugin.list` 从它取工具 / 片段 / 技能 / 钩子点位。
    ///
    /// 为什么必须有：`PluginManager::scan_plugins` 只扫目录、**从不执行插件**，
    /// 于是每个磁盘插件的贡献计数恒为 0、核心工具页的"被插件重实现"永远不亮。
    /// 执行过插件的只有组合根，所以结论由它写进这个活体。
    pub plugin_contribution_views: Option<agent_node::plugins::PluginContributionStore>,
    /// 后台任务表（`run_background` 族与 `task.list` / `task.kill` 共用）。
    pub background_tasks: Option<Arc<agent_toolkit::background::BackgroundTasks>>,
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
            plugin_contribution_views: None,
            background_tasks: None,
        }
    }

    /// 带上后台任务表（`task.list` / `task.kill` 需要与工具共用同一份）。
    pub fn with_background_tasks(
        mut self,
        tasks: Arc<agent_toolkit::background::BackgroundTasks>,
    ) -> Self {
        self.background_tasks = Some(tasks);
        self
    }

    /// 带上插件贡献视图（P4）：`plugin.list` 才能列出"实际装上了什么"。
    pub fn with_plugin_contribution_views(
        mut self,
        views: agent_node::plugins::PluginContributionStore,
    ) -> Self {
        self.plugin_contribution_views = Some(views);
        self
    }

    pub fn with_factory(mut self, factory: EngineFactory) -> Self {
        self.factory = Some(factory);
        self
    }

    /// 带上插件技能（SP-45）。没带 = 如实为空，不是"假装没有插件"。
    pub fn with_plugin_skills(mut self, skills: agent_node::skills::SkillStore) -> Self {
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
pub(super) struct SlowArm {
    pub(super) method: String,
    pub(super) started: std::time::Instant,
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
pub(super) fn revert_response(
    outcome: Option<agent_node::checkpoint::RevertOutcome>,
) -> serde_json::Value {
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

pub(super) fn merge_contribution_views(
    store: Option<&agent_node::plugins::PluginContributionStore>,
    items: &mut [agent_node::plugins::PluginItem],
) {
    let Some(store) = store else {
        return;
    };
    let views = match store.read() {
        Ok(views) => views.clone(),
        Err(_) => return,
    };
    for item in items.iter_mut() {
        // 内置插件的贡献是编译进来的（`builtins.rs` 自带清单），静态扫描已经给全了
        if item.scope == "builtin" {
            continue;
        }
        let Some(view) = views.iter().find(|v| v.plugin_id == item.id) else {
            continue;
        };
        item.tools = view.tools.clone();
        item.skills = view.skills.clone();
        item.prompts = view
            .prompts
            .iter()
            .map(|p| serde_json::json!({ "id": p.id, "content": p.content }))
            .collect();
        item.hooks = view.hooks.clone();
        // `plugin.contributions.tools` 是声明形状（装配与审计读它），与上面对齐着填
        item.plugin.contributions.tools = view
            .tools
            .iter()
            .map(|t| agent_node::plugins::PluginToolDeclaration {
                name: t.name.clone(),
                label: None,
                description: t.description.clone(),
                parameters: t
                    .parameters
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
                // 投影回声明形状：读写性来自描述符（`is_write` 就是 `!is_readonly()`），
                // 这里只是把同一份事实写成声明侧的字符串。
                access: if t.is_write { "write" } else { "read" }.to_string(),
            })
            .collect();
    }
}

/// 把界面传来的图片**归一成可以直接进模型请求的 data URI**。
///
/// 界面产出的一律是 data URI（粘贴/拖拽走 `FileReader.readAsDataURL`，选文件走
/// `fs.read_base64`），但老盘上恢复出来的会话可能是裸路径，所以这里兜一手。
///
/// **为什么归一化在这层**：`agent-base` 是零 IO 的领域层，引擎拿不到文件系统——
/// 读盘转 base64 只能发生在 RPC 入口。读不到的图片**丢掉并记日志**，不给模型一个
/// 它取不到的路径（那会变成一个只有上游能看懂的报错）。
pub(super) fn normalize_images(images: Option<Vec<String>>) -> Option<Vec<String>> {
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
            Err(e) => warn!(
                "图片无法读取，本轮丢弃该图（path={trimmed}）：{}",
                e.message
            ),
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 辅助函数：根据目标工作区优先从缓存中获取引擎，若缺失且存在工厂则现场装配并写入缓存
pub(super) async fn resolve_engine_from_cache(
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
        if let Some(eng) = f(p.clone()).await {
            let mut guard = cache.lock().await;
            guard.insert(p, eng.clone());
            return Some(eng);
        }
    }
    fallback.clone()
}

/// 把将被丢弃的历史渲染成给"压缩器"看的纯文本（带角色前缀，工具输出取前 400 字）。
pub(super) fn render_transcript_for_summary(items: &[Arc<Item>]) -> String {
    let mut out = String::new();
    for it in items {
        match it.as_ref() {
            Item::User { text, .. } => out.push_str(&format!("【用户】{}\n", text.trim())),
            Item::Assistant { text, .. } if !text.trim().is_empty() => {
                out.push_str(&format!("【助手】{}\n", text.trim()))
            }
            Item::Thinking { text, .. } if !text.trim().is_empty() => {
                out.push_str(&format!("【思考】{}\n", text.trim()))
            }
            Item::Tool {
                name,
                status,
                output,
                ..
            } => {
                let brief: String = output.as_deref().unwrap_or("").chars().take(400).collect();
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
    if text.is_empty() { None } else { Some(text) }
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
            .find_map(|it| match it.as_ref() {
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
            t.items = vec![Arc::new(Item::Compact {
                id: compact_id.clone(),
                at: now,
                summary: summary.clone(),
                pre_tokens,
                post_tokens,
                saved_tokens: pre_tokens.saturating_sub(post_tokens),
                turns_summarized: to_summarize as u64,
            })];
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
