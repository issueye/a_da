//! 模型供应商适配器：把 `agent-base` 的请求类型送上网，把响应解成 `StreamDelta`。

pub mod stream;

pub use stream::stream_model_chat;
