//! 工具端口：`Tool` + `ToolCatalog`。
//!
//! 这两个 trait 是 AGENTS.md §1/§2/§4/§10 那批"多处同时登记、漏一处就静默失效"的直接解药：
//! **能力由 [`ToolDescriptor`] 声明，注册表是唯一真源**，`validate()` 把
//! "声明了没人实现 / 实现了没人声明"变成返回值而不是靠人记。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use crate::ports::{CancelToken, EventSink, Scope};

/// 装箱 future（与 `agent_core::runner::executor` 的写法一致：不用宏也能做对象安全的异步 trait）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 一次工具执行的上下文。**这里是把"隐含依赖"变显式的地方**：
/// 现状里 checkpoint / question / abort / event 都是散在 executor 分支里的 `Option` 参数。
pub struct ToolContext<'a> {
    pub scope: &'a dyn Scope,
    pub cancel: &'a dyn CancelToken,
    pub events: &'a dyn EventSink,
    pub thread_id: &'a str,
}

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;

    /// 执行一次调用。**返回 [`ToolReceipt`] 而不是字符串**（INV-5）。
    fn execute<'a>(&'a self, call: &'a ToolCall, ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt>;
}

/// 工具消费侧：注册表要保证**每一个工具在每一侧都被正确分类**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// 只读过滤（plan/只读子智能体拿不拿得到）
    ReadonlyFilter,
    /// 审批策略
    ApprovalPolicy,
    /// 回滚/检查点策略
    RollbackPolicy,
    /// 子智能体白名单
    SubagentAllowlist,
    /// 插件声明
    PluginDeclaration,
}

impl Consumer {
    pub const ALL: [Consumer; 5] = [
        Consumer::ReadonlyFilter,
        Consumer::ApprovalPolicy,
        Consumer::RollbackPolicy,
        Consumer::SubagentAllowlist,
        Consumer::PluginDeclaration,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractViolation {
    pub consumer: Consumer,
    pub tool: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// 未注册的工具名（**必须是错误**，不许静默返回空结果）
    Unknown(String),
    /// 已注册但该环境不支持（例如此产品没装这个工具包）
    Unsupported(String),
    Failed(String),
}

pub trait ToolCatalog: Send + Sync {
    /// 当前作用域下模型可见的工具表。**组合根装配一次**，不再每次调用扫盘。
    fn descriptors(&self) -> Vec<ToolDescriptor>;

    fn resolve(&self, name: &str) -> Result<Arc<dyn Tool>, ToolError>;

    /// 注册表完整性：声明 ↔ 实现双向核对，返回所有违约项（空 = 全绿）。
    fn validate(&self, consumers: &[Consumer]) -> Vec<ContractViolation>;
}
