//! INV-4: 失败方向在类型里不变量
//! 失败策略默认采用 FailDirection::Closed（失败时阻断而非放行）。

use agent_base::domain::FailDirection;

/// 断言系统的默认失败方向为 Closed 失败闭合
pub fn assert_default_fail_direction_is_closed() -> Result<(), String> {
    let def = FailDirection::default();
    if def != FailDirection::Closed {
        return Err("基座 FailDirection 默认值必须是 Closed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inv4_fail_direction() {
        assert_default_fail_direction_is_closed().expect("默认失败方向断言必须通过");
    }
}
