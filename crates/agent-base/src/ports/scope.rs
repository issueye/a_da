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
    ///
    /// # 失败方向**固定为拒绝**，且刻意不做成可配置（W5-T5 的设计决定）
    ///
    /// [`crate::ports::ApprovalGate`] 有 `direction()`，因为"等审批"确实会**拿不到判定依据**
    /// （超时、会话中止）——那是"没有答案"，需要方向来决定往哪边倒。
    ///
    /// 这里不一样：`resolve_path` 返回 `Err` 是一个**确定的判定**（路径越界），
    /// 不是"没有答案"。如果给它一个 `FailDirection::Open` 开关，语义会变成
    /// "拿不准时允许逃出沙箱"——那是把安全边界交给配置错误。
    ///
    /// 因此本端口**不提供** `direction()`：越界就是拒绝，没有第二种解释。
    /// 对应 AGENTS.md §2 的失败安全（未知/拿不准 → 当作危险处理）。
    fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind>;
}
