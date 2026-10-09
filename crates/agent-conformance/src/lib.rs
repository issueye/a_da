//! AGENT CONFORMANCE 合规套件（`docs/agent-base-design.md` v0.2 §8）
//!
//! 提供：
//! 1. [`ports`] —— 端口契约断言集合。
//!    端口清单以 `crates/agent-base/src/ports/` 的 `pub trait` 为**唯一真源**
//!    （当前 **11 个**：`AppHome`、`ApprovalGate`、`CancelToken`、`Clock`、`EventSink`、
//!    `ModelClient`、`PromptSource`、`Scope`、`SessionStore`、`Tool`、`ToolCatalog`）。
//!    `cargo xtask verify-wiring` 会**从 trait 定义派生**这份清单，逐个要求有生产实现。
//!
//!    口径订正（W6-T5）：此处曾写"8 个端口（… RollbackStore, PluginHost, ScopePolicy …）"，
//!    但 `RollbackStore` / `PluginHost` **在本仓从未存在**，`ScopePolicy` 也早已改名 `Scope`——
//!    那是一句"看起来在描述系统、实际描述幻想"的注释。现在按 trait 实际列出。
//! 2. [`invariants`] —— 8 条跨端口系统级不变量（单一引擎、隔离运行、注册表真源、失败闭合、结构化回执、单调 seq、领域投影分离、取消贯穿）
//! 3. [`golden`] —— golden 回放夹具（W3-T5）：脚本化模型 → 冻结事件序列

pub mod golden;
pub mod invariants;
pub mod ports;

pub use golden::{
    assert_all_fixtures, assert_fixture, compare, golden_dir, load_fixtures, parse_fixture,
    project_event, replay, stop_reason_name, ExpectedEvent, GoldenExpectation, GoldenFixture,
    GoldenReport,
};
pub use invariants::*;
pub use ports::*;
