//! 系统时钟：基础设施层读取"当前 Unix 毫秒"的**唯一**实现。
//!
//! 为什么在这里而不是 `agent-base`：内核（`agent-base`）明确禁止直接读系统时间
//! （见其 `lib.rs` 的零 IO 约束——时间只能经 [`agent_base::ports::Clock`] 由适配器注入）。
//! 但工具、会话落盘、子智能体这些基础设施代码**本来就必须**读真实时钟，
//! 此前它们各自抄了一份 5 行 `SystemTime::now()`（曾达 10 份逐字副本），
//! 于是"时间语义"散落在各处，改一处口径要改十处。
//!
//! 本函数放在工具包（依赖链最下层：`agent-base` ← `agent-toolkit` ← `agent-adapter` ← `agent-node`），
//! 让这三层共用一处；网关因为"只依赖 `agent-proto`"的硬边界，另有一份同类实现
//! （`agent_gateway::supervisor::now_ms`），那属于**受控**的重复。

/// 当前 Unix 毫秒时间戳。
///
/// 系统时钟早于 `UNIX_EPOCH`（时钟被回拨到 1970 前）时返回 0——与历史上各份手写实现一致，
/// 保持行为不变比"返回错误"更重要：调用方都把时间戳当单调标识用。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
