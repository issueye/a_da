//! 事件出口：引擎产出的事件**只有这一条出口**（INV-6）。
//!
//! 现状是 `tokio::mpsc::Sender<AgentLoopEvent>` 直接长在 `run_agent_loop` 的签名里，
//! 且 `dispatch.rs` 用手写 match 消费两份；换成端口后，传输、录制、测试替身各实现各的。

use crate::domain::{AgentEvent, EventOutcome, PluginEvent};
use crate::ports::hooks::HookObservation;
use crate::ports::BoxFuture;

pub trait EventSink: Send + Sync {
    /// 同步入队；**顺序由引擎保证**（同一 `thread_id` 的 `seq` 单调递增）。
    fn emit(&self, event: AgentEvent);
}

/// 插件事件分发端口（`event_*` 命名空间）。
///
/// 与 [`EventSink`] 的分工：
/// - `EventSink` 是引擎 → 宿主/UI 的**领域事件唯一出口**；
/// - `EventDispatcher` 是引擎 → 插件系统的**拦截/通知入口**，实现放在 `agent-node`。
///
/// `publish` 只用于 `Notify` / `Stream`；`dispatch` 用于 `Intercept` / `Decorate`，
/// 返回值决定引擎是否继续、替换载荷或追加旁注。端口没有默认实现（INV-2）。
pub trait EventDispatcher: Send + Sync {
    /// 纯通知：不等待插件处理结果。
    fn publish(&self, event: PluginEvent);

    /// 需要结果的事件：异步执行处理器，引擎等待 `EventOutcome`。
    ///
    /// `on_observation` 用于把插件处理器的逐条观测（进入/结束/超时）交回引擎，
    /// 由引擎转换成带 `seq` 的诊断事件——分发器不伪造事件信封。
    fn dispatch<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, EventOutcome>;

    /// 需要**多个插件结果**的事件（事后 Decorate）：返回全部 note/observe，
    /// 由引擎统一追加到工具结果/诊断面。`Intercept` 仍用 [`Self::dispatch`]。
    fn dispatch_many<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, Vec<EventOutcome>>;
}
