//! 运行策略（INV-4、INV-8）：定义轮次内的预算和并发控制。

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RunPolicy {
    /// 轮内最大迭代步数。若达到此限制且模型仍未结束，报告 `TurnStopReason::BudgetExhausted`。
    /// None 表示不设人为限制（完全由模型决策、完成或取消决定退出）。
    pub max_steps: Option<u32>,
    /// 最大并行执行工具数（默认为 1，即串行；未来扩展并发安全工具执行）。
    pub max_parallel_tools: usize,
    /// 单个工具执行超时时间（可选）。
    pub tool_timeout: Option<Duration>,
}

impl Default for RunPolicy {
    fn default() -> Self {
        Self {
            max_steps: None,
            max_parallel_tools: 1,
            tool_timeout: None,
        }
    }
}
