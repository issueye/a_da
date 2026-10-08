//! 应用数据目录端口：内核与适配器**不许**自己读 `A_DA_HOME`/`USERPROFILE`/`HOME`。
//!
//! 现状是 `agent_core::session::get_app_home()` 被 10 多处直接调用，于是
//! "测试会写用户真实的 `~/.a-da`"这类问题没有解（TS 时代靠 `scripts/test-preload.ts` 重定向，
//! Rust 侧没有等价物）。把它做成端口后，组合根注入真实实现、测试注入临时目录。
//!
//! **刻意不枚举分区**（`DataKind::Plugins` 之类）：那等于把某个产品的目录表写进基座，
//! 而"生活类助手"根本没有 checkpoints/插件这些概念。基座只提供**具名子路径**，
//! 名字由产品层决定（[`AppHome::dir`]）。

use std::path::{Path, PathBuf};

pub trait AppHome: Send + Sync {
    /// 应用数据根目录。
    fn root(&self) -> &Path;

    /// 根目录下的具名子目录（**不保证已存在**：创建由使用方负责）。
    /// 名字是产品层的约定，例如 `"sessions"` / `"checkpoints"` / `"extensions"`。
    fn dir(&self, name: &str) -> PathBuf;

    /// 配置文件的完整路径。单独一个方法是因为它可能被独立的环境变量覆盖（现状 `A_DA_CONFIG`）。
    fn config_file(&self) -> PathBuf;
}
