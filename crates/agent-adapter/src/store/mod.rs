//! 会话与检查点的**落盘适配器**（设计 §9 的 `store-fs`）。
//!
//! 布局：
//! - [`types`]：会话文件格式（`SessionHeader` / `SessionEntry*` / `SessionSummary`）
//! - [`slug`]：工作区散列目录名与会话 id 清洗
//! - [`jsonl`]：行级读写 + 折叠语义（`agent-core` 的 `SessionManager` 与
//!   [`fs_store::FsSessionStore`] **共用这一份**，R2）
//! - [`checkpoint_types`]：检查点落盘类型
//! - [`fs_store`]：`SessionStore` 端口的真实实现
//!
//! 搬运口径见 `docs/agent-base-wiring-plan.md` §5 W1-T2a；`agent-core` 侧保留
//! `pub use` 兼容 shim（R6），调用点零改动。

pub mod checkpoint_types;
pub mod fs_store;
pub mod jsonl;
pub mod slug;
pub mod types;

pub use fs_store::FsSessionStore;
pub use slug::{safe_id, workspace_slug};
pub use types::*;
