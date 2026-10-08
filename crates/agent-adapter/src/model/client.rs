//! 网络模型客户端适配器：实现基座的 `ModelClient` 端口。

use agent_base::ports::{
    BoxFuture, CancelToken, CompletionRequest, DeltaStream, ModelCapabilities,
    ModelClient, ModelError,
};
use tokio::sync::watch;

use super::stream::stream_model_chat;

/// 基于网络 HTTP/SSE 的真实大模型客户端（支持 3 家协议与指数退避重试）。
pub struct NetworkModelClient {
    capabilities: ModelCapabilities,
}

impl NetworkModelClient {
    pub fn new() -> Self {
        Self {
            capabilities: ModelCapabilities {
                streaming: true,
                tools: true,
                images: false, // MVP 严格遵从如实回答：false，不虚报
                thinking: true,
            },
        }
    }
}

impl Default for NetworkModelClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelClient for NetworkModelClient {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities
    }

    fn stream<'a>(
        &'a self,
        req: CompletionRequest,
        cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, Result<DeltaStream, ModelError>> {
        Box::pin(async move {
            let is_cancelled = cancel.map(|c| c.is_cancelled()).unwrap_or(false);
            let (_abort_tx, abort_rx) = watch::channel(is_cancelled);

            let rx = stream_model_chat(req.config, req.messages, req.options, Some(abort_rx)).await;
            Ok(rx)
        })
    }
}
