//! **a-da 网关**：AGENT 的管理平台、AGENT 之间的交互平台、与桌面端/WEB 端的桥接平台。
//!
//! # 它是什么
//!
//! ```text
//! 桌面端 / WEB 端
//!       │ WS（管理 API + 透传）
//!       ▼
//!   a-da-gateway ──┬─ 管理平台：注册表 / 生命周期 / 健康
//!                  ├─ 交互平台：AgentBus 服务端（S6）
//!                  └─ 桥接平台：协议管道 / 路由
//!       │ 节点 RPC（薄）
//!       ▼
//!   ada-coding × N（agent-node）
//! ```
//!
//! # 两条硬边界（已写成 `verify-wiring` 门禁）
//!
//! | 边界 | 理由 |
//! |---|---|
//! | **网关不含引擎** | INV-1：单一引擎。网关是管理/路由平面，不跑 agent |
//! | **网关不缓存会话状态** | INV-8：线程/消息/工具回执归节点 |
//!
//! 所以本 crate 只依赖 **`agent-proto`（线协议）**——不依赖 `agent-node`、
//! `agent-base::engine`、`agent-toolkit`。这条边界由 check J 守着。

pub mod auth;
pub mod delegate;
pub mod registry;
pub mod relay;
pub mod supervisor;

pub use auth::{AuthConfig, AuthError, AuthOutcome, TokenScope};
pub use delegate::{delegate, DelegateError, DelegateOutcome, DelegationRegistry, MAX_DELEGATION_DEPTH};
pub use registry::{AgentInstance, AgentRegistry, AgentStatus, normalize_workspace};
pub use relay::{Gateway, GATEWAY_METHOD_PREFIX, methods};
pub use supervisor::{ensure_agent, spawn_agent, SpawnError, SpawnSpec};
