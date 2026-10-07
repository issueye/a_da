//! 代理至独立的 ts_engine 模块，保持历史符号与路径兼容
pub use ts_engine::env as api;
pub use ts_engine::runtime as event_loop;

pub use ts_engine::env::{inject_node_environment, inject_p0_environment};
pub use ts_engine::runtime::{EventLoopMsg, PureTsRuntime};
