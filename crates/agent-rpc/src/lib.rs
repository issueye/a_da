//! **JSON-RPC 桥接面**（S4 从 `agent-core` 拆出）：把「客户端说的话」翻译成「节点要做的事」。
//!
//! # 边界
//!
//! | 内容 | 说明 |
//! |---|---|
//! | `server/` | 72 个方法的 JSON-RPC 分发、WS 宿主、快照合帧、事件投影、宿主侧文件浏览 |
//! | `state/` | `AgentStore`——**给界面看的投影**（线程、条目、UI 外壳、待答问题） |
//! | `runner/` | 引擎调用 + `AgentEvent` → `AgentLoopEvent` 投影（UI 事件词汇表） |
//!
//! 依赖方向**单向**：本 crate 依赖 `agent-node`，**绝不反向**。
//!
//! # 这是过渡形态
//!
//! 网关方向（见 `docs/agent-base-wiring-plan.md`）里，桥接平台的终点是
//! **并入网关**：协议管道（帧 / 鉴权 / 广播 / 路由）归网关，节点只暴露一个薄 RPC 面。
//! 所以本 crate 是"**过渡形态**"，不是终点——它的存在是为了让拆包**分步可验证**，
//! 而不是一次性把 `agent-core` 劈成三块。

pub mod gateway_bus;
pub mod runner;
pub mod server;
pub mod state;

// 基座兼容 shim（与 `agent-core` 时代同名同路径，调用点不动）
pub mod ai;
pub mod protocol;
pub mod tools;
