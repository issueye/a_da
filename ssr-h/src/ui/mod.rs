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
    effort_label, model_chip_inner, normalize_effort, render_slot, waited_from_ms, ApprovalTray,
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
