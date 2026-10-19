use boa_engine::Context;
use boa_engine::error::EngineError;
/// 沙箱运行限额（P1 · SP-12）。
///
/// # 为什么必须有
///
/// Boa 的循环迭代上限默认是 `u64::MAX`（等于**不限**），而沙箱的 `terminate` 只是往事件循环
/// 发一条消息——**同步死循环期间它根本排不上队**（消息要等当前执行返回才被处理）。
/// 结果是 `while(true){}` 把 worker 线程占死：宿主只能超时后放弃等待，那条线程再也回不来。
///
/// 有了进程内的指令上限，同步段**必然返回**（Boa 抛 `RuntimeLimitError`），超时才能变成
/// 一条可纠正的回执，而不是"线程卡住"。
///
/// 默认值取"**宽但有限**"：正常脚本碰不到，死循环必然撞上（见 [`Limits::default`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 循环迭代上限；`0` 表示不限（与 Boa 的 `u64::MAX` 同义）。
    pub loop_iterations: u64,
    /// 函数递归深度上限。
    pub recursion_depth: usize,
    /// VM 栈上限。
    pub stack_size: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 宽但**有限**：一千万次迭代在 Boa 里约等于数百毫秒到数秒——
            // 正常脚本碰不到，纯死循环必然撞上。
            loop_iterations: 10_000_000,
            // 递归与栈沿用 Boa 的默认值（这两项它本来就有上限，不用我们收紧）
            recursion_depth: 512,
            stack_size: 1024 * 10,
        }
    }
}

impl Limits {
    /// 完全不限（只给"我知道自己在干什么"的场景；默认**不用**）。
    pub fn unlimited() -> Self {
        Self {
            loop_iterations: 0,
            recursion_depth: 512,
            stack_size: 1024 * 10,
        }
    }

    /// 相对默认更紧的限额（测试与将来的紧凑预算用）。
    pub fn tight(loop_iterations: u64) -> Self {
        Self {
            loop_iterations,
            ..Self::default()
        }
    }
}

/// 预算超限错误的**统一标记**。
///
/// 宿主靠它把错误映射成 `Timeout` 回执，**不必去猜 Boa 的文案**（升级 Boa 时文案会变，
/// 标记不会）。`crates/agent-node` 的脚本宿主读它，`plugin-sdk` 的契约也以它为准。
pub const LIMIT_MARKER: &str = "[script-limit]";

/// 错误串是否表示"撞上了沙箱预算"。
pub fn is_limit_error(err: &str) -> bool {
    err.contains(LIMIT_MARKER)
}

/// 把 [`Limits`] 应用到 Boa 的 `Context` 上（唯一一处，避免限额散落）。
pub(super) fn apply_limits(ctx: &mut Context, limits: Limits) {
    let l = ctx.runtime_limits_mut();
    if limits.loop_iterations == 0 {
        l.disable_loop_iteration_limit();
    } else {
        l.set_loop_iteration_limit(limits.loop_iterations);
    }
    l.set_recursion_limit(limits.recursion_depth);
    l.set_stack_size_limit(limits.stack_size);
}

/// 结构化识别"是不是撞了限额"：Boa 的 `EngineError::RuntimeLimit` 是引擎级错误，
/// 可以从 `JsError` 直接取出来（不用匹配文案）。
pub(super) fn limit_kind(err: &boa_engine::JsError) -> Option<&'static str> {
    match err.as_engine() {
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::LoopIteration)) => {
            Some("loop_iterations")
        }
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::Recursion)) => {
            Some("recursion_depth")
        }
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::StackSize)) => {
            Some("stack_size")
        }
        _ => None,
    }
}
