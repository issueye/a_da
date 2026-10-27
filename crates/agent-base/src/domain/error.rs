//! 错误与拒绝原因：拒绝的来路必须是一等值，不是文档里的一句话。
//!
//! AGENTS.md §2（`isWriteTool` 失败安全）、§9（绝不捏造确定性）的共同点是
//! "拿不到判定依据时不得假装成功"。这里保留结构化的拒绝原因，供调用方与界面分辨来路。

use thiserror::Error;

/// 一次拒绝的原因。界面/日志要说人话，所以这里带上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenialKind {
    /// 门禁/策略拒绝
    Gate { reason: String },
    /// 能力未实现（INV-2：不支持要明说，不许静默路过）
    Unsupported { what: String },
}

impl std::fmt::Display for DenialKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gate { reason } => write!(f, "策略拒绝：{reason}"),
            Self::Unsupported { what } => write!(f, "该能力未实现：{what}"),
        }
    }
}

impl std::error::Error for DenialKind {}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("模型调用失败：{0}")]
    Model(String),
    #[error("工具执行失败：{0}")]
    Tool(String),
    #[error("会话存储失败：{0}")]
    Store(String),
    #[error("{0}")]
    Denied(#[from] DenialKind),
    #[error("已取消")]
    Cancelled,
    #[error("{what} 超时（{after_ms}ms）")]
    Timeout { what: String, after_ms: u64 },
    /// INV-2：不支持必须是显式错误，不许用默认实现假装成功
    #[error("端口 {port} 不支持能力 {capability}")]
    Unsupported { port: &'static str, capability: &'static str },
    #[error("内部错误：{0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_an_error_not_a_silent_noop() {
        let e = AgentError::Unsupported { port: "HookRunner", capability: "beforeTurn" };
        assert!(e.to_string().contains("HookRunner"));
    }
}
