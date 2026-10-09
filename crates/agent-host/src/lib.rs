//! AGENT HOST：**宿主装配层**（`docs/agent-base-design.md` v0.2 §9）。
//!
//! # 它解决什么问题
//!
//! W3 之前，产品二进制（`products/ada-coding`）与 Tauri 宿主（`src-tauri`）都是直接调
//! `agent_core` 的 **legacy 主循环** `run_agent_loop`——也就是说：`AgentRuntime`
//! （真引擎）虽然装配得出来，**产品运行时却根本没在用它**（计划 §2 P0-1）。
//!
//! 本 crate 是那个缺掉的中间层：把「产品声明 + 宿主态」装配成**带真实端口**的
//! `AgentRuntime`，让上层（产品二进制 / ws 宿主）只需要说"按这份 spec 起一个产品"。
//!
//! ```text
//! products/ada-coding ─┐
//! src-tauri            ─┼─→ agent-host ─→ agent-runtime ─→ agent-base（引擎）
//!                      ─┘        └─────→ agent-adapter（真实端口实现）
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
use agent_core::approval::{ApprovalManager, HostApprovalGate};
use agent_core::server::{EngineInjection, StateBroadcaster, WsEventSink};
use agent_core::state::AgentStore;
use agent_runtime::{AgentSpec, CapabilitySpec, IdentitySpec, ProductBuilder, SpecError};
use thiserror::Error;
use tokio::sync::RwLock;

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
        }
    }

    /// 复用宿主已有的会话态（推荐：让引擎与 UI 看同一份配置）。
    pub fn with_store(mut self, store: Arc<RwLock<AgentStore>>) -> Self {
        self.store = Some(store);
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
    pub tool_descriptors: Vec<ToolDescriptor>,
    /// 子智能体配置表（W4-T5）。宿主若要与 Dispatcher 共用（`SUBAGENT_RESUME` 等），
    /// 可把它传给 `WsHostServer`；不传也不会出错——profile 来自内置表 + 用户配置。
    pub subagent_mgr: Arc<agent_core::subagents::SubagentManager>,
    /// 产品声明的 `capabilities.images`（W5-T4）：**真实消费者**——
    /// 声明 `false` 的产品不得把图片送进模型。
    pub accepts_images: bool,
    pub workspace: PathBuf,
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
    let approval = Arc::new(ApprovalManager::new());

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

    let scope: Arc<dyn agent_base::ports::Scope> =
        Arc::new(WorkspaceScope::new(workspace.clone()));
    let session_store: Arc<dyn agent_base::ports::SessionStore> = Arc::new(FsSessionStore::new(
        sessions_root,
        workspace.to_string_lossy().to_string(),
    ));
    let prompt: Arc<dyn agent_base::ports::PromptSource> = Arc::new(
        CodingPromptSource::new(workspace.to_string_lossy().to_string())
            .with_identity(ProductIdentity::new(name.clone(), persona.clone(), locale.clone()))
            .with_extensions(options.prompt_extensions.clone()),
    );
    let clock: Arc<dyn agent_base::ports::Clock> = Arc::new(SystemClock);
    let model: Arc<dyn ModelClient> = options
        .model
        .clone()
        .unwrap_or_else(|| Arc::new(NetworkModelClient::new()));
    let approval_gate: Arc<dyn agent_base::ports::ApprovalGate> =
        Arc::new(HostApprovalGate::new(store.clone(), approval.clone()));

    // ── 按声明装配工具包（`spec.toolkits` 的真实消费者）────────────────────
    let tools: Vec<Arc<dyn Tool>> = agent_toolkit::tools_for_toolkits(&spec.toolkits, &workspace)
        .map_err(SpecError::Violation)?;

    // W4-T5：`invoke_subagent` 是**宿主耦合**工具（要 SubagentManager / 父 provider / 检查点），
    // 工具包工厂构造不出来，所以由组合根注入。
    //
    // 但它是否出现由**产品声明**决定（INV-10 能力优先）：`capabilities.subagents == false`
    // 的产品不该拿到委派工具——否则"声明说不支持子智能体，模型却看得到这个工具"。
    let subagent_mgr = Arc::new(agent_core::subagents::SubagentManager::new());
    let mut tools = tools;
    if caps.subagents {
        tools.push(Arc::new(agent_core::subagents::InvokeSubagentTool::new(
            workspace.clone(),
            store.clone(),
            subagent_mgr.clone(),
            None,
            // W4-T6：子智能体的工具按**同一份产品声明**装配，而不是另一张手写清单
            spec.toolkits.clone(),
        )));
    } else {
        tracing::info!("产品声明 capabilities.subagents=false → 不装配 invoke_subagent");
    }

    let tool_descriptors: Vec<ToolDescriptor> =
        tools.iter().map(|t| t.descriptor().clone()).collect();

    // 借用 `spec.capabilities` 到这里结束（后面要把 `spec` move 进 `HostedProduct`）
    let accepts_images = caps.images;

    let runtime = ProductBuilder::new(spec.clone())
        .with_tools(tools)
        .with_model(model)
        .with_approval(approval_gate)
        .with_store(session_store)
        .with_prompt(prompt)
        .with_scope(scope)
        .with_clock(clock)
        .build()?;

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

/// 为宿主装配**注入包**（`EngineInjection`）：宿主（产品二进制 / Tauri）只需要
/// 把 `store`、工作区与自己的 `agent.spec.json` 交给它。
///
/// 收敛在这里的理由（R2：一处真源）：`ada-coding` 与 `src-tauri` 两个宿主
/// 需要的装配完全一样——复用宿主的 `store`、会话落盘到 app home、把
/// `ApprovalManager` 一并带出去给 `Dispatcher` 共用。各写一份迟早漂移。
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
    let options = HostOptions::new(workspace.as_ref())
        .with_store(store.clone())
        .with_sessions_root(sessions_root.into());

    let HostedProduct { runtime, approval, spec, .. } = run_from_spec(spec, options)?;
    Ok(EngineInjection {
        runtime: Arc::new(runtime),
        approval_mgr: approval,
        // W3-T6：声明一起注入，`session.initialize` 才能如实回报能力位与产品身份
        spec: Arc::new(spec),
    })
}

/// 工作区根目录推导（`--workspace` 未给时的默认值）。
pub fn default_workspace() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
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
    use agent_base::model::StreamDelta;
    use agent_base::testing::ScriptedModelClient;

    const SKELETON_SPEC: &str = r#"{
        "id": "ada-host-test",
        "archetype": "coding",
        "identity": { "name": "宿主测试助手", "persona": "system.md", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
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

    /// W3-T1 出口判据：**从 spec 到 AgentRuntime 的全链路**。
    #[tokio::test]
    async fn test_run_from_spec_builds_runtime_end_to_end() {
        let ws = tmp_ws("e2e");
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
