//! 作用域端口：取代贯穿一切实现细节的 `workspace: &Path`。
//!
//! coding 助手的"作用域"是工作区目录；生活类助手可能是"用户数据根 + 资源白名单"；
//! 通用助手可能只是一个会话沙箱。内核只要求两件事：**一个稳定标识** + **一次路径判定**。

use std::path::PathBuf;

use crate::domain::DenialKind;

pub trait Scope: Send + Sync {
    /// 稳定标识（用于落盘分组、日志、事件归属）。
    fn id(&self) -> &str;

    /// 把外部传入的路径解析成作用域内的真实路径；越界一律 [`DenialKind::Sandbox`]。
    /// 失败方向固定为**拒绝**（对应 AGENTS.md §2 的失败安全）。
    fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind>;
}
