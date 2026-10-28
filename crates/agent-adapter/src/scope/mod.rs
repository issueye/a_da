//! 作用域适配器：把"工作区目录"实现成 `agent_base::ports::Scope`。
//!
//! 内核只要求作用域提供两件事——**一个稳定标识** + **一次路径解析**。
//! 解析逻辑（`..`、realpath、符号链接/junction）不在适配器里重写，
//! 而是复用 `agent-toolkit::resolve_path`（R2：一次只动一个真源）。
//!
//! 访问边界不属于这一层：核心不再做允许/拒绝判定。

pub mod resolver;

pub use resolver::PathResolver;
