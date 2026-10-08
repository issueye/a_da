//! 审批端口：**策略**与**执行**分开（AGENTS.md §14），且判定结果如实标明是谁答的。
//!
//! 现状（计划 §1.3）：`should_ask_approval` 只在测试里被调用、`ApprovalManager::register_waiter`
//! 唯一调用点在 `#[cfg(test)]` 里、`config.approval` 只写不读——整条闸门在权威路径上**没有接线**。
//! 端口化之后，"引擎必须调用它"成为可断言的契约（合规套件：每次受约束调用前恰好调用一次）。

use crate::domain::{DenialKind, FailDirection};
use crate::ports::tools::BoxFuture;
use crate::ports::CancelToken;

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub thread_id: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub is_write: bool,
    /// 当前审批档位（`auto` / `ask` / `readonly`）；语义由实现解释
    pub mode: String,
    /// 为什么要问（命中哪条危险模式等）
    pub reason: Option<String>,
}

/// **谁**给出了这个结果。拒绝有三条不同的来路，事后审计要能分辨（AGENTS.md §14）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredBy {
    User,
    Policy,
    Timeout,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOutcome {
    pub approved: bool,
    pub by: AnsweredBy,
    pub reason: Option<String>,
}

impl ApprovalOutcome {
    pub fn allowed(by: AnsweredBy) -> Self {
        Self { approved: true, by, reason: None }
    }

    pub fn denied(by: AnsweredBy, reason: impl Into<String>) -> Self {
        Self { approved: false, by, reason: Some(reason.into()) }
    }

    /// 被拒时转成领域拒绝原因（调用方把它作为**工具结果**回给模型，而不是抛错，见 §14）。
    pub fn denial(&self, tool: &str) -> Option<DenialKind> {
        if self.approved {
            return None;
        }
        Some(DenialKind::Approval { tool: tool.to_string(), reason: self.reason.clone() })
    }
}

pub trait ApprovalGate: Send + Sync {
    /// 拿不到判定依据时往哪边倒（INV-4）。默认实现是 `Closed`，但**必须是显式取值**。
    fn direction(&self) -> FailDirection;

    /// 判断一次调用是否需要人工确认；需要则挂起等待，不需要则直接返回 Policy/User 允许。
    fn decide<'a>(
        &'a self,
        req: ApprovalRequest,
        cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, ApprovalOutcome>;
}
