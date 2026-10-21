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

/// 供应商详情里的一个可用模型（对应 `ModelEntry`）。
#[derive(Debug, Clone)]
pub struct ProviderModelRow {
    /// 列表内下标：模型 id 里可能带 `/`，用下标寻址才不会和路由冲突。
    pub index: usize,
    pub id: String,
    pub name: String,
    pub context_window: String,
    pub max_output_tokens: String,
    pub supports_images: bool,
    /// 是否为该供应商当前生效的模型。
    pub current: bool,
}

/// 供应商卡片：列表与详情共用一份数据（详情多带几个可编辑字段）。
#[derive(Debug, Clone)]
pub struct ProviderCard {
    pub id: String,
    /// 左列链接（`id` 已做查询编码，别在模板里再拼）。
    pub href: String,
    pub name: String,
    pub protocol: String,
    pub protocol_label: String,
    pub base_url: String,
    pub proxy_url: String,
    /// `k: v; k2: v2` 文本形式，编辑框直接回填。
    pub headers: String,
    /// 是否已配置密钥（密钥本身**不回显**，只在后端）。
    pub api_key_set: bool,
    /// 打码后的提示，例如 `sk-…7f3a`。
    pub api_key_hint: String,
    pub model_count: usize,
    pub active: bool,
    pub models: Vec<ProviderModelRow>,
    /// 该供应商每个模型的编辑弹窗（组件库渲染好的 HTML，模板直接塞进去）。
    pub modals: String,
    /// 列表行右侧的「删除供应商」动作（组件库渲染；最后一个供应商是禁用态）。
    pub delete_form: String,
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
    /// 审批 + 排队托盘（组件库渲染）。
    pub trays_html: String,
    /// 组件库渲染好的输入框（含插件插槽）。
    pub composer_html: String,
    /// 任务规划悬浮面板（组件库渲染）；无规划时为空。
    pub todo_html: String,
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

/// 协议下拉项。
#[derive(Debug, Clone)]
pub struct ProtocolOption {
    pub value: String,
    pub label: String,
}

#[derive(Template)]
#[template(path = "providers.html")]
pub struct ProvidersPage {
    pub base: PageBase,
    /// 左列的供应商清单（顺序＝后端顺序）。
    pub providers: Vec<ProviderCard>,
    /// 右列的当前选中项。
    pub selected: Option<ProviderCard>,
    /// 当前生效的模型与端点（顶部状态条）。
    pub current_model: String,
    pub current_base_url: String,
    /// 协议下拉的组件渲染结果（Select 组件，非原生 `<select>`）。
    pub protocol_select: String,
    /// 详情区的页签导航条（组件库渲染）。面板由模板摆放，id 前缀与它一致。
    pub tabs: String,
    /// 当前页签 key（`connection` / `models`），决定哪块面板不渲染 `hidden`。
    pub active_tab: String,
    /// "新增模型"弹窗（组件库渲染；列表区的「新增模型」按钮用 data-modal-open 打开）。
    pub model_new_modal: String,
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
