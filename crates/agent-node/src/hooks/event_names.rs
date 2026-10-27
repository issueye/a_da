//! 事件名 ↔ 旧钩子点位映射（迁移期兼容层）。
//!
//! 映射表在 `agent-base::ports::hooks::EVENT_POINTS`（单一真源）；本模块只做薄封装，
//! 供 `HookRuntime::register` 与 `HookEventDispatcher` 使用。
//!
//! 等引擎迁移到 `EventDispatcher` 后整块删除。

#[allow(unused_imports)]
pub use agent_base::ports::hooks::EVENT_POINTS;

pub fn normalize_point(raw: &str) -> String {
    agent_base::ports::hooks::normalize_event_name(raw)
}

/// 旧点位名 → 统一事件名（诊断/收据用；当前尚无生产调用方）。
#[allow(dead_code)]
pub fn event_name_for_point(point: &str) -> Option<&'static str> {
    agent_base::ports::hooks::event_name_for_point(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_both_directions_for_every_point() {
        for (point, event) in EVENT_POINTS {
            assert_eq!(normalize_point(event), *point);
            assert_eq!(normalize_point(point), *point);
            assert_eq!(event_name_for_point(point), Some(*event));
        }
    }

    #[test]
    fn unknown_event_name_is_left_for_the_validator() {
        assert_eq!(normalize_point("event_before_nope"), "event_before_nope");
    }
}
