//! 委派总线端口 `AgentBus`（S3）。
//!
//! # 它解决什么
//!
//! `invoke_subagent` 现在**直接调用** `run_subagent`（进程内、一次性、临时上下文）。
//! 将来网关要做「管理平台 + agent 间交互平台」，同一件事会变成"经网关调用一个远端
//! agent 实例"。如果不先把**语义**抽出来，网关就会变成**第二套委派机制**——
//! 而本仓已经为"两份实现"付过代价（INV-1、`verify-archive` 的第二份引擎断言）。
//!
//! ```text
//! invoke_subagent (Tool) ──► AgentBus 端口 ──┬─ LocalAgentBus（进程内，= 今天的 run_subagent）
//!                                            └─ GatewayAgentBus（S6，远端 agent 实例）
//! ```
//!
//! # 端口为什么只有两个方法
//!
//! 因为**今天真实存在的语义只有这些**：发现有哪些 agent、把任务交给其中一个并等它跑完
//! （可取消）。本仓的规矩是「实现或删声明」——**不许声明做不到的事**（P0-5 的
//! `ask_user` 就是"声明了却没接线"的教训）。
//!
//! 以下方法**刻意留到 S6**，因为一次性、临时上下文的本地实现**没有诚实的实现**：
//!
//! | 方法 | 为什么现在不能定 |
//! |---|---|
//! | `send(agent_id, msg)`（多轮交互） | 本地子智能体上下文**刻意是临时的**（`EphemeralSessionStore`），跑完就没了；写成 `Unsupported` 桩就是 P0-5 那类问题 |
//! | `status(agent_id)`（异步查询） | 本地派活是**同一次 `await` 内**完成的，没有可查询的中间态 |
//! | `cancel(dispatch_id)`（按 id 取消） | 本地取消是**按 dispatch 传 cancel 令牌**（见 [`DispatchRequest::cancel`]）；按 id 取消需要网关那层的派发注册表 |
//!
//! 到了 S6，网关实现会**同时**引入这三件事（远端 agent 可寻址、可多轮、可查询），
//! 那时才把方法加进端口——加方法的同时两个实现都要给出**真实**行为。

use agent_base::ports::{BoxFuture, CancelToken};

/// 一个可委派的 agent（发现结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHandle {
    /// 稳定标识（本地实现 = 子智能体 profile id）
    pub id: String,
    /// 展示名（错误信息里用它，与现状逐字一致）
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// 一次派活请求。
///
/// 借用了调用方的 `cancel`（`ToolContext.cancel`）——它只有轮询式 `is_cancelled()`，
/// 且生命周期短于 `'static`，所以请求是**借用**而非拥有。
pub struct DispatchRequest<'a> {
    /// 目标 agent（本地实现 = 子智能体 profile id）
    pub agent_id: &'a str,
    pub task: &'a str,
    pub additional_context: Option<String>,
    /// 父会话取消令牌：取消必须**真的**传到 agent 内部（W4-T3 的教训）
    pub cancel: Option<&'a dyn CancelToken>,
    /// **委派深度**（S6）。
    ///
    /// 跨网关委派让"嵌套深度"跨过了进程边界——本地的 `NEVER_FOR_SUBAGENT`
    /// 黑名单管不到另一台机器上的 agent。所以深度由请求**显式携带**，
    /// 由网关强制上限（`agent-gateway` 的 `MAX_DELEGATION_DEPTH`）。
    ///
    /// 本地实现不使用它（进程内递归由 `NEVER_FOR_SUBAGENT` 拦），
    /// 但网关实现**必须**如实传出去。
    pub depth: u32,
    /// **续跑的目标线程**（S6 多轮）。
    ///
    /// `None` = 新开一轮（网关实现会建新线程）；`Some(tid)` = 在**已有线程**上接着说，
    /// 目标 agent 保留上一轮的上下文。第一次派活的回执里带 `details.threadId`，
    /// 把它回传即构成多轮。
    ///
    /// **本地实现必须如实拒绝 `Some`**：进程内子智能体是**一次性**的（跑完即散，
    /// 没有可续的线程）。静默忽略等于"看起来支持多轮"。
    pub thread_id: Option<&'a str>,
}

/// 派活结果。
///
/// `details == None` 表示**未进入执行**（校验未通过：agent 不存在 / 已禁用 / 没有可用模型）。
/// 这个区分是必需的：界面与诊断要能分辨"没跑"和"跑了但失败"。
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub ok: bool,
    /// 人可读摘要（成功时是结果摘要，失败时是原因）
    pub summary: String,
    /// 执行失败时的补充原因（未进入执行时为 `None`）
    pub error_message: Option<String>,
    /// 结构化细节（步数 / 耗时 / 工具调用数 / 输出文件）。
    ///
    /// 原样进工具回执的 `details` 字段——**保持与重构前逐字节一致**，
    /// 也是网关实现将来能对上同一份回执形状的原因。
    pub details: Option<serde_json::Value>,
}

impl DispatchOutcome {
    /// 未进入执行（校验未通过）。
    pub fn rejected(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: summary.into(),
            error_message: None,
            details: None,
        }
    }
}

/// 委派总线。
///
/// 两个生产实现：
/// - [`crate::subagents::local_bus::LocalAgentBus`]：进程内、一次性、临时上下文（今天的行为）
/// - [`crate::subagents::gateway_bus::GatewayAgentBus`]：走协议经网关委派给**另一个 agent 节点**（S6）
pub trait AgentBus: Send + Sync + 'static {
    /// 发现：当前可委派的 agent 列表。
    ///
    /// 消费者：工具层据此给出"找不到指定的子智能体配置"这类**精确**错误，
    /// 以及将来的 `list_agents` 工具 / PM agent。
    ///
    /// **S6 起改为异步**：网关实现的列表在**另一台机器/进程**上，必须走网络拿。
    /// （S3 时只有本地实现，同步签名够用；接口随语义变化而变，这正是"实现或删声明"
    /// 的反面——**先有真实需求再改契约**。）
    fn list_agents(&self) -> BoxFuture<'_, Vec<AgentHandle>>;

    /// 派活：把任务交给一个 agent，等它跑完（或取消）。
    ///
    /// 语义约定（两个实现都必须遵守）：
    /// - **不编造**：目标不存在 / 被禁用 / 没有可用模型 → [`DispatchOutcome::rejected`]，绝不"随便挑一个继续跑"；
    /// - **取消穿透**：`req.cancel` 触发后必须真的传到 agent 内部（本地 = 子智能体引擎 → 进程树；网关 = `thread.abort` 打到目标）；
    /// - **如实回执**：`ok` / `summary` / `details` 必须反映真实执行结果。
    fn dispatch<'a>(&'a self, req: DispatchRequest<'a>) -> BoxFuture<'a, DispatchOutcome>;
}
