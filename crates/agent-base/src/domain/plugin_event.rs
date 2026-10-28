//! 插件事件的统一契约（`event_*` 命名空间）。
//!
//! 这是 [PLAN-unified-events.md](../../docs/architecture/PLAN-unified-events.md) 的
//! 第一层：**只定义类型，不实现总线**。事件总线在插件系统里（`agent-node`），
//! 引擎只通过端口发布/等待结果。
//!
//! 关键语义：
//! - `Intercept` / `Decorate` 事件由引擎**等待**结果；
//! - `Notify` / `Stream` 是 fire-and-forget，不阻塞轮次；
//! - 事件名统一 `event_` 前缀 + snake_case；旧钩子名作为别名在插件系统内映射。

use serde::{Deserialize, Serialize};

use crate::domain::TraceCtx;

/// 事件来源（用于归因与诊断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventSource {
    Engine,
    Plugin,
    Host,
}

/// 事件的投递效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventEffect {
    /// 事前决策：异步执行处理器，引擎等待 `EventOutcome`。
    Intercept,
    /// 事后装饰：异步执行，引擎短等待以应用旁注。
    Decorate,
    /// 纯通知：异步队列，不阻塞引擎。
    Notify,
    /// 高频增量：独立 lane，可合并/丢弃。
    Stream,
}

/// 事件所属的投递 lane（与 [`EventEffect`] 对应，但显式写出便于总线调度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventLane {
    Critical,
    Decorate,
    Notify,
    Stream,
}

impl EventLane {
    pub fn of(effect: EventEffect) -> Self {
        match effect {
            EventEffect::Intercept => Self::Critical,
            EventEffect::Decorate => Self::Decorate,
            EventEffect::Notify => Self::Notify,
            EventEffect::Stream => Self::Stream,
        }
    }
}

/// 没有拿到结果时的失败方向。
///
/// 当前安全/审批未启用，默认 `Pass`；后续安全插件可以把关键事件声明为 `Deny`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventFailure {
    Pass,
    Deny,
}

/// 一次事件的声明（事件目录的条目）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSpec {
    pub name: String,
    pub effect: EventEffect,
    /// 允许返回的动作；`Intercept` 至少需要 `deny` 或 `replace` 之一。
    pub allowed_actions: Vec<EventAction>,
    pub timeout_ms: u64,
    pub failure: EventFailure,
    pub lane: EventLane,
}

/// 处理器的返回值。`Pass` 是默认值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum EventOutcome {
    Pass,
    Deny {
        #[serde(default)]
        reason: String,
    },
    Replace {
        payload: serde_json::Value,
    },
    Note {
        #[serde(default)]
        text: String,
    },
    /// 只进诊断面，不进模型上下文（对应旧 `AfterNote::Observe`）。
    Observe {
        #[serde(default)]
        data: serde_json::Value,
    },
    #[serde(rename = "abort")]
    AbortTurn {
        #[serde(default)]
        reason: String,
    },
}

impl Default for EventOutcome {
    fn default() -> Self {
        Self::Pass
    }
}

/// 事件处理器允许返回的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventAction {
    Pass,
    Deny,
    Replace,
    Note,
    Observe,
    #[serde(rename = "abort")]
    AbortTurn,
}

/// 一次插件事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEvent {
    /// `event_before_tool_call` / `event_turn_finished` / ...
    pub name: String,
    /// 与领域事件同源；插件事件不另造序号。
    pub seq: u64,
    pub at_ms: i64,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default)]
    pub trace: TraceCtx,
    pub source: EventSource,
    /// 投影后的 DTO；敏感字段在进入总线前过滤。
    pub payload: serde_json::Value,
}

impl PluginEvent {
    pub fn new(
        name: impl Into<String>,
        seq: u64,
        at_ms: i64,
        thread_id: impl Into<String>,
        source: EventSource,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            seq,
            at_ms,
            thread_id: thread_id.into(),
            turn_id: None,
            call_id: None,
            trace: TraceCtx::none(),
            source,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_lane_follows_effect() {
        assert_eq!(EventLane::of(EventEffect::Intercept), EventLane::Critical);
        assert_eq!(EventLane::of(EventEffect::Decorate), EventLane::Decorate);
        assert_eq!(EventLane::of(EventEffect::Notify), EventLane::Notify);
        assert_eq!(EventLane::of(EventEffect::Stream), EventLane::Stream);
    }

    #[test]
    fn outcome_serializes_with_action_tag() {
        let deny = serde_json::to_value(EventOutcome::Deny {
            reason: "no".to_string(),
        })
        .unwrap();
        assert_eq!(deny["action"], "deny");
        assert_eq!(deny["reason"], "no");
        assert_eq!(EventOutcome::default(), EventOutcome::Pass);
    }
}
