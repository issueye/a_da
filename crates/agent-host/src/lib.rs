//! AGENT HOST：**宿主装配层**（`docs/agent-base-design.md` v0.2 §9）。
//!
//! # 它解决什么问题
//!
//! W3 之前，产品二进制（`products/ada-coding`）与桌面壳宿主（当时的 Tauri 宿主）
//! 都是直接调 `agent_core` 的 **legacy 主循环** `run_agent_loop`——也就是说：`AgentRuntime`
//! （真引擎）虽然装配得出来，**产品运行时却根本没在用它**（计划 §2 P0-1）。
//!
//! 本 crate 是那个缺掉的中间层：把「产品声明 + 宿主态」装配成**带真实端口**的
//! `AgentRuntime`，让上层（产品二进制 / ws 宿主）只需要说"按这份 spec 起一个产品"。
//!
//! ```text
//! products/ada-coding ─→ agent-host ─→ agent-runtime ─→ agent-base（引擎）
//!                            └─────→ agent-adapter（真实端口实现）
//! ```
//!
//! # 端口真实性（INV-2：端口必须真接线，不许留 double）
//!
//! [`run_from_spec`] 装配的**全部是生产实现**，没有一个测试替身：
//!
//! | 端口 | 真实实现 | 来源 |
//! |---|---|---|
//! | `Scope` | [`WorkspaceScope`] | `agent-adapter`（路径沙箱） |
//! | `SessionStore` | [`FsSessionStore`] | `agent-adapter`（JSONL 落盘） |
//! | `PromptSource` | [`CodingPromptSource`] | `agent-adapter`（静态前缀 + 扩展段） |
//! | `Clock` | [`SystemClock`] | `agent-adapter`（全仓唯一读系统时间处） |
//! | `ModelClient` | [`NetworkModelClient`] | `agent-adapter`（三家协议 SSE） |
//! | `ApprovalGate` | [`HostApprovalGate`] | 宿主执行侧（AGENTS.md §14：策略在插件、执行在核心） |
//! | `EventSink` | [`WsEventSink`] | 宿主事件出口（有序账本 + seq 违约检测） |
//!
//! # 边界（本任务不做什么）
//!
//! - **不建第二个 ws 服务**：`WsHostServer` 已存在于 `agent_core::server`，
//!   把它的分发切到本 crate 装配出的 runtime 是 **W3-T2**；这里只负责把 runtime
//!   与事件出口准备好。
//! - **不做能力位自检**：`spec.capabilities` 与握手一致性属 **W3-T6**。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_adapter::cancel::CancelHandle;
use agent_adapter::clock::SystemClock;
use agent_adapter::model::client::NetworkModelClient;
use agent_adapter::prompt::{CodingPromptSource, ProductIdentity};
use agent_adapter::scope::WorkspaceScope;
use agent_adapter::store::FsSessionStore;
use agent_base::domain::ToolDescriptor;
use agent_base::engine::{AgentRuntime, TurnOutcome};
use agent_base::model::ProviderConfig;
use agent_base::ports::{ModelClient, Tool};
use agent_node::approval::{ApprovalManager, HostApprovalGate};
use agent_rpc::server::{EngineInjection, StateBroadcaster, WsEventSink};
use agent_rpc::state::AgentStore;
use agent_runtime::{AgentSpec, CapabilitySpec, IdentitySpec, ProductBuilder, SpecError};
use thiserror::Error;
use tokio::sync::RwLock;

pub mod live;

pub use live::{new_skill_store, LivePluginContributions, LivePluginReassembler, SkillStore};

#[derive(Debug, Error)]
pub enum HostError {
    #[error("产品声明违约：{0}")]
    Spec(#[from] SpecError),
    #[error("宿主装配失败：{0}")]
    Assembly(String),
}

/// 装配一个宿主产品所需的输入。
///
/// `model` 可注入：生产路径用 `None`（走真实 [`NetworkModelClient`]），
/// 测试可注入脚本化模型——**注入点存在不等于生产路径在用替身**，
/// 生产路径的默认值是真实实现。
pub struct HostOptions {
    pub workspace: PathBuf,
    /// 会话落盘根目录（默认 `<workspace>/.a-da/sessions`）。
    pub sessions_root: Option<PathBuf>,
    /// 系统提示词扩展段（插件/技能注入）。
    pub prompt_extensions: Vec<(String, String)>,
    pub model: Option<Arc<dyn ModelClient>>,
    /// 事件广播通道：由 ws 传输层提供（它消费 `rx` 推给界面）。
    ///
    /// `None` = **离线形态**（只记账不广播）——CLI/测试/无界面宿主用，
    /// 避免"没人消费的无界通道"在长轮次里无限增长。
    pub broadcast_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// 宿主已有的会话态（UI 快照/配置的来源）。
    ///
    /// **必须与 ws 宿主共用同一个实例**：新引擎的审批闸门（[`HostApprovalGate`]）要读
    /// `store.config.approval` 才知道当前档位。各造一个 store 会让"界面上是只读档、
    /// 引擎却按默认档跑"。
    pub store: Option<Arc<RwLock<AgentStore>>>,
    /// 技能管理器实例（若不注入则装配默认实例）。
    pub skill_mgr: Option<Arc<agent_node::skills::SkillManager>>,
    /// 审批 waiter 表（若不注入则自建一个）。
    ///
    /// 🔴 **同一进程里所有引擎必须共用同一个实例**：`Dispatcher.approval_mgr` 是 UI 的
    /// `approval.decide` 落到的那张表，而闸门（[`HostApprovalGate`]）把 waiter 注册到
    /// 自己那份表上。各装配一次就多一张表，UI 的批准就永远送不到闸门——表现为
    /// "审批卡弹出来了、点了批准却一直卡住"，直到 30 分钟超时被自动拒绝。
    /// **按工作区动态装配**（`EngineFactory`）时尤其容易漏：那次装配同样必须复用这张表。
    pub approval_mgr: Option<Arc<ApprovalManager>>,
    /// **组合根预先装好的插件贡献**（P1·SP-16 的工具 + P2·SP-44/45 的片段与技能
    /// + P3a·SP-50 的钩子句柄）。
    ///
    /// 为什么由外部传而不是在这里收集：收集要**异步装载**插件脚本（`ScriptHost::load`），
    /// 而 `run_from_spec` 是同步的（它连带被同步测试与 CLI 用着）。把异步那一步留在
    /// 组合根（产品 `main` 本来就是 async），这里只负责"装进管线 + 报审计与拒绝"。
    ///
    /// 四类贡献**合成一个结构**（而不是四个 setter）：它们出自**同一次**装载，
    /// 分开传就会出现"工具装了、钩子没装"这种半截状态，而且没人说得清哪个是权威。
    pub plugin_contributions: PluginContributions,
}

/// 组合根收集好的插件贡献（形状与 [`agent_node::scripts::PluginToolAssembly`] 对齐）。
#[derive(Default)]
pub struct PluginContributions {
    /// `(plugin_id, tool)`：id 是**来源标注**，装配归属与埋点都靠它
    pub tools: Vec<(String, Arc<dyn Tool>)>,
    /// `(plugin_id, 片段 id, 正文)`：进提示词时带 `plugin:<id>` 来源
    pub prompt_fragments: Vec<(String, String, String)>,
    /// `(plugin_id, 插件名, 声明)`：并进技能扫描结果（第三个来源）
    pub skills: Vec<(String, String, agent_node::plugins::PluginSkillDeclaration)>,
    /// 钩子运行时（P3a · SP-50）：由组合根用 `collect_hooks` 装好。
    ///
    /// 为什么连运行时一起传进来、而不是传句柄让宿主自己装：装它要 **`ScriptHost`**
    /// （按句柄调处理器），而宿主这一层没有也不该有脚本宿主（它只做组装与审计）。
    pub hook_runner: Option<Arc<dyn agent_base::ports::HookRunner>>,
    /// 装配期被拒的钩子声明（越级 / 未实现 / 重复），由宿主如实报成诊断。
    pub hook_rejections: Vec<agent_node::hooks::HookRejection>,
}

impl HostOptions {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            sessions_root: None,
            prompt_extensions: Vec::new(),
            model: None,
            broadcast_tx: None,
            store: None,
            skill_mgr: None,
            approval_mgr: None,
            plugin_contributions: PluginContributions::default(),
        }
    }

    /// 传入组合根收集好的插件贡献（工具 / 片段 / 技能 / 钩子句柄）。
    pub fn with_plugin_contributions(mut self, contributions: PluginContributions) -> Self {
        self.plugin_contributions = contributions;
        self
    }

    /// 复用宿主已有的审批 waiter 表（跨引擎共享，见字段注释）。
    pub fn with_approval_manager(mut self, mgr: Arc<ApprovalManager>) -> Self {
        self.approval_mgr = Some(mgr);
        self
    }

    /// 复用宿主已有的会话态（推荐：让引擎与 UI 看同一份配置）。
    pub fn with_store(mut self, store: Arc<RwLock<AgentStore>>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn with_skill_manager(mut self, mgr: Arc<agent_node::skills::SkillManager>) -> Self {
        self.skill_mgr = Some(mgr);
        self
    }

    pub fn with_sessions_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.sessions_root = Some(root.into());
        self
    }

    pub fn with_prompt_extensions(mut self, extensions: Vec<(String, String)>) -> Self {
        self.prompt_extensions = extensions;
        self
    }

    /// 注入模型客户端（测试用；生产留 `None`）。
    pub fn with_model(mut self, model: Arc<dyn ModelClient>) -> Self {
        self.model = Some(model);
        self
    }

    /// 接入 ws 传输层的事件广播通道。
    pub fn with_broadcast(mut self, tx: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        self.broadcast_tx = Some(tx);
        self
    }
}

/// 装配完成的宿主产品。
pub struct HostedProduct {
    /// 真引擎：所有轮次都必须走它（INV-1 单一引擎）。
    pub runtime: AgentRuntime,
    /// 产品声明（原样保留，供宿主读取 identity/capabilities）。
    pub spec: AgentSpec,
    /// 宿主会话态（UI 快照与配置的来源）。
    pub store: Arc<RwLock<AgentStore>>,
    /// 审批执行侧（waiter 表）。
    pub approval: Arc<ApprovalManager>,
    /// 事件出口（有序账本）。
    pub events: Arc<WsEventSink>,
    /// 轮次取消句柄（每次轮次取 `child()`）。
    pub cancel: CancelHandle,
    /// 装配期工具描述符快照（真源投影，供界面/自检读取）。
    ///
    /// ⚠️ 它是**快照**：插件启停的重装配（SP-46）改的是活体注册表（
    /// [`LivePluginContributions::catalog`]，引擎每轮查的就是那本），这里不跟着变。
    /// 要问"现在到底装了什么"，用活体注册表。
    pub tool_descriptors: Vec<ToolDescriptor>,
    /// 子智能体配置表（W4-T5）。宿主若要与 Dispatcher 共用（`SUBAGENT_RESUME` 等），
    /// 可把它传给 `WsHostServer`；不传也不会出错——profile 来自内置表 + 用户配置。
    pub subagent_mgr: Arc<agent_node::subagents::SubagentManager>,
    /// 产品声明的 `capabilities.images`（W5-T4）：**真实消费者**——
    /// 声明 `false` 的产品不得把图片送进模型。
    pub accepts_images: bool,
    pub workspace: PathBuf,
    /// 活的插件贡献集合（P2 · SP-46）：组合根据此驱动"插件启停真的改变贡献集合"。
    ///
    /// 拿它调 [`LivePluginContributions::apply`]，工具 / 提示词片段 / 技能三样会**一起**换掉；
    /// 没拿它（老调用点）就只是没有这条能力，其余行为一字不变。
    pub live: Arc<LivePluginContributions>,
}

impl HostedProduct {
    /// 工具名清单（按装配顺序）。
    pub fn tool_names(&self) -> Vec<&str> {
        self.tool_descriptors.iter().map(|d| d.name.as_str()).collect()
    }

    /// 跑一轮（薄封装，便于宿主与测试用同一条路径）。
    pub async fn run_turn(
        &self,
        thread_id: &str,
        config: ProviderConfig,
    ) -> Result<TurnOutcome, HostError> {
        self.run_turn_with_images(thread_id, config, None).await
    }

    /// 跑一轮，并可带图片输入。
    ///
    /// **W5-T4**：`capabilities.images == false` 的产品在这里**拒绝**图片输入——
    /// 声明的能力位必须真的决定行为，而不是"声明了没人读"。
    pub async fn run_turn_with_images(
        &self,
        thread_id: &str,
        config: ProviderConfig,
        images: Option<Vec<String>>,
    ) -> Result<TurnOutcome, HostError> {
        if images.as_ref().is_some_and(|i| !i.is_empty()) && !self.accepts_images {
            return Err(HostError::Assembly(
                "产品声明 capabilities.images=false，拒绝图片输入".to_string(),
            ));
        }
        let mut req = agent_base::engine::TurnRequest::new(thread_id, config);
        if let Some(imgs) = images {
            req = req.with_images(imgs);
        }
        let child = self.cancel.child();
        self.runtime
            .run_turn(req, self.events.as_ref(), &child)
            .await
            .map_err(|e| HostError::Assembly(format!("轮次执行失败：{e}")))
    }
}

/// 按产品声明装配一个可运行的宿主产品（**真实端口**，见 crate 文档的表）。
pub fn run_from_spec(spec: AgentSpec, options: HostOptions) -> Result<HostedProduct, HostError> {
    // 提问通道必须**在装配期就装上**，不能等"谁先碰全局单例谁顺手装"。
    //
    // `ask_user` 的注册器原本只挂在 `global_question_manager()` 的 `OnceLock` 初始化里，
    // 而生产路径上第一个碰它的地方是"用户答复提问"或"中止会话"——于是**全新进程里的
    // 第一次 ask_user 必然失败**（实测 PM 就这样：工具直接回
    // "当前环境未接入交互提问协调器（无事件问答通道）"，只能改成自己猜），
    // 只有先答过一次提问或中止过一次会话之后才会好。装配期显式装上是幂等的。
    agent_node::approval::init_question_registrar();

    let workspace = options.workspace.clone();
    let sessions_root = options
        .sessions_root
        .clone()
        .unwrap_or_else(|| workspace.join(".a-da").join("sessions"));

    // ── 宿主态 ────────────────────────────────────────────────────────────
    // 复用宿主传入的 store（审批档位必须与界面看到的一致）；没有才自建
    let store = options
        .store
        .clone()
        .unwrap_or_else(|| Arc::new(RwLock::new(AgentStore::new(
            workspace.to_string_lossy().to_string(),
        ))));
    // 复用宿主注入的 waiter 表：**同一进程里所有引擎（含按工作区动态装配的那些）
    // 必须把 waiter 注册到同一张表**，否则 UI 的 `approval.decide` 找不到闸门的等待者
    // （见 `HostOptions::approval_mgr`）。
    let approval = options
        .approval_mgr
        .clone()
        .unwrap_or_else(|| Arc::new(ApprovalManager::new()));

    // 事件出口：有 ws 传输层就广播，没有就走离线形态（只记账，不产生无人消费的通道）
    let events = Arc::new(match &options.broadcast_tx {
        Some(tx) => {
            let seq = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let broadcaster = StateBroadcaster::new(store.clone(), seq, tx.clone());
            WsEventSink::new(broadcaster)
        }
        None => WsEventSink::with_capacity(None, 4096),
    });

    // ── 真实端口 ──────────────────────────────────────────────────────────
    //
    // W5-T4：**显式读产品声明**。`identity` 与 `capabilities` 原先解析出来没人读
    // （`cargo xtask verify-spec` 的红例基线），现在各自有真实消费者：
    // - `identity` → 系统提示词（产品名字/人格/语言）；
    // - `capabilities.subagents` → 是否装配委派工具；
    // - `capabilities.images` → 是否接受图片输入。
    let IdentitySpec { name, persona, locale } = &spec.identity;
    let caps: &CapabilitySpec = &spec.capabilities;

    let scope: Arc<dyn agent_base::ports::Scope> = {
        // P1b（SP-33）：作用域按**权限档位**裁决路径边界。档位与审批闸门读同一处
        // （`StoreBackedNodeConfig` 同时实现 `NodeConfigSource` 与 `ApprovalModeSource`）——
        // 两处各读一份会出现"界面显示完全访问、路径仍被锁在工作区"这种自相矛盾。
        let mode_source: Arc<dyn agent_adapter::scope::ApprovalModeSource> =
            Arc::new(agent_rpc::server::StoreBackedNodeConfig::new(store.clone()));
        // 保留目录：应用自己的配置与密钥（`~/.a-da`）。**任何档位都不可越**（见 `scope::access`）。
        let app_home_root = {
            use agent_base::ports::AppHome;
            agent_adapter::app_home::SystemAppHome::from_env()
                .root()
                .to_path_buf()
        };
        Arc::new(agent_adapter::scope::ApprovalAwareScope::new(
            WorkspaceScope::new(workspace.clone()),
            mode_source,
            vec![app_home_root],
        ))
    };
    let session_store: Arc<dyn agent_base::ports::SessionStore> = Arc::new(FsSessionStore::new(
        sessions_root,
        workspace.to_string_lossy().to_string(),
    ));

    let skill_mgr = options.skill_mgr.clone().unwrap_or_else(|| {
        Arc::new(agent_node::skills::SkillManager::new())
    });
    let ws_str = workspace.to_string_lossy().to_string();
    // 组合根收好的插件贡献（四类出自**同一次**装载，见 `PluginContributions`）
    let PluginContributions {
        tools: plugin_tools,
        prompt_fragments: plugin_prompt_fragments,
        skills: plugin_skills_decls,
        hook_runner,
        hook_rejections,
    } = options.plugin_contributions;
    // P2·SP-45：插件技能在装配期就转成扫描结果的形状（带 plugin_id / plugin_name / scope）——
    // 于是提示词里那条技能片段能标上 `plugin:<id>`，界面列表也认得出它是插件带来的。
    //
    // SP-46：放进**共享活体**而不是一份快照：插件启停后重装配会原地替换它，
    // 而解析器每次取提示词都会重新读一遍——于是"关掉插件 → 下一轮提示词里就没有它了"。
    let plugin_skill_summaries: Vec<agent_node::skills::SkillSummary> = plugin_skills_decls
        .iter()
        .map(|(pid, pname, decl)| decl.to_summary(pid, pname))
        .collect();
    let plugin_skill_store = new_skill_store(plugin_skill_summaries);
    // SP-46：片段同理走共享活体（提示词源每步读它一次）
    let plugin_fragment_store = agent_adapter::prompt::new_plugin_fragment_store();
    if let Ok(mut f) = plugin_fragment_store.write() {
        *f = plugin_prompt_fragments.clone();
    }
    let skill_resolver: agent_adapter::prompt::SkillResolver = {
        let sm = skill_mgr.clone();
        let ws = ws_str.clone();
        let plugin_skills = plugin_skill_store.clone();
        Arc::new(move || {
            let extra = plugin_skills.read().map(|g| g.clone()).unwrap_or_default();
            // 技能三来源在这里合流：目录 / **插件** / 内置（见 `scan_skills_with`）
            sm.scan_skills_with(Some(&ws), &extra)
                .into_iter()
                .filter(|s| s.enabled)
                .map(|s| {
                    let mut p = agent_adapter::prompt::SkillPrompt::new(s.name, s.description, s.body);
                    if let Some(w) = s.when_to_use {
                        p = p.with_when_to_use(w);
                    }
                    // 归因：插件带来的技能在 `prompt.composed` 的片段报表里标 `plugin:<id>`
                    if let Some(pid) = s.plugin_id {
                        p = p.with_source(format!("plugin:{pid}"));
                    }
                    p
                })
                .collect()
        })
    };

    let prompt: Arc<dyn agent_base::ports::PromptSource> = Arc::new(
        CodingPromptSource::new(ws_str)
            .with_identity(ProductIdentity::new(name.clone(), persona.clone(), locale.clone()))
            .with_extensions(options.prompt_extensions.clone())
            // P2·SP-44：插件声明的提示词片段（带 plugin:<id> 来源）；SP-46 走共享活体
            .with_shared_plugin_fragments(plugin_fragment_store.clone())
            .with_skill_resolver(skill_resolver),
    );
    let clock: Arc<dyn agent_base::ports::Clock> = Arc::new(SystemClock);
    let model: Arc<dyn ModelClient> = options
        .model
        .clone()
        .unwrap_or_else(|| Arc::new(NetworkModelClient::new()));
    // S2：节点配置端口。**唯一**知道"配置存在 AgentStore 里"的地方在这里
    // （组合根）——节点层（审批闸门、委派工具）只认端口。
    let node_config: Arc<dyn agent_node::node_config::NodeConfigSource> =
        Arc::new(agent_rpc::server::StoreBackedNodeConfig::new(store.clone()));
    let approval_gate: Arc<dyn agent_base::ports::ApprovalGate> = Arc::new(
        HostApprovalGate::new(node_config.clone(), approval.clone()),
    );

    // ── 按声明装配（P2·SP-40：走**统一装配管线**）──────────────────────────
    //
    // 工具包工具与插件工具都往同一条管线里加，"同名谁胜出、谁该炸、谁该跳过"由它一处决定
    // （此前这段规则手写在 `agent-host` 里，而提示词/技能各有自己的一套）。
    let mut assembly = agent_runtime::AssemblyBuilder::new();
    for tool in
        agent_toolkit::tools_for_toolkits(&spec.toolkits, &workspace).map_err(SpecError::Violation)?
    {
        assembly
            .add_builtin_tool(tool)
            .map_err(|e| SpecError::Violation(e.message))?;
    }
    for (plugin_id, tool) in plugin_tools.iter().cloned() {
        assembly.add_plugin_tool(plugin_id, tool);
    }
    let plan = assembly.build();
    let tools: Vec<Arc<dyn Tool>> = plan.tools.clone();
    let contribution_summary = plan.contributions.len();

    // ── 钩子装配（P3a · SP-50/54/53）───────────────────────────────────────
    //
    // 分级校验（越级/未实现/重名）与运行时构建都在组合根完成（它才有脚本宿主），
    // 这里只负责两件事：① 按产品声明决定要不要装；② 把拒绝理由如实报成诊断。
    //
    // 🔴 声明 `capabilities.hooks == false` 的产品**不装**：那时引擎没有 runner，
    // 一条 hook.* 诊断都不发。为此**额外报一条**说清"不是你的插件写错了，
    // 是这个产品没开钩子"——否则插件作者会一直对着自己的代码找问题。
    let hooks_blocked_by_product = !caps.hooks && hook_runner.is_some();
    if hooks_blocked_by_product {
        tracing::warn!("[hooks] 产品声明未开钩子（capabilities.hooks=false）：已装好的钩子不会被执行");
    }
    let hook_runner = if caps.hooks { hook_runner } else { None };

    // ── 活的贡献集合（P2·SP-46）────────────────────────────────────────────
    //
    // 装配期就把"能改的那本账"造好并挂出去：工具注册表、片段活体、技能活体三者
    // 与引擎/提示词源用的是**同一份**——否则重装配换的是副本，开关就成了摆设。
    let live = Arc::new(LivePluginContributions::new(
        &plan,
        plugin_fragment_store.clone(),
        plugin_skill_store.clone(),
    ));

    // 被插件顶掉的内置工具**必须报出来**：静默替换等于把用户信任的工具悄悄换掉。
    if !plan.replaced_builtins.is_empty() {
        use agent_base::domain::{
            DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_PLUGIN_REPLACED_BUILTIN,
        };
        // 装配期没有并发（引擎还没开始跑），`try_write` 基本必然成功；
        // 万一拿不到锁，宁可丢这条审计也不能在运行时线程上阻塞（会 panic）。
        match store.try_write() {
            Ok(mut store_w) => {
                for name in &plan.replaced_builtins {
                    agent_rpc::server::diag::project_out_of_turn(
                        &mut store_w,
                        &DiagnosticEvent::new(
                            DiagFamily::Plugin,
                            POINT_PLUGIN_REPLACED_BUILTIN,
                            Origin::Builtin {
                                name: name.clone(),
                            },
                        )
                        .with_outcome(DiagOutcome::Applied)
                        .with_reason(format!("`{name}` 的实现在本次装配里由插件提供")),
                    );
                }
            }
            Err(_) => tracing::warn!(
                "[plugins] 装配期拿不到 store 写锁，未能记录 {:?} 的替换审计",
                plan.replaced_builtins
            ),
        }
    }
    for skipped in &plan.skipped {
        tracing::warn!(
            "[assembly] 跳过一条 {} 贡献 `{}`（{}）：{}",
            skipped.kind.label(),
            skipped.id,
            skipped.origin.label(),
            skipped.reason
        );
    }

    // 被拒的**钩子声明**必须报出来（P3a·SP-54 的验收判据）：
    // "第三方想在控制面拦下"这类问题如果只在装配期 `tracing::warn` 一下，
    // 插件作者在界面上看不到任何线索，只会以为自己的拦截生效了。
    if !hook_rejections.is_empty() || hooks_blocked_by_product {
        use agent_base::domain::{
            DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_PLUGIN_SKIPPED,
        };
        if let Ok(mut store_w) = store.try_write() {
            for r in &hook_rejections {
                agent_rpc::server::diag::project_out_of_turn(
                    &mut store_w,
                    &DiagnosticEvent::new(
                        DiagFamily::Hook,
                        POINT_PLUGIN_SKIPPED,
                        match r.plugin_id.as_str() {
                            "" => Origin::Host,
                            id => Origin::Plugin { id: id.to_string() },
                        },
                    )
                    .with_outcome(DiagOutcome::Skipped)
                    .with_reason(r.reason.clone())
                    .with_data(serde_json::json!({ "point": r.point })),
                );
            }
            if hooks_blocked_by_product {
                agent_rpc::server::diag::project_out_of_turn(
                    &mut store_w,
                    &DiagnosticEvent::new(DiagFamily::Hook, POINT_PLUGIN_SKIPPED, Origin::Host)
                        .with_outcome(DiagOutcome::Skipped)
                        .with_reason("hooks_disabled_by_product")
                        .with_data(serde_json::json!({
                            "hint": "该产品声明 capabilities.hooks=false，插件钩子不会被调用",
                        })),
                );
            }
        }
    }
    tracing::debug!("[assembly] 本轮装配贡献 {contribution_summary} 条");

    // W4-T5：`invoke_subagent` 是**宿主耦合**工具（要 SubagentManager / 父 provider / 检查点），
    // 工具包工厂构造不出来，所以由组合根注入。
    //
    // 但它是否出现由**产品声明**决定（INV-10 能力优先）：`capabilities.subagents == false`
    // 的产品不该拿到委派工具——否则"声明说不支持子智能体，模型却看得到这个工具"。
    //
    // S3：工具不再自己跑子智能体，而是经 `AgentBus` 端口派活。
    // S6：**用哪个总线由声明决定**（`capabilities.delegation`）——
    // 这决定"这个实例在委派链上的位置"，是装配事实，不是运行时探测出来的。
    let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
    let (agent_bus, delegation_depth): (Arc<dyn agent_node::agent_bus::AgentBus>, u32) =
        match spec.capabilities.delegation {
            agent_runtime::DelegationMode::Gateway => {
                // 声明了经网关委派，却没说网关在哪 → **装配失败**，不是"回退到本地"。
                //
                // 回退会静默改变行为：产品声明"我要跨节点协作"，实际却退化成进程内临时子智能体。
                // 宁可起不来，也不要"看起来装上了"。
                let endpoint = spec
                    .gateway
                    .as_ref()
                    .map(|g| g.endpoint.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .ok_or_else(|| {
                        SpecError::Violation(
                            "产品声明 capabilities.delegation=\"gateway\"，但缺少 gateway.endpoint——\
                             无法装配网关总线（不回退到本地：那会静默改变行为）"
                                .to_string(),
                        )
                    })?;
                tracing::info!("按产品声明装配**网关**委派总线 → {endpoint}");
                (
                    Arc::new(agent_rpc::gateway_bus::GatewayAgentBus::new(endpoint, 1)),
                    // 经网关委派：本节点处在委派链第 1 层
                    1,
                )
            }
            agent_runtime::DelegationMode::Local => (
                Arc::new(agent_node::subagents::LocalAgentBus::new(
                    workspace.clone(),
                    subagent_mgr.clone(),
                    node_config.clone(),
                    None,
                    // W4-T6：子智能体的工具按**同一份产品声明**装配，而不是另一张手写清单
                    spec.toolkits.clone(),
                )),
                // 进程内委派由 `NEVER_FOR_SUBAGENT` 拦，不使用深度
                0,
            ),
        };
    let mut tools = tools;
    if caps.subagents {
        // S6 补完：深度来源。有会话态就用它（网关派活会在线程上记深度），
        // 没有就退化成固定 0 —— 那是**正确**的默认（没有会话态 = 不是被派活的）。
        // `store` 在这个作用域里已经是 `Arc<RwLock<AgentStore>>`（上面 unwrap_or_else 保证）
        let depth_source: Arc<dyn agent_node::delegation_depth::DelegationDepthSource> =
            Arc::new(
                agent_rpc::server::delegation_depth::StoreBackedDelegationDepth::new(
                    store.clone(),
                ),
            );
        tools.push(Arc::new(
            agent_node::subagents::InvokeSubagentTool::with_depth_source(
                agent_bus.clone(),
                delegation_depth,
                Some(depth_source),
            ),
        ));
    } else {
        tracing::info!("产品声明 capabilities.subagents=false → 不装配 invoke_subagent");
    }

    let tool_descriptors: Vec<ToolDescriptor> =
        tools.iter().map(|t| t.descriptor().clone()).collect();

    // 借用 `spec.capabilities` 到这里结束（后面要把 `spec` move 进 `HostedProduct`）
    let accepts_images = caps.images;

    let runtime = ProductBuilder::new(spec.clone())
        .with_tools(tools)
        // SP-46：引擎用**活体注册表**（重装配要往同一本账里 register/unregister）
        .with_catalog(live.catalog())
        .with_model(model)
        .with_approval(approval_gate)
        .with_store(session_store)
        .with_prompt(prompt)
        .with_scope(scope)
        .with_clock(clock)
        .build()?;

    // P3a·SP-51/53：装上钩子运行时（单点位预算在它内部，来自插件能力的 `hookTimeoutMs`）。
    // 只有产品开了钩子才装 runner——没开就一条 hook.* 诊断都不发（见上面的说明）。
    let mut runtime = runtime;
    if let Some(runner) = hook_runner {
        runtime = runtime.with_hooks(runner);
    }

    Ok(HostedProduct {
        runtime,
        spec,
        store,
        approval,
        events,
        cancel: CancelHandle::new(),
        tool_descriptors,
        subagent_mgr,
        accepts_images,
        workspace,
        live,
    })
}

/// 从 spec JSON 文本装配（产品二进制读自己的 `agent.spec.json` 后调用）。
pub fn run_from_spec_json(
    spec_json: &str,
    options: HostOptions,
) -> Result<HostedProduct, HostError> {
    let spec = AgentSpec::from_json_str(spec_json)
        .map_err(|e| HostError::Assembly(format!("产品规格解析失败：{e}")))?;
    run_from_spec(spec, options)
}

/// 为宿主装配**注入包**（`EngineInjection`）：宿主（产品二进制 / 桌面壳）只需要
/// 把 `store`、工作区与自己的 `agent.spec.json` 交给它。
///
/// 收敛在这里的理由（R2：一处真源）：产品宿主所需的装配完全一样——复用宿主的 `store`、
/// 会话落盘到 app home、把 `ApprovalManager` 一并带出去给 `Dispatcher` 共用。
/// 各写一份迟早漂移。
///
/// `sessions_root` 由调用方给出（通常是 `get_app_home()/sessions`）。
pub fn build_engine_injection(
    store: &Arc<RwLock<AgentStore>>,
    workspace: impl AsRef<Path>,
    spec_json: &str,
    sessions_root: impl Into<PathBuf>,
) -> Result<EngineInjection, HostError> {
    let spec = AgentSpec::from_json_str(spec_json)
        .map_err(|e| HostError::Assembly(format!("产品规格解析失败：{e}")))?;
    let sessions_root_buf: PathBuf = sessions_root.into();
    let options = HostOptions::new(workspace.as_ref())
        .with_store(store.clone())
        .with_sessions_root(sessions_root_buf.clone());

    let HostedProduct { runtime, approval, spec, .. } = run_from_spec(spec, options)?;

    let spec_json_string = spec_json.to_string();
    let store_for_factory = store.clone();
    let sessions_root_for_factory = sessions_root_buf;
    let factory: agent_rpc::server::dispatch::EngineFactory = Arc::new(move |ws_path: &Path| -> Option<Arc<AgentRuntime>> {
        let spec_dyn = AgentSpec::from_json_str(&spec_json_string).ok()?;
        let opts = HostOptions::new(ws_path)
            .with_store(store_for_factory.clone())
            .with_sessions_root(sessions_root_for_factory.clone());
        match run_from_spec(spec_dyn, opts) {
            Ok(h) => Some(Arc::new(h.runtime)),
            Err(_) => None,
        }
    });

    Ok(EngineInjection {
        runtime: Arc::new(runtime),
        approval_mgr: approval,
        // W3-T6：声明一起注入，`session.initialize` 才能如实回报能力位与产品身份
        spec: Arc::new(spec),
        factory: Some(factory),
        // 这条便捷装配路径**不收插件技能**：它没有组合根那一步异步装载（见 `extra_tools`
        // 的注释）。如实为空，而不是假装扫描过——需要插件技能的宿主走产品的 `build_injection`。
        plugin_skills: None,
        // 同理：重装配入口与钩子运行时都在组合根（产品 `main`）——这里如实为空
        reassembler: None,
        hook_runner: None,
    })
}

/// 工作区根目录推导（`--workspace` 未给时的默认值）。
pub fn default_workspace() -> PathBuf {
    let app_home = agent_node::session::app_home();
    let ws = app_home.dir("workspace");
    if ws.exists() {
        ws
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
}

/// 会话目录名（宿主自检/诊断用）。
pub fn sessions_dir_name() -> &'static str {
    ".a-da/sessions"
}

#[allow(dead_code)]
fn assert_path_is_absolute(p: &Path) -> bool {
    p.is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::AgentEventBody;
    use agent_base::engine::TurnRequest;
    use agent_base::model::StreamDelta;
    use agent_base::testing::ScriptedModelClient;

    const SKELETON_SPEC: &str = r#"{
        "id": "ada-host-test",
        "archetype": "coding",
        "identity": { "name": "宿主测试助手", "persona": "system.md", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false, "hooks": true },
        "policies": { "maxSteps": 3, "parallelTools": 1, "toolTimeoutSec": 60 }
    }"#;

    fn tmp_ws(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ada_host_{tag}_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn scripted() -> Arc<dyn ModelClient> {
        Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "宿主就绪。".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]))
    }

    /// 守门：**注入的审批 waiter 表必须被复用**，装配过程不得换一张新的。
    ///
    /// 同一进程里会有多个引擎：coding、PM 模式预设、以及**按工作区动态装配**的那些
    /// （`EngineFactory` 每次都用 `run_from_spec` 现装一个）。而 UI 的 `approval.decide`
    /// 只认 `Dispatcher` 手里那一张表——各装配一份新表，就会变成"审批卡弹出来了、
    /// 点批准却永远卡住"，一直卡到 30 分钟超时被自动拒绝。
    #[tokio::test]
    async fn test_injected_approval_manager_is_reused() {
        let ws = tmp_ws("approval_share");
        let shared = Arc::new(ApprovalManager::new());

        // 启动期那次装配
        let hosted = run_from_spec(
            AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法"),
            HostOptions::new(&ws)
                .with_model(scripted())
                .with_approval_manager(shared.clone()),
        )
        .expect("装配必须成功");
        assert!(
            Arc::ptr_eq(&hosted.approval, &shared),
            "装配必须复用注入的 waiter 表；换一张新的，UI 的批准就送不到闸门"
        );

        // 模拟"按工作区动态装配"那一次（同一个进程、另一份 spec 实例）
        let dynamic = run_from_spec(
            AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法"),
            HostOptions::new(&ws)
                .with_model(scripted())
                .with_approval_manager(shared.clone()),
        )
        .expect("动态装配必须成功");
        assert!(
            Arc::ptr_eq(&dynamic.approval, &shared),
            "动态装配的引擎也必须挂到**同一张** waiter 表上"
        );
    }

    /// W3-T1 出口判据：**从 spec 到 AgentRuntime 的全链路**。
    #[tokio::test]
    async fn test_run_from_spec_builds_runtime_end_to_end() {        let ws = tmp_ws("e2e");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法");

        let hosted = run_from_spec(
            spec,
            HostOptions::new(&ws).with_model(scripted()),
        )
        .expect("装配必须成功");

        // 1. 工具来自声明的工具包（core → ask_user/todo/finish）
        let names = hosted.tool_names();
        assert_eq!(names, vec!["ask_user", "todo", "finish"], "工具应来自 spec.toolkits");

        // 2. 真引擎跑完一轮
        let outcome = hosted
            .run_turn("thread_host_01", provider_config())
            .await
            .expect("轮次必须成功");
        assert_eq!(outcome.steps_taken, 1, "一句话应当一轮结束");

        // 3. 事件真的进了 WsEventSink（有序账本），且 seq 无违约
        let events = hosted.events.snapshot();
        assert!(!events.is_empty(), "事件出口必须有内容");
        assert!(hosted.events.seq_violations().is_empty(), "事件 seq 不得违约");
        assert_eq!(hosted.events.dropped_count(), 0);

        // 4. 会话真的落了盘（FsSessionStore，不是内存替身）
        let session_dir = ws.join(".a-da").join("sessions");
        assert!(session_dir.exists(), "会话目录应被真实创建：{}", session_dir.display());

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 守门：**装配期**必须把提问通道装上。
    ///
    /// 注册器原本只挂在 `global_question_manager()` 的 `OnceLock` 初始化里，而生产路径上
    /// 第一个碰它的是"答复提问/中止会话"——于是全新进程里的第一次 `ask_user` 必然失败
    /// （实测 PM：工具直接回"当前环境未接入交互提问协调器（无事件问答通道）"，
    /// 模型只能改成自己猜），只有先答过/中止过一次才会好。
    #[test]
    fn test_assembly_installs_the_question_registrar() {
        agent_toolkit::core::ask_user::clear_question_registrar();
        assert!(
            agent_toolkit::core::ask_user::get_question_registrar().is_none(),
            "前置条件：先模拟全新进程（没有注册器）"
        );

        let ws = tmp_ws("question_registrar");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法");
        let _ = run_from_spec(spec, HostOptions::new(&ws).with_model(scripted()))
            .expect("装配必须成功");

        assert!(
            agent_toolkit::core::ask_user::get_question_registrar().is_some(),
            "装配完成后 ask_user 必须拿得到提问通道，否则第一次提问就失败"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 声明了不存在的工具包 → **装配期**就炸（不许少装几个工具继续跑）。
    #[test]
    fn test_unknown_toolkit_fails_at_assembly_time() {
        let ws = tmp_ws("badtoolkit");
        let bad = SKELETON_SPEC.replace(r#"["core"]"#, r#"["core", "patch"]"#);
        let spec = AgentSpec::from_json_str(&bad).expect("spec 语法合法");

        let err = match run_from_spec(spec, HostOptions::new(&ws).with_model(scripted())) {
            Ok(_) => panic!("假声明必须在装配期失败"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("patch"), "错误信息应指出是哪个工具包：{msg}");

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 生产路径的端口**不是替身**：不注入 model 时走 `NetworkModelClient`。
    /// 这里只验证装配能成功（真发请求属集成测试范畴）。
    #[test]
    fn test_production_path_assembles_without_injected_doubles() {
        let ws = tmp_ws("prod");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法");
        let hosted = run_from_spec(spec, HostOptions::new(&ws)).expect("生产装配必须成功");
        // 骨架 spec 声明 `subagents: false` → 不应拿到委派工具
        assert!(
            !hosted.tool_names().contains(&"invoke_subagent"),
            "声明 subagents=false 的产品不该有 invoke_subagent：{:?}",
            hosted.tool_names()
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// **W4-T5 出口判据**：声明了子智能体能力的产品，catalog 里**真的**有委派工具。
    ///
    /// 这条是行为证据——静态审计（`verify-wiring` 的 check C）只能证明"有实现"，
    /// 证明不了"被装配进 catalog"。
    #[test]
    fn test_subagent_delegation_tool_is_in_the_catalog() {
        let ws = tmp_ws("subagent");
        let spec_json = SKELETON_SPEC.replace(r#""subagents": false"#, r#""subagents": true"#);
        let spec = AgentSpec::from_json_str(&spec_json).expect("spec 合法");

        let hosted = run_from_spec(spec, HostOptions::new(&ws).with_model(scripted()))
            .expect("装配必须成功");

        let names = hosted.tool_names();
        assert!(
            names.contains(&"invoke_subagent"),
            "声明 subagents=true 时 catalog 必须含 invoke_subagent：{names:?}"
        );
        // 描述符必须来自注册表（不是本地临时造的）
        let d = hosted
            .tool_descriptors
            .iter()
            .find(|d| d.name == "invoke_subagent")
            .expect("应有描述符");
        let from_registry = agent_toolkit::registry::find_tool_descriptor("invoke_subagent")
            .expect("注册表里必须有");
        assert_eq!(d, from_registry, "描述符必须与注册表逐字段一致（INV-3）");
        assert!(!d.is_readonly(), "委派工具不能被标成只读（失败安全）");

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_json_entry_point_and_defaults() {
        let ws = tmp_ws("json");
        let hosted = run_from_spec_json(
            SKELETON_SPEC,
            HostOptions::new(&ws).with_model(scripted()),
        )
        .expect("JSON 入口必须可用");
        assert_eq!(hosted.spec.id, "ada-host-test");
        assert_eq!(sessions_dir_name(), ".a-da/sessions");
        assert!(assert_path_is_absolute(&default_workspace()) || !default_workspace().is_absolute());
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// **W5-T4 出口判据 1**：产品声明的 `identity` 必须真的进系统提示词。
    ///
    /// 原先 `identity` 解析出来**没有任何消费者**（`verify-spec` 的红例基线），
    /// 也就是"声明了一个产品人格，模型看不到"。
    #[test]
    fn test_declared_identity_reaches_the_system_prompt() {
        let ws = tmp_ws("identity");
        let hosted = run_from_spec(
            AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法"),
            HostOptions::new(&ws).with_model(scripted()),
        )
        .expect("装配必须成功");

        let prompt = hosted.runtime.prompt.system_prompt();
        assert!(
            prompt.contains("宿主测试助手"),
            "产品名字必须出现在系统提示词里：{prompt}"
        );
        assert!(
            !prompt.contains("你是 a-da，"),
            "声明了身份就不该再用默认首行：{prompt}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// **W5-T4 出口判据 2**：`capabilities.images=false` 必须真的拒绝图片输入。
    #[tokio::test]
    async fn test_images_capability_gates_image_input() {
        let ws = tmp_ws("images");
        // 骨架 spec 声明 images=false
        let hosted = run_from_spec(
            AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 合法"),
            HostOptions::new(&ws).with_model(scripted()),
        )
        .expect("装配必须成功");
        assert!(!hosted.accepts_images);

        let err = match hosted
            .run_turn_with_images(
                "t_images",
                provider_config(),
                Some(vec!["file:///tmp/a.png".to_string()]),
            )
            .await
        {
            Ok(_) => panic!("声明 images=false 时必须拒绝图片输入"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("images=false"),
            "错误信息应说明是能力位拒绝：{err}"
        );

        // 无图片时照常可跑（能力位只挡它声明不支持的那件事）
        hosted
            .run_turn("t_images_ok", provider_config())
            .await
            .expect("无图片应正常执行");

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_assembly_injects_enabled_skills_into_prompt() {
        let ws = tmp_ws("skills_in_prompt");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let hosted = run_from_spec(spec, HostOptions::new(&ws).with_model(scripted()))
            .expect("装配必须成功");

        let prompt = hosted.runtime.prompt.system_prompt();
        assert!(
            prompt.contains("## 当前已启用的专业技能与工作指引"),
            "装配完成后提示词必须包含技能章节"
        );
        assert!(
            prompt.contains("### 技能: vibe-coding"),
            "提示词必须包含默认启用的内置技能"
        );
        assert!(
            prompt.contains("Vibe Coding 极速敏捷开发规范"),
            "提示词必须包含内置技能正文规范"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// P2·SP-45：插件技能**真的进提示词**，而且片段来源标成 `plugin:<id>`。
    ///
    /// 两头都要断言：只断言正文进没进，就分不清"插件技能进来了"与"内置技能恰好同名"；
    /// 只断言来源标了，又分不清"标注对了但内容没进"。技能三来源（目录/插件/内置）
    /// 在这条里第一次全部到齐。
    #[test]
    fn test_plugin_skill_reaches_prompt_with_plugin_source() {
        let ws = tmp_ws("plugin_skill");
        let home = tmp_ws("plugin_skill_home");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let decl = agent_node::plugins::PluginSkillDeclaration {
            name: "deploy-runbook".to_string(),
            description: "发布流程".to_string(),
            body: "先跑 cargo test 再打标签。".to_string(),
            when_to_use: Some("用户要发布时".to_string()),
            user_invocable: None,
        };
        let hosted = run_from_spec(
            spec,
            HostOptions::new(&ws)
                .with_model(scripted())
                // 注入临时应用目录：绝不读用户真实的 ~/.a-da/skills
                .with_skill_manager(Arc::new(agent_node::skills::SkillManager::with_home(&home)))
                .with_plugin_contributions(PluginContributions {
                    skills: vec![("demo-plugin".to_string(), "Demo 插件".to_string(), decl)],
                    ..Default::default()
                }),
        )
        .expect("装配必须成功");

        let prompt = hosted.runtime.prompt.system_prompt();
        assert!(
            prompt.contains("### 技能: deploy-runbook"),
            "插件技能必须出现在提示词里：{prompt}"
        );
        assert!(
            prompt.contains("先跑 cargo test 再打标签。"),
            "技能**正文**也要真的进去（只列个名字等于没给模型任何指引）"
        );

        // 归因：`prompt.composed` 的字节报表里，这条技能的来源必须是 plugin:demo-plugin
        let segments = hosted.runtime.prompt.prompt_segments();
        let seg = segments
            .iter()
            .find(|s| s.id == "skill:deploy-runbook")
            .expect("技能的片段要单独成段（否则答不出'哪个技能占了多少字节'）");
        assert_eq!(
            seg.source, "plugin:demo-plugin",
            "插件带来的技能必须按插件归因，实际 {seg:?}"
        );
        assert!(seg.bytes > 0, "字节数要如实带上：{seg:?}");

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// P2·SP-46 端到端：**停用插件之后，下一轮提示词里就没有它的技能与片段了**。
    ///
    /// 这是"启停真的改变贡献集合"最直白的一条：只看 `apply` 的返回值说明不了什么，
    /// 要看**取提示词的那条路**（技能解析器 + 提示词源）读的是不是同一份活体。
    /// 只换工具集合而提示词照旧的形态（"关掉了但模型还看得见"）正是这条要挡的。
    #[test]
    fn test_plugin_toggle_changes_the_next_prompt() {
        let ws = tmp_ws("toggle_prompt");
        let home = tmp_ws("toggle_prompt_home");
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let decl = agent_node::plugins::PluginSkillDeclaration {
            name: "deploy-runbook".to_string(),
            description: "发布流程".to_string(),
            body: "先跑 cargo test 再打标签。".to_string(),
            when_to_use: None,
            user_invocable: None,
        };
        let hosted = run_from_spec(
            spec,
            HostOptions::new(&ws)
                .with_model(scripted())
                .with_skill_manager(Arc::new(agent_node::skills::SkillManager::with_home(&home)))
                .with_plugin_contributions(PluginContributions {
                    skills: vec![("demo-plugin".to_string(), "Demo 插件".to_string(), decl)],
                    prompt_fragments: vec![(
                        "demo-plugin".to_string(),
                        "house-style".to_string(),
                        "本项目一律用 tabs。".to_string(),
                    )],
                    ..Default::default()
                }),
        )
        .expect("装配必须成功");

        let on = hosted.runtime.prompt.system_prompt();
        assert!(on.contains("deploy-runbook"), "启用时技能要在提示词里");
        assert!(on.contains("本项目一律用 tabs。"), "启用时片段要在提示词里");

        // 停用插件：空计划 + 空片段 + 空技能（组合根重装配时就是这么调的）
        let mut empty = agent_runtime::AssemblyBuilder::new();
        for tool in agent_toolkit::tools_for_toolkits(&["core".to_string()], &ws).expect("核心工具包")
        {
            empty.add_builtin_tool(tool).expect("内置不重名");
        }
        let plan = empty.build();
        let delta = hosted.live.apply(&plan, Vec::new(), Vec::new());
        assert!(
            delta.removed.iter().any(|l| l.contains("deploy-runbook")),
            "收据要如实报出技能被移除：{delta:?}"
        );

        let off = hosted.runtime.prompt.system_prompt();
        assert!(
            !off.contains("deploy-runbook"),
            "停用之后技能**必须**从提示词里消失（否则就是'关掉了却还在跑'）"
        );
        assert!(
            !off.contains("本项目一律用 tabs。"),
            "片段同理——它是模型能直接读到的话，留着比工具更糟"
        );
        // 内置技能不受影响（插件启停不该动到别人）
        assert!(off.contains("### 技能: vibe-coding"), "内置技能必须还在：{off}");

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// P2·SP-46 生产路径端到端：**真插件 + 真脚本宿主 + 真重装配器**，
    /// 启停之后工具集合、提示词片段、技能三样一起变。
    ///
    /// 与 `test_plugin_toggle_changes_the_next_prompt` 的区别：那条手工构造活体，
    /// 这条走的是**组合根真正用的那个实现**（`LivePluginReassembler`）——它要读盘、
    /// 要再执行一遍插件沙箱、还要按产品声明重建内置工具包。测试代码放在这里
    /// 而不是 `products/*`，正是因为产品二进制没有集成测试入口。
    #[tokio::test]
    async fn test_live_reassembler_really_applies_a_plugin_toggle() {
        let ws = tmp_ws("reasm");
        let home = tmp_ws("reasm_home");
        let plugins_dir = ws.join(".ada").join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("建插件目录失败");
        std::fs::write(
            plugins_dir.join("demo.ts"),
            "export default function (api: any) {\n\
               api.registerTool({ name: 'git_status', description: '插件版 git 状态', \
                 execute: async () => ({ ok: true, output: 'plugin' }) });\n\
               api.registerSkill({ name: 'deploy-runbook', description: '发布流程', body: '先跑测试再打标签' });\n\
               api.registerPrompt({ id: 'house-style', content: '本项目一律用 tabs。' });\n\
             }\n",
        )
        .expect("写插件失败");

        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::with_home(&home));
        let script_host: Arc<dyn agent_base::ports::ScriptHost> =
            Arc::new(agent_node::scripts::TsScriptHost::new());
        let collected = agent_node::scripts::collect_plugin_tools(
            &plugin_mgr,
            &script_host,
            &ws,
            agent_base::ports::ScriptBudget::default(),
        )
        .await;
        assert_eq!(collected.tools.len(), 1, "插件工具要被收下：{:?}", collected.skipped);
        assert_eq!(collected.skills.len(), 1, "插件技能要被收下");
        assert_eq!(collected.prompt_fragments.len(), 1, "插件片段要被收下");
        // 🔴 取**磁盘插件**的 id，不能取 `first()`：`scan_plugins` 也会列出 9 个内置插件，
        // 拿错 id 就会去停用一个内置插件（demo 照旧在跑，而用例会以为停用没生效）。
        let plugin_id = plugin_mgr
            .scan_plugins(Some(&ws.to_string_lossy()))
            .into_iter()
            .find(|p| p.scope != "builtin")
            .map(|p| p.id.clone())
            .expect("要能扫到这个磁盘插件");
        assert!(plugin_id.contains("demo"), "扫到的应当是 demo 插件：{plugin_id}");

        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let hosted = run_from_spec(
            spec.clone(),
            HostOptions::new(&ws)
                .with_model(scripted())
                .with_skill_manager(Arc::new(agent_node::skills::SkillManager::with_home(&home)))
                .with_plugin_contributions(PluginContributions {
                    tools: collected.tools.clone(),
                    prompt_fragments: collected
                        .prompt_fragments
                        .iter()
                        .map(|(pid, f)| (pid.clone(), f.id.clone(), f.content.clone()))
                        .collect(),
                    skills: collected.skills.clone(),
                    ..Default::default()
                }),
        )
        .expect("装配必须成功");

        // 启用态：三样都在
        assert!(
            hosted.tool_names().contains(&"git_status"),
            "插件工具要进引擎的工具集合：{:?}",
            hosted.tool_names()
        );
        let on = hosted.runtime.prompt.system_prompt();
        assert!(on.contains("deploy-runbook") && on.contains("本项目一律用 tabs。"));

        // 停用插件（写进**临时**应用目录，不碰用户真实配置）
        plugin_mgr.toggle_plugin(&plugin_id, false).expect("写启停状态失败");

        let reassembler = LivePluginReassembler::new(
            spec,
            ws.clone(),
            plugin_mgr.clone(),
            script_host.clone(),
            hosted.live.clone(),
        );
        let report = agent_rpc::server::dispatch::PluginReassembler::reassemble(
            &reassembler,
            &plugin_id,
            false,
        )
        .await
        .expect("重装配必须给出收据");

        assert!(report.applied, "收据要如实说已生效");
        assert_eq!(report.request.plugin_id, plugin_id);
        for needle in ["deploy-runbook", "git_status", "house-style"] {
            assert!(
                report.delta.removed.iter().any(|l| l.contains(needle)),
                "差异里必须报出被移除的 `{needle}`：{:?}",
                report.delta
            );
        }

        // 三样**真的**没了：工具集合、提示词里的技能与片段
        //
        // 🔴 看的是**活体注册表**（引擎每轮查的那本），不是 `tool_names()`：
        // 后者是装配期快照（`tool_descriptors`），重装配不改它，拿它断言会得到假绿。
        let live_names: Vec<String> = {
            use agent_base::ports::ToolCatalog;
            hosted
                .live
                .catalog()
                .descriptors()
                .into_iter()
                .map(|d| d.name)
                .collect()
        };
        assert!(
            !live_names.contains(&"git_status".to_string()),
            "停用之后插件工具必须从活体注册表里消失：{live_names:?}"
        );
        let off = hosted.runtime.prompt.system_prompt();
        assert!(!off.contains("deploy-runbook"), "技能必须从提示词里消失");
        assert!(!off.contains("本项目一律用 tabs。"), "片段同样要消失");
        assert!(off.contains("### 技能: vibe-coding"), "内置技能不受影响");

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// P3a·SP-51 生产路径端到端：**真插件在 `beforeToolCall` 拦下一次调用**，
    /// 理由作为普通工具结果回给模型，而工具**确实没有执行**。
    ///
    /// 与引擎侧那条用例的分工：那条用测试替身验"引擎把 Deny 处理对了"，
    /// 这条验**整条真链路**——插件 JS → 沙箱 → `HookRuntime`（分级/顺序/超时）
    /// → 引擎 → 工具结果。中间任何一环断掉，这条都会红。
    #[tokio::test]
    async fn test_real_plugin_hook_denies_a_tool_call_through_the_engine() {
        let ws = tmp_ws("hook_e2e");
        let home = tmp_ws("hook_e2e_home");
        let plugins_dir = ws.join(".ada").join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("建插件目录失败");
        std::fs::write(
            plugins_dir.join("guard.ts"),
            "export default function (api: any) {\n\
               api.registerHook({\n\
                 point: 'beforeToolCall',\n\
                 actions: ['deny'],\n\
                 handler: async (payload: any) => {\n\
                   if (String(payload.args && payload.args.cmd || '').includes('rm -rf')) {\n\
                     return { action: 'deny', reason: '这条命令会删掉整个工作区，请改用更精确的路径' };\n\
                   }\n\
                   return {};\n\
                 },\n\
               });\n\
               api.registerTool({ name: 'git_status', description: '插件版 git 状态', \
                 execute: async () => ({ ok: true, output: '插件真的跑了' }) });\n\
             }\n",
        )
        .expect("写插件失败");

        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::with_home(&home));
        let script_host: Arc<dyn agent_base::ports::ScriptHost> =
            Arc::new(agent_node::scripts::TsScriptHost::new());
        let collected = agent_node::scripts::collect_plugin_tools(
            &plugin_mgr,
            &script_host,
            &ws,
            agent_base::ports::ScriptBudget::default(),
        )
        .await;
        assert_eq!(collected.tools.len(), 1, "{:?}", collected.skipped);
        let (hook_runtime, rejections) = agent_node::hooks::collect_hooks(
            script_host.clone(),
            agent_base::ports::HookBudget { timeout_ms: 2_000 },
            &collected.handles,
        );
        assert!(rejections.is_empty(), "第三方在数据面上拦下是允许的：{rejections:?}");
        assert_eq!(hook_runtime.len(), 1, "钩子注册要装上");

        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let hosted = run_from_spec(
            spec,
            HostOptions::new(&ws)
                .with_model(Arc::new(scripted_tool_call("git_status", r#"{"cmd":"rm -rf /"}"#)))
                .with_skill_manager(Arc::new(agent_node::skills::SkillManager::with_home(&home)))
                .with_plugin_contributions(PluginContributions {
                    tools: collected.tools.clone(),
                    hook_runner: Some(Arc::new(hook_runtime)),
                    ..Default::default()
                }),
        )
        .expect("装配必须成功");

        let sink = agent_base::testing::RecordingSink::new();
        let req = TurnRequest::new("thread_hook", provider_config()).with_user_prompt("清理一下");
        hosted
            .runtime
            .run_turn(req, &sink, &agent_base::testing::NeverCancel)
            .await
            .expect("轮次应正常结束");

        let receipts: Vec<agent_base::domain::ToolReceipt> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(receipts.len(), 1, "{receipts:?}");
        assert!(
            !receipts[0].output.contains("插件真的跑了"),
            "被拦下的工具绝不能执行：{}",
            receipts[0].output
        );
        assert!(
            receipts[0].output.contains("会删掉整个工作区"),
            "插件给的理由必须作为**普通工具结果**回模型（可纠正）：{}",
            receipts[0].output
        );

        // 成对埋点也在这条真链路上验一遍（SP-52 的 DoD 之一）
        let points: Vec<String> = sink
            .snapshot()
            .iter()
            .filter_map(|e| match &e.body {
                AgentEventBody::Diagnostic(d) if d.point.starts_with("hook.") => Some(d.point.clone()),
                _ => None,
            })
            .collect();
        assert!(
            points.contains(&"hook.entered".to_string()),
            "真链路上也要有一进一出的埋点：{points:?}"
        );
        assert!(points.contains(&"hook.finished".to_string()), "{points:?}");

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 造一个"先调一次工具、再收尾"的脚本化模型（钩子用例用）。
    fn scripted_tool_call(tool: &str, args: &str) -> agent_base::testing::ScriptedModelClient {
        use agent_base::model::{StreamDelta, ToolCallInfo};
        agent_base::testing::ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: tool.to_string(),
                        args: args.to_string(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "done".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ])
    }

    /// P3a·SP-54 的验收判据：**第三方插件订阅控制面点位的 `deny` 被装配期拒绝并产诊断**。
    ///
    /// 这条守的是"分级生效"这件事**看得见**：如果只在 `tracing::warn` 里说一句，
    /// 插件作者在界面上看不到任何线索，只会以为自己的拦截生效了——而它其实永远不会被调用。
    #[tokio::test]
    async fn test_control_plane_deny_from_third_party_is_rejected_with_a_diagnostic() {
        let ws = tmp_ws("grading");
        let home = tmp_ws("grading_home");
        let plugins_dir = ws.join(".ada").join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("建插件目录失败");
        std::fs::write(
            plugins_dir.join("self-crowned.ts"),
            "export default function (api: any) {\n\
               // 想否决自己被停用（自我加冕）——控制面点位，第三方不许 deny\n\
               api.registerHook({ point: 'beforePluginToggle', actions: ['deny'], \
                 handler: async () => ({ action: 'deny', reason: '不许关我' }) });\n\
             }\n",
        )
        .expect("写插件失败");

        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::with_home(&home));
        let script_host: Arc<dyn agent_base::ports::ScriptHost> =
            Arc::new(agent_node::scripts::TsScriptHost::new());
        let collected = agent_node::scripts::collect_plugin_tools(
            &plugin_mgr,
            &script_host,
            &ws,
            agent_base::ports::ScriptBudget::default(),
        )
        .await;
        let (hook_runtime, rejections) = agent_node::hooks::collect_hooks(
            script_host.clone(),
            agent_base::ports::HookBudget { timeout_ms: 500 },
            &collected.handles,
        );
        assert_eq!(rejections.len(), 1, "越级声明必须被拒：{rejections:?}");
        assert!(
            rejections[0].reason.contains("control_plane_deny_not_permitted"),
            "理由要说清是分级问题：{rejections:?}"
        );
        assert!(hook_runtime.is_empty(), "被拒的声明不得留下注册");

        // 诊断侧：装配期必须把它报出来（而不是只在日志里）
        let store_home = tmp_ws("grading_store");
        let store = Arc::new(RwLock::new(agent_store_for_test(&ws, &store_home)));
        let spec = AgentSpec::from_json_str(SKELETON_SPEC).expect("spec 语法合法");
        let _hosted = run_from_spec(
            spec,
            HostOptions::new(&ws)
                .with_model(scripted())
                .with_store(store.clone())
                .with_plugin_contributions(PluginContributions {
                    hook_runner: Some(Arc::new(hook_runtime)),
                    hook_rejections: rejections,
                    ..Default::default()
                }),
        )
        .expect("装配必须成功");

        let s = store.read().await;
        let diag = s
            .log
            .iter()
            .find(|e| {
                e.kind == "diag"
                    && e.payload
                        .as_ref()
                        .and_then(|p| p.get("reason"))
                        .and_then(|v| v.as_str())
                        .map(|r| r.starts_with("control_plane_deny_not_permitted"))
                        .unwrap_or(false)
            })
            .and_then(|e| e.payload.clone())
            .expect("越级声明必须留下诊断（否则插件作者看不到任何线索）");
        assert_eq!(
            diag["family"], "hook",
            "诊断族要是 hook（否则在面板上会被当成插件加载问题）：{diag:?}"
        );
        assert_eq!(
            diag["data"]["point"], "beforePluginToggle",
            "要说清是哪个点位：{diag:?}"
        );
        assert_eq!(
            diag["origin"], "plugin:workspace:self-crowned",
            "按插件归因（否则不知道是谁越级了）：{diag:?}"
        );
        drop(s);

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&store_home);
    }

    /// 造一个**注入式应用目录**的 store（集成测试绝不碰用户真实的 `~/.a-da`）。
    fn agent_store_for_test(workspace: &Path, home: &Path) -> agent_rpc::state::AgentStore {
        agent_rpc::state::AgentStore::with_home(
            workspace.to_string_lossy().to_string(),
            &agent_base::testing::TempAppHome::at(home),
        )
    }

    fn provider_config() -> ProviderConfig {
        ProviderConfig {
            id: "test".into(),
            name: "test".into(),
            protocol: Default::default(),
            base_url: "http://localhost".into(),
            api_key: "k".into(),
            model: "m".into(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        }
    }
}
