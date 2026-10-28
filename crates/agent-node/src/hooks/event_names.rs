//! 事件名归一化（迁移收尾）。
//!
//! 映射表在 `agent-base::ports::hooks::EVENT_POINTS`（单一真源）；本模块只做薄封装。
//! 插件注册必须给 `event_*`；内部旧点位名会被归一成事件名，供引擎/管理面调用。

#[allow(unused_imports)]
pub use agent_base::ports::hooks::EVENT_POINTS;

pub fn normalize_event(raw: &str) -> String {
    agent_base::ports::hooks::normalize_event_name(raw)
}

pub fn is_known_event(raw: &str) -> bool {
    agent_base::ports::hooks::is_known_event(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_internal_points_to_event_names() {
        for (point, event) in EVENT_POINTS {
            assert_eq!(normalize_event(point), *event);
            assert_eq!(normalize_event(event), *event);
            assert!(is_known_event(event));
            assert!(!is_known_event(point));
        }
    }

    #[test]
    fn unknown_event_name_is_left_for_the_validator() {
        assert_eq!(normalize_event("event_before_nope"), "event_before_nope");
        assert!(!is_known_event("event_before_nope"));
    }
}
