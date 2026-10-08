//! 模型端口：把"哪家协议、怎么鉴权、怎么重试"关在适配器里。
//!
//! 现状：`agent_core::ai::stream::stream_model_chat` 直接决定三家协议的分支与 URL，
//! 调用方还必须自己拼 `ModelChatOptions`；`max_retries` 更是"写了没人读"。
//! 端口化之后，`capabilities()` 让"声明支持什么"和"实际能不能"必须一致。

use crate::model::{ChatCompletionMessage, ModelChatOptions, ProviderConfig, StreamDelta};
use crate::ports::tools::BoxFuture;
use crate::ports::CancelToken;

/// 一次补全请求。
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub config: ProviderConfig,
    pub messages: Vec<ChatCompletionMessage>,
    pub options: ModelChatOptions,
}

/// 增量流。沿用现有的 `mpsc::Receiver<StreamDelta>`：改动面最小，
/// 也让"合帧/取消"的语义与今天完全一致。
pub type DeltaStream = tokio::sync::mpsc::Receiver<StreamDelta>;

/// 能力位：**握手与降级都读它**，不许"解析了不用"（现状 `hookTimeoutMs` 就是这样被扔掉的）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub images: bool,
    pub thinking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    Network(String),
    Protocol(String),
    Unauthorized,
    RateLimited { retry_after_ms: Option<u64> },
    Cancelled,
}

pub trait ModelClient: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;

    /// 流式调用。取消语义：`cancel` 被置位后必须尽快结束流并返回
    /// （现状是"传了 abort_rx 但子进程/工具侧拿不到"，见计划 §1.3）。
    fn stream<'a>(
        &'a self,
        req: CompletionRequest,
        cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, Result<DeltaStream, ModelError>>;
}
