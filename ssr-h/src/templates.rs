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
    /// 会话区右上角的悬浮插槽（任务规划之类的常驻卡片）。
    pub chat_corner: String,
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
    /// 当前**激活**的工作区（快照里的 `workspace.project`；没有快照时退回启动参数）。
    /// 侧栏的"新会话"与工作区树用它，而不是静态的 `workspace`。
    pub active_workspace: String,
    /// 当前激活工作区的末段目录名（只用于展示）。
    pub active_workspace_name: String,
    pub gateway: String,
    pub product: String,
    pub plugin_nav: Vec<NavLink>,
    /// 启用插件声明的 CSS（由 manifest `styles` 注入 `<head>`）。
    pub plugin_styles: Vec<String>,
    /// 启用插件声明的 JS（由 manifest `scripts` 注入 `<head>`，`defer`）。
    pub plugin_scripts: Vec<String>,
    pub flash: Option<Flash>,
    /// 插件插槽内容（SSR 期填充）。
    pub slots: SlotContent,
    /// 图标 sprite（组件库生成的唯一图标源）。
    pub icons: String,
    /// 侧栏整块 HTML（组件库 [`crate::ui::sidebar`] 渲染）。
    ///
    /// 侧栏原先在 `chat.html` 与 `layout.html` 各写一份，改一次要改两处；
    /// 现在模板只摆 `{{ base.sidebar|safe }}`。聊天页拿到工作区树之后会覆盖它。
    pub sidebar: String,
    /// 共享确认弹窗（组件库渲染）：每个页面外壳都要带上，别在模板里手写一份。
    pub confirm_dialog: String,
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
    /// 会话创建时刻（毫秒）：工作区树按"最近会话"排序用。
    pub created_at: u64,
}

/// 侧栏工作区树的一个分组：一个工作区 + 它下面的会话。
#[derive(Debug, Clone)]
pub struct WorkspaceGroup {
    /// 规范化后的路径键（大小写/斜杠归一），用于判断"同一个工作区"。
    pub key: String,
    /// 原始路径（展示与表单提交都用它）。
    pub path: String,
    /// 末段目录名。
    pub name: String,
    pub threads: Vec<ThreadRow>,
    pub count: usize,
    pub running: bool,
    /// 是否是当前激活工作区（决定默认展开与"当前"标记）。
    pub active: bool,
}

/// 工作区选择器里的一个根节点（磁盘 / 主目录 / 当前项目）。
#[derive(Debug, Clone)]
pub struct FsRootRow {
    pub label: String,
    pub path: String,
    pub href: String,
    pub kind: String,
}

/// 工作区选择器里的一个可进入目录。
#[derive(Debug, Clone)]
pub struct FsEntryRow {
    pub name: String,
    pub path: String,
    pub href: String,
}

/// 工作区选择器里的"最近工作区"条目。
#[derive(Debug, Clone)]
pub struct WorkspaceRow {
    pub path: String,
    pub name: String,
    pub sessions: usize,
    pub active: bool,
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
    /// 侧栏工作区树（按工作区分组；当前工作区即使没有会话也会出现）。
    pub workspaces: Vec<WorkspaceGroup>,
    /// 当前会话所属工作区（"新会话"提交到它；空串时由 agent 用默认值）。
    pub current_workspace: String,
    pub active: Option<ThreadRow>,
    pub active_thread_id: String,
    /// 已渲染好的对话行。
    pub items: Vec<ItemRow>,
    /// 空会话时的提示片段。
    pub transcript_empty: String,
    /// 审批 + 排队托盘（组件库渲染）。
    pub trays_html: String,
    /// 组件库渲染好的输入框（含插件插槽）。
    pub composer_html: String,
    /// 顶栏状态药丸（运行中 / 审批档位 / 未连接）。
    pub topbar_status: String,
    /// 顶栏刷新 / 停止按钮。
    pub topbar_actions: String,
    pub connected: bool,
}

#[derive(Template)]
#[template(path = "partials/transcript.html")]
pub struct TranscriptPartial {
    pub items: Vec<ItemRow>,
    pub transcript_empty: String,
}

/// 工作区选择/添加页：左边驱动器与最近工作区，右边目录浏览。
#[derive(Template)]
#[template(path = "workspaces.html")]
pub struct WorkspacesPage {
    pub base: PageBase,
    pub roots: Vec<FsRootRow>,
    pub recent: Vec<WorkspaceRow>,
    /// 当前浏览目录（真实路径）。
    pub current: String,
    /// 上一级目录；根目录时为空。
    pub parent: String,
    /// 上一级目录的链接（路径已做查询编码）。
    pub parent_href: String,
    /// 当前目录下的子目录（只列目录：工作区必须是目录）。
    pub entries: Vec<FsEntryRow>,
    /// 目录被后端截断时的提示（`truncated` / `omitted`）。
    pub truncated: bool,
    pub omitted: usize,
}

/// 协议下拉项。
#[derive(Debug, Clone)]
pub struct ProtocolOption {
    pub value: String,
    pub label: String,
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
    pub plugin_id: String,
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

