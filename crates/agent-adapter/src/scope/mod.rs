//! 作用域适配器：把"工作区目录"实现成 `agent_base::ports::Scope`。
//!
//! 设计口径（`docs/architecture/kernel-design.md` §3、`docs/plans/wiring.md` §5 W1-T1）：
//! 内核只要求作用域提供两件事——**一个稳定标识** + **一次路径判定**；
//! 判定逻辑（`..` 穿越、realpath、符号链接/junction）**不在适配器里重写**，
//! 而是复用 `agent-toolkit::check_workspace_sandbox`（R2：一次只动一个真源）。
//!
//! `access` 是 P1b 加的**第二层**：按权限档位决定"能不能越出工作区"
//! （保留目录红线任何档位都不可越）。

pub mod access;
pub mod workspace;

pub use access::{ApprovalAwareScope, ApprovalModeSource};
pub use workspace::WorkspaceScope;
