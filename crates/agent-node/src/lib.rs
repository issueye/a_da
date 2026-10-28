//! **Agent 节点**（S4 从 `agent-core` 拆出）：一个可被管理、可被委派的 agent。
//!
//! # 边界（两条硬约束）
//!
//! | 约束 | 理由 |
//! |---|---|
//! | **不含协议管道**（不得依赖 `tokio-tungstenite`） | 协议管道是**桥接层**的事（`agent-rpc`，终点是网关） |
//! | **不含 UI 投影**（不得依赖 `AgentStore`） | 节点是领域行为；UI 投影归桥接层（S2 已断掉这条依赖，`verify-wiring` check H 盯着） |
//!
//! 节点只依赖基座各 crate（`agent-base` / `agent-proto` / `agent-adapter` / `agent-toolkit`
//! / `agent-runtime`）+ `ada-ts-engine`（插件沙箱）。
//!
//! # 与 `agent-rpc` 的关系
//!
//! ```text
//! agent-rpc（桥接面）：JSON-RPC 分发 + WS 宿主 + UI 投影 + 快照
//!    └── agent-node（本 crate）：会话 / 提问 / 检查点 / 委派 / 插件 / 技能
//!           └── agent-base（引擎与端口）
//! ```
//!
//! 依赖方向**单向**：`agent-rpc → agent-node`。反向的依赖由 `cargo xtask verify-wiring` 守门。

pub mod agent_bus;
pub mod checkpoint;
pub mod delegation_depth;
pub mod hooks;
pub mod node_config;
pub mod plugins;
pub mod scripts;
pub mod session;
pub mod skills;
pub mod subagents;
