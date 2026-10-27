//! 插件运行环境注入（process / path / Buffer / fs / net / sqlite / timers 等）。
//!
//! `Needs` 是能力声明；具体注入流程在 `inject` 子模块，本文件只保留公开类型与再导出。

pub mod assert;
pub mod buffer;
pub mod child_process;
pub mod child_process_async;
pub mod crypto;
pub mod events;
pub mod fs;
pub mod os;
pub mod path;
pub mod process;
pub mod sqlite;
pub mod timers;
pub mod url;
pub mod util;

mod inject;

pub use inject::{inject_environment, inject_node_environment, inject_p0_environment};

/// 环境注入清单（P1 · SP-13 的最小版）：**未声明即不注入**。
///
/// # 为什么"不注入"比"注入后再拦"更强
///
/// 拦截是在既成的能力面上加判断，绕过的路子（顺手造一个别的调用路径、拼一个别名）永远比
/// 拦截点多。不注入则连 API 都不存在——插件写不出那行代码，也就不需要拦。
///
/// # 为什么最小版只有两项
///
/// 这里只关**真正带外部副作用**的那一项（网络），fs 的读写分离与 crypto/process 的细粒度
/// 声明属于 P4·SP-71 的完整版。刻意**不**把无害的纯计算能力（crypto / url / util / Buffer）
/// 默认关掉：那只会给正常插件制造摩擦，换不来任何安全收益（"默认收紧"要收紧在能力上，
/// 不是收紧在算术上）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    /// 注入 `fs` 模块与它的原生读写能力（读写分离见 P4·SP-71）。
    pub fs_read: bool,
    pub fs_write: bool,
    /// 注入 `fetch` / `AbortController` / `Response` 与 `__native_fetch_async`。
    ///
    /// **默认 false**：这是插件唯一能"把数据送出去"的口子，值得显式声明才开。
    pub net: bool,
    /// 注入 `node:sqlite`（`DatabaseSync` / `StatementSync`）。
    ///
    /// **默认 false**：SQLite 会创建/改写工作区里的数据库文件。它与 fs 的边界
    /// 不同（SQL 能批量读写、能触发触发器），所以单独声明而不是搭 fs 的便车。
    pub sqlite: bool,
    /// 注入 `node:child_process`：同步子集（`execFileSync` / `spawnSync` / `execSync`）
    /// 与异步子集（`spawn` / `exec` / `execFile`，流式 stdout/stderr）。
    ///
    /// **默认 false**：这是任意代码执行，等价于把宿主用户的权限交给插件；只有
    /// 本地受信插件才应声明 `@ada-needs exec`。异步 `fork`（需要 IPC channel）仍不提供。
    pub exec: bool,
}

impl Default for Needs {
    fn default() -> Self {
        // 默认按**插件**口径收紧（与 [`Needs::everything`] 的差别只有网络这一项）
        Self::plugin_default()
    }
}

impl Needs {
    /// 全开：CLI（`ts_engine run/eval`）与本地脚本的现状行为——
    /// 那是用户自己写的脚本，不在"不可信第三方"的口径里。
    pub fn everything() -> Self {
        Self {
            fs_read: true,
            fs_write: true,
            net: true,
            sqlite: true,
            exec: true,
        }
    }

    /// 插件默认档：`fs: true`、`net/sqlite/exec: false`（见设计文档 §3.5 的默认档位）。
    pub fn plugin_default() -> Self {
        Self {
            fs_read: true,
            fs_write: true,
            net: false,
            sqlite: false,
            exec: false,
        }
    }
}

#[cfg(test)]
mod tests;
