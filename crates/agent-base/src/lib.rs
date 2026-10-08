//! AGENT BASE 内核（`docs/agent-base-design.md` v0.2）
//!
//! 约束（设计 §3、INV-8）：**零 IO、零产品名词、零传输、零 UI**。
//! 这里只放领域类型、端口 trait、引擎与策略；任何 `reqwest`/`std::fs`/`Path`/`workspace`
//! 都必须留在适配器里。
//!
//! 当前落地进度：`model/`（模型对话与用量类型，原 `agent_core::ai::types` +
//! `think_filter`）。`domain/`（Thread/Turn/Message/ToolDescriptor/ToolReceipt）、
//! `ports/`、`engine/`、`policy/` 按计划 M1 落地。

pub mod model;
