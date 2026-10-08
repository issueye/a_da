//! 模型面类型：对话消息、流式增量、用量、供应商配置（与具体产品无关）。
//!
//! `types` 与 `think_filter` 都是纯逻辑：不碰网络、不碰文件系统。

pub mod think_filter;
pub mod types;

pub use think_filter::{ThinkFilterPart, ThinkTagFilter};
pub use types::*;
