//! **a_da SSR 网页界面**（产品代号 `ssr-h`）：网关的 UI 投影层。
//!
//! 这个 crate 有两种使用方式：
//!
//! 1. **被网关挂载**（最终形态）：`agent_gateway` 依赖本 crate，调用 [`router::router`]
//!    把页面、表单动作、SSE 与插件路由挂到网关同一个端口上；
//! 2. **独立运行**（开发/验收）：`ssr-h` 二进制通过 [`agent::AgentClient`] 的 HTTP 后端
//!    连到已经启动的网关。
//!
//! 两种方式共用同一套处理器、模板与静态资源；差别只在 [`agent::AgentClient`] 背后的
//! 传输实现（`HttpBackend` / 网关进程内的 `GatewayBackend`）。
//!
//! # 边界
//!
//! 本 crate 是**投影层**：不持有线程/消息/工具状态的第二份真相，不跑多轮引擎。
//! 它只依赖线协议常量（`agent-proto`）与 axum/Askama/Boa 等纯前端依赖；
//! 不得依赖 `agent-node`、`agent-base`、`agent-runtime` 等节点/引擎 crate。

pub mod agent;
pub mod backend;
pub mod context;
pub mod guard;
pub mod handlers;
pub mod html;
pub mod markdown;
pub mod plugins;
pub mod questions;
pub mod router;
pub mod standalone;
pub mod templates;
pub mod tsx;
pub mod ui;

pub use handlers::AppState as WebState;
pub use router::router;
