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
    /// **工具内部点位**的钩子入口（保留给未来插件工具）。
    ///
    /// `None` = 这个产品没开钩子（或这次调用没经过引擎，例如测试直接构造上下文）。
    /// 2026-10-27：`todo` 的 `beforeTodoUpdate` / `afterTodoUpdate` 已移除，核心工具
    /// 不再使用这条入口；当前没有内置工具需要它。
    pub hooks: Option<crate::ports::hooks::ToolHooks<'a>>,
}

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;

    /// 执行一次调用。**返回 [`ToolReceipt`] 而不是字符串**（INV-5）。
    fn execute<'a>(&'a self, call: &'a ToolCall, ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt>;

    /// 这个工具是否会**合法地**长时间运行 / 阻塞等待外部输入？
    ///
    /// 引擎的策略超时（[`crate::engine::RunPolicy::tool_timeout`]）对 `true` 的工具**不生效**：
    /// 提问要等用户（上限 30 分钟），派活要等子智能体跑完，命令执行自带超时
    /// （最长 10 分钟）——拿策略超时（120 秒）去掐它们，只会把"正常的长任务/等用户点一下"
    /// 变成一次假失败。
    ///
    /// 默认 `false` = 受策略超时约束。**哪些工具该是 `true` 由注册表钉死**
    /// （`agent_toolkit::registry::LONG_RUNNING_TOOLS` + 契约测试），这里只是各自的声明口。
    fn long_running(&self) -> bool {
        false
    }
}

/// 工具消费侧：注册表要保证**每一个工具在每一侧都被正确分类**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// 只读过滤（plan/只读子智能体拿不拿得到）
    ReadonlyFilter,
    /// 回滚/检查点策略
    RollbackPolicy,
    /// 子智能体白名单
    SubagentAllowlist,
    /// 插件声明
    PluginDeclaration,
}

impl Consumer {
    pub const ALL: [Consumer; 4] = [
        Consumer::ReadonlyFilter,
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
