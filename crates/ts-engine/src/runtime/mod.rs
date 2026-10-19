//! 纯 Rust TypeScript 运行时。
//!
//! 实现按职责拆在子模块里：控制台捕获、预算限额、事件循环消息、线程局部状态、
//! 事件循环 worker、原生能力注册、公开运行时类型。`mod.rs` 只做模块声明与再导出，
//! 不再承载具体实现。

mod console;
mod limits;
mod messages;
mod natives;
mod runtime;
mod state;
mod worker;

pub use console::{CONSOLE_CAPTURE_LIMIT, ConsoleMode, ConsoleSnapshot};
pub use limits::{LIMIT_MARKER, Limits, is_limit_error};
pub use messages::EventLoopMsg;
pub use runtime::PureTsRuntime;

#[cfg(test)]
mod tests;
