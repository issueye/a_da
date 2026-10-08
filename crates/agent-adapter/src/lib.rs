//! AGENT BASE 适配器层（`docs/agent-base-design.md` v0.2 §3 的 `agent-adapter`）。
//!
//! 这里放"与外部世界打交道"的实现：模型供应商、会话存储、插件运行时、作用域。
//! 依赖方向永远是 **适配器 → agent-base**，不许反向。
//!
//! 当前落地进度：`model/`（原 `agent_core::ai::stream`，三家协议的 SSE 解析 + 中止）。
//! `store/`（会话落盘）、`plugin/`（ts_engine 沙箱）、`scope/` 按计划 M1 逐步搬入。

pub mod model;
