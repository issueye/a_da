//! `CancelToken` 端口契约合规断言（INV-2 / INV-8）。
//!
//! 端口只有一个方法 `is_cancelled()`，能**通用**断言的性质只有两条：
//! 1. **幂等**：同一时刻连续询问必须给同一答案（不允许"每次问都换答案"的实现）；
//! 2. **状态如实**：调用方明确知道当前应处于的状态时，实现必须与之相符
//!    （这条同时排除"恒 true"与"恒 false"两种假实现）。
//!
//! "取消能穿透到子进程/子智能体"属于**跨端口行为**，在
//! [`crate::invariants::inv8_cancellation_penetration`] 里断言，不在这里重复。

use agent_base::ports::CancelToken;

/// 验证取消令牌契约。
///
/// - `token`：待验证的令牌；
/// - `expect_cancelled`：调用方**已确定**该令牌此刻应处于的状态
///   （例如刚刚 `cancel()` 过就是 `true`，从未取消就是 `false`）。
pub fn verify_cancel_contract<C: CancelToken + ?Sized>(
    token: &C,
    expect_cancelled: bool,
) -> Result<(), String> {
    let first = token.is_cancelled();
    let second = token.is_cancelled();

    if first != second {
        return Err(format!(
            "is_cancelled() 必须幂等：连续两次询问给出不同答案（{first} → {second}）"
        ));
    }
    if first != expect_cancelled {
        return Err(format!(
            "令牌状态与调用方预期不符：预期 {expect_cancelled}，实际 {first}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_adapter::cancel::CancelHandle;
    use agent_base::testing::{ManualCancel, NeverCancel};

    #[test]
    fn test_never_cancel_conforms() {
        verify_cancel_contract(&NeverCancel, false).expect("永不禁用令牌必须合规");
    }

    #[test]
    fn test_manual_cancel_conforms_before_and_after() {
        let token = ManualCancel::new();
        verify_cancel_contract(&token, false).expect("未取消时必须合规");
        token.cancel();
        verify_cancel_contract(&token, true).expect("已取消时必须合规");
    }

    /// 真实实现（不是 double）也要过同一份契约。
    #[test]
    fn test_real_cancel_handle_conforms() {
        let token = CancelHandle::new();
        verify_cancel_contract(&token, false).expect("真实令牌（未取消）必须合规");
        token.cancel();
        verify_cancel_contract(&token, true).expect("真实令牌（已取消）必须合规");
    }

    /// 子令牌同样受契约约束，且"父已取消 → 子报告已取消"。
    #[test]
    fn test_child_cancel_conforms() {
        let parent = CancelHandle::new();
        let child = parent.child();
        verify_cancel_contract(&child, false).expect("父未取消时子必须合规");
        parent.cancel();
        verify_cancel_contract(&child, true).expect("父取消后子必须合规（已取消）");
    }

    /// 反向假实现必须被契约挡住（守住"断言不是空转"）。
    #[test]
    fn test_contract_rejects_lying_implementation() {
        struct AlwaysCancelled;
        impl CancelToken for AlwaysCancelled {
            fn is_cancelled(&self) -> bool {
                true
            }
        }
        struct NeverCancelled;
        impl CancelToken for NeverCancelled {
            fn is_cancelled(&self) -> bool {
                false
            }
        }

        assert!(
            verify_cancel_contract(&AlwaysCancelled, false).is_err(),
            "恒 true 的实现必须被契约判为不合规"
        );
        assert!(
            verify_cancel_contract(&NeverCancelled, true).is_err(),
            "恒 false 的实现必须被契约判为不合规"
        );
    }
}
