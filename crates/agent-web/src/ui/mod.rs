//! 组件库（服务端渲染）。
//!
//! 组成：
//!
//! - [`icons`]：全系统唯一的 SVG 图标来源，模板与 Rust 组件共用；
//! - [`theme`]：语义色、尺寸、类名与焦点态规范；
//! - [`components`]：可复用组件（按钮、药丸、输入、折叠、消息、提示、插槽…）；
//! - [`rows`]：对话流行的视图模型与渲染。
//!
//! 组件只依赖 `--ph-*` 设计变量（见 `static/components.css`），
//! 不写死颜色，因此深浅主题是同一套信息层级的两种呈现。

#![allow(dead_code, unused_imports)]

pub mod components;
pub mod icons;
pub mod rows;
pub mod sidebar;
pub mod theme;
pub mod todo;

pub use components::{
    effort_label, model_chip_inner, normalize_effort, render_slot, waited_from_ms,
    AssistantMessage, AuxItem, Button, Chip, Composer, ComposerAux, ConfirmDialog, Disclosure,
    EmptyState, IconButton, Modal, ModelChoice, ModelGroup, ModelPicker, ModelTable, ModelTableRow,
    Notice, Panel, QueueItem, QuestionTray, QueueTray, Render, Select, SendButton, Slot, TabItem,
    Tabs, TextField, ToolCall, UserMessage,
};
pub use icons::{icon, sprite};
pub use rows::{
    render_empty, render_empty_thread, render_new_conversation, render_row, RowView, WorkspaceChoice,
};
pub use theme::{ButtonKind, ButtonVariant, Size, Tone};
pub use todo::{derive_todo_plan, read_todo_items, TodoList, TodoPlan, TodoStatus, TodoStep};

/// 渲染期取当前时刻（毫秒）。
///
/// 只服务于"最后活动 2 天前"这类**纯展示**计算：agent-web 是投影层，不持有
/// 引擎的时钟端口（`Clock` 在 `agent-base`），也不该为了显示相对时间引一个。
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
