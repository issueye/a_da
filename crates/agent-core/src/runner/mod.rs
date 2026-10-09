//! 引擎桥接与 UI 事件投影。
//!
//! # 这里曾经有什么（S1 删除记录）
//!
//! 本模块原有 5 个文件，其中 3 个是 **W3-T4 删除 legacy 主循环后的残留**：
//!
//! | 文件 | 行数 | 处置 |
//! |---|---|---|
//! | `builtin_tools.rs` | 436 | **已删**：`execute_ask_user` / `execute_builtin_plugin_tool` 无人调用 |
//! | `executor.rs` | 253 | **已删**：`execute_tool_call` / `capture_tool_checkpoint` 无人调用 |
//! | `prompt.rs` | 302 | **已删**：`builtin_tools()` / `build_system_prompt` / `format_messages_for_model` 无人调用 |
//!
//! 合计 **991 行**。它们之所以不报 `dead_code` 警告，是因为本文件原先用
//! `pub use` 把它们"公开"了——`pub` 会遮蔽死代码检测。
//!
//! ## 删除前确认过的三件事
//!
//! 1. **外部零引用**：全仓（含 `products/`、`src-tauri/`、`tauri-ui/`）没有任何调用点；
//! 2. **行为有替代**：`batch_write` / `decide` / `check_gate` 在 `agent-toolkit` 里有
//!    真正的 `Tool` 实现与测试；`build_system_prompt` 由 `agent-adapter` 的
//!    `CodingPromptSource` 取代（含 8 条测试）；
//! 3. **`evaluate_diff` / `manage_ponytail` 连描述符都不在**（不在 24 个工具里）——
//!    那些测试测的是死分支。
//!
//! ## 删掉的 7 个测试里，有 1 个是"唯一实现"
//!
//! `ask_user` 的问句流程：legacy `execute_ask_user` 是**唯一**会注册 waiter 并发
//! `QuestionAsked` 事件的实现，而新路径的 `AskUserTool` 直接返回错误
//! （见 `agent-toolkit/src/core/ask_user.rs`）。
//!
//! 因此这次删除**没有引入回归**（该能力在新路径上本来就不通），但**缺口已登记**：
//! `cargo xtask verify-wiring` 的 `check_event_emitters` 把 `QuestionAsked` 钉在
//! `UNEMITTED_EVENT_ALLOW` 豁免表里——**补齐发射者后必须删掉该豁免，否则门会报红**。
//!
//! 同类缺口还有 `SubagentStarted` / `SubagentFinished`（同一张豁免表）。

pub mod engine_bridge;
pub mod ui_events;

pub use engine_bridge::{
    project_to_loop_event, run_agent_turn, run_turn_with_engine, EngineError, LoopEventBridge,
};
pub use ui_events::AgentLoopEvent;
