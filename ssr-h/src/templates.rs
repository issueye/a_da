//! Askama 模板的数据结构。
//!
//! 所有模板共享 `PageBase`；布局只依赖它，插件导航与插件插槽也在这里统一注入。
//!
//! v0.6 起，重复出现的 UI 由**服务端组件库**（`crate::ui`）渲染成 HTML，
//! 模板只负责摆放。因此这里的视图模型大多已经是"渲染好的片段"，
//! 模板不再逐个元素地拼 class —— 组件库才是 UI 契约的唯一来源。

use askama::Template;

use crate::plugins::{NavLink, PluginWidget};

#[derive(Debug, Clone)]
pub struct Flash {
    pub kind: String,
    pub message: String,
}

/// 插件插槽的渲染结果（每个插槽一段 HTML）。
#[derive(Debug, Clone, Default)]
pub struct SlotContent {
    pub sidebar_top: String,
    pub sidebar_bottom: String,
    pub topbar_right: String,
    pub composer_left: String,
    pub composer_right: String,
    pub composer_footer: String,
    /// 输入框上方的悬浮插槽（贴底浮在对话流之上）。
    pub composer_above: String,
    pub transcript_after: String,
    pub extensions_list: String,
    pub dashboard_widgets: String,
}

#[derive(Debug, Clone)]
pub struct PageBase {
    pub title: String,
    pub active: String,
    pub workspace: String,
    pub gateway: String,
    pub product: String,
    pub plugin_nav: Vec<NavLink>,
    pub flash: Option<Flash>,
    /// 插件插槽内容（SSR 期填充）。
    pub slots: SlotContent,
    /// 图标 sprite（组件库生成的唯一图标源）。
    pub icons: String,
}

#[derive(Debug, Clone)]
pub struct Stat {
    pub label: String,
    pub value: String,
    pub hint: String,
    pub tone: String,
}

#[derive(Debug, Clone)]
pub struct DashboardView {
    pub stats: Vec<Stat>,
    pub widgets: Vec<PluginWidget>,
    pub recent: Vec<ThreadRow>,
}

#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub mode: String,
    pub running: bool,
    pub item_count: usize,
}

/// 对话流的一行：**组件库渲染好的 HTML**。
#[derive(Debug, Clone)]
pub struct ItemRow {
    pub html: String,
}

#[derive(Debug, Clone)]
pub struct ApprovalRow {
    pub call_id: String,
    pub tool_name: String,
    pub args: String,
    pub waited: String,
}

#[derive(Debug, Clone)]
pub struct QueueRow {
    pub index: usize,
    pub order: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct FrontendPluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub nav_count: usize,
    pub page_count: usize,
    pub slot_count: usize,
    pub first_page: Option<String>,
    pub has_dashboard: bool,
    pub has_assets: bool,
    pub enabled: bool,
    pub status: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentPluginRow {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub enabled: bool,
    pub status: String,
    pub tools: usize,
    pub error: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub base: PageBase,
    pub view: DashboardView,
}

#[derive(Template)]
#[template(path = "partials/dashboard.html")]
pub struct DashboardPartial {
    pub view: DashboardView,
    /// 页面外壳数据（插槽挂载点从这里取）。
    pub base: PageBase,
}

/// 截图式主界面：左侧会话栏 + 中间对话流 + 底部输入框。
#[derive(Template)]
#[template(path = "chat.html")]
pub struct ChatPage {
    pub base: PageBase,
    pub threads: Vec<ThreadRow>,
    pub active: Option<ThreadRow>,
    pub active_thread_id: String,
    /// 已渲染好的对话行。
    pub items: Vec<ItemRow>,
    /// 空会话时的提示片段。
    pub transcript_empty: String,
    /// 审批托盘（组件库渲染）。
    pub approvals_html: String,
    /// 排队托盘（组件库渲染）。
    pub queue_html: String,
    /// 顶栏展示的审批档位文案。
    pub approval: String,
    /// 组件库渲染好的输入框（含插件插槽）。
    pub composer_html: String,
    /// 任务规划悬浮面板（组件库渲染）；无规划时为空。
    pub todo_html: String,
    pub connected: bool,
}

#[derive(Template)]
#[template(path = "partials/transcript.html")]
pub struct TranscriptPartial {
    pub items: Vec<ItemRow>,
    pub transcript_empty: String,
}

#[derive(Template)]
#[template(path = "providers.html")]
pub struct ProvidersPage {
    pub base: PageBase,
    pub providers: Vec<ProviderRow>,
    pub model: String,
    pub base_url: String,
}

#[derive(Template)]
#[template(path = "extensions.html")]
pub struct ExtensionsPage {
    pub base: PageBase,
    pub frontend: Vec<FrontendPluginRow>,
    pub agent: Vec<AgentPluginRow>,
    pub frontend_count: usize,
    pub agent_count: usize,
}

#[derive(Template)]
#[template(path = "plugin.html")]
pub struct PluginPage {
    pub base: PageBase,
    pub plugin_name: String,
    pub page_title: String,
    pub html: String,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    pub base: PageBase,
    pub message: String,
}
