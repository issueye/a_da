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

#[cfg(test)]
mod tests {
    /// **S1 防复活断言**：`runner/` 的三个 legacy 残渣文件不得回来。
    ///
    /// 为什么需要：它们被删掉前有 **991 行**，而且**不报 `dead_code` 警告**——
    /// 因为 `runner/mod.rs` 用 `pub use` 把它们公开了（`pub` 遮蔽死代码检测）。
    /// 也就是说，同样的东西可以**悄无声息地长回来**：有人加回一个 `pub use`、
    /// 加回一个"过渡用"的函数，编译器一声不吭。
    ///
    /// 这条断言用**文件存在性**做判据（比符号名更稳）：文件回来就报红。
    ///
    /// **S4 迁址说明**：这两条断言原先在 `agent-core/src/lib.rs`，随 `runner/` 一起
    /// 搬到了本 crate——**断言必须和它守的代码在同一个 crate**，否则
    /// `CARGO_MANIFEST_DIR` 会指向别处（拆包时就是这么红的：断言查的是
    /// `agent-core/src/runner`，而那里已经没有 `runner/` 了）。
    #[test]
    fn test_legacy_runner_residue_must_not_come_back() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runner");
        for gone in ["builtin_tools.rs", "executor.rs", "prompt.rs"] {
            assert!(
                !src_dir.join(gone).exists(),
                "`src/runner/{gone}` 是 W3-T4 的 legacy 残渣（991 行死代码），S1 已删除。\
                 它被 `pub use` 遮蔽了 dead_code 检测，请勿加回；\
                 若确有需要，请先说明它由谁调用"
            );
        }

        // 反向：`runner/mod.rs` 不得再导出这些符号（`pub use` 是遮蔽的来源）
        //
        // ⚠️ 只扫**生产段**：本断言自己就在 `runner/mod.rs` 里，被禁的字符串
        // 作为字面量出现在下面——不切掉测试段就会**自己把自己判红**。
        // （S1 时这两条断言在 `agent-core/src/lib.rs`，扫的是另一个文件，所以没暴露；
        //   S4 搬到同文件后才现形。）
        let mod_src = std::fs::read_to_string(src_dir.join("mod.rs")).expect("runner/mod.rs 应存在");
        let production = mod_src.split("#[cfg(test)]").next().unwrap_or("");
        for banned in [
            "pub use executor::",
            "pub use prompt::",
            "pub mod executor;",
            "pub mod prompt;",
            "pub mod builtin_tools;",
        ] {
            assert!(
                !production.contains(banned),
                "`runner/mod.rs` 的生产段不得出现 `{banned}`——它会把死代码重新变成\
                 「公开 API」从而遮蔽 dead_code 检测"
            );
        }
    }

    /// **S1 出口判据**：删掉的 991 行必须真的没了，且剩下的模块仍在。
    #[test]
    fn test_runner_module_is_lean_after_s1() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runner");
        for kept in ["mod.rs", "engine_bridge.rs", "ui_events.rs"] {
            assert!(src_dir.join(kept).exists(), "`src/runner/{kept}` 必须保留");
        }
        let count = std::fs::read_dir(&src_dir)
            .expect("runner 目录应存在")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
            .count();
        assert_eq!(count, 3, "`src/runner/` 应只剩 3 个文件，实际 {count} 个");
    }
}
