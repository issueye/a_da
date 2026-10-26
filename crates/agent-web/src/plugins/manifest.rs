//! 前端插件的清单格式。
//!
//! 在原有能力（导航 / 独立页面 / 仪表盘卡片 / 静态资源）之上，
//! 插件模板只接受 `.tsx`；页面、插槽、仪表盘卡片都由 TSX 在服务端渲染。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// 插件可以声明的挂载点。
///
/// 插槽是**稳定契约**：界面保证这些位置一直存在，插件只是往里放东西。
/// 插槽集合宁可少也不要随意加——每加一个就多一份需要维护的兼容承诺。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// 侧栏顶部（品牌区下方）。
    SidebarTop,
    /// 侧栏底部（插件导航之后）。
    SidebarBottom,
    /// 顶栏右侧（操作按钮区）。
    TopbarRight,
    /// 会话区**右上角**的悬浮点位（贴右上角浮在对话流之上；只在聊天页存在）。
    ///
    /// 给"常驻状态卡"（任务规划之类）用：它不该插进正文流，也不该占顶栏高度。
    ChatCorner,
    /// 输入框工具行左侧。
    ComposerLeft,
    /// 输入框工具行右侧。
    ComposerRight,
    /// 输入框下方辅助区。
    ComposerFooter,
    /// 输入框**上方**的悬浮点位（贴底浮在对话流之上）。
    ComposerAbove,
    /// 托盘区（输入框上方 in-flow 区域）里**提问段**的挂载点。
    ///
    /// 托盘区从上到下是：审批（核心）→ 提问 → 排队（核心）。插件在这里的输出
    /// **非空时替换核心提问托盘**（审批/排队不受影响）；插件停用、加载失败或
    /// 没有插件时自动退回核心实现（见 [docs/PLAN-question-panel.md](../docs/PLAN-question-panel.md) D5）。
    ThreadTray,
    /// 托盘区里**审批段**的挂载点。
    ///
    /// 插件输出非空时替换核心审批托盘；批准/拒绝仍提交核心
    /// `/approvals/{call}/decision`，插件只负责呈现，**不能**改变审批策略。
    ThreadApproval,
    /// 对话流末尾（消息之后）。
    TranscriptAfter,
    /// 扩展页列表。
    ExtensionsList,
    /// 概览页卡片区。
    DashboardWidgets,
}

impl Slot {
    /// 清单里使用的字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Slot::SidebarTop => "sidebar.top",
            Slot::SidebarBottom => "sidebar.bottom",
            Slot::TopbarRight => "topbar.right",
            Slot::ChatCorner => "chat.corner",
            Slot::ComposerLeft => "composer.left",
            Slot::ComposerRight => "composer.right",
            Slot::ComposerFooter => "composer.footer",
            Slot::ComposerAbove => "composer.above",
            Slot::ThreadTray => "thread.tray",
            Slot::ThreadApproval => "thread.approval",
            Slot::TranscriptAfter => "transcript.after",
            Slot::ExtensionsList => "extensions.list",
            Slot::DashboardWidgets => "dashboard.widgets",
        }
    }

    /// 全部插槽（用于帮助文档与校验）。
    pub fn all() -> &'static [Slot] {
        &[
            Slot::SidebarTop,
            Slot::SidebarBottom,
            Slot::TopbarRight,
            Slot::ChatCorner,
            Slot::ComposerLeft,
            Slot::ComposerRight,
            Slot::ComposerFooter,
            Slot::ComposerAbove,
            Slot::ThreadTray,
            Slot::ThreadApproval,
            Slot::TranscriptAfter,
            Slot::ExtensionsList,
            Slot::DashboardWidgets,
        ]
    }

    /// 解析插槽名；未知名字直接报错，避免插件"挂上去却没生效"还查不出原因。
    pub fn parse(raw: &str) -> Result<Slot> {
        let normalized = raw.trim();
        Slot::all()
            .iter()
            .copied()
            .find(|slot| slot.as_str() == normalized)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "未知的插件插槽 `{normalized}`，可用：{}",
                    Slot::all()
                        .iter()
                        .map(|slot| slot.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                )
            })
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 插件清单里的一条插槽声明。
#[derive(Debug, Clone, Deserialize)]
pub struct SlotEntry {
    /// 挂载点（见 [`Slot`]）。
    pub slot: String,
    /// 相对插件根目录的 TSX 模板。
    pub template: String,
    /// 同一插槽内的排序，小的在前。
    #[serde(default)]
    pub order: i32,
}

impl SlotEntry {
    pub fn parsed_slot(&self) -> Result<Slot> {
        Slot::parse(&self.slot)
    }
}

/// 导航项。
#[derive(Debug, Clone, Deserialize)]
pub struct NavEntry {
    pub label: String,
    pub slug: String,
}

/// 独立页面。
#[derive(Debug, Clone, Deserialize)]
pub struct PageEntry {
    pub slug: String,
    pub title: String,
    pub template: String,
}

/// 插件 API 端点的 HTTP 方法。刻意的白名单：插件 API 是同源数据面，
/// 不需要 PUT/DELETE 之类的全套 REST（读写都要过宿主的语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiMethod {
    Get,
    Post,
}

impl ApiMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiMethod::Get => "get",
            ApiMethod::Post => "post",
        }
    }

    pub fn parse(raw: &str) -> Result<ApiMethod> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "get" => Ok(ApiMethod::Get),
            "post" => Ok(ApiMethod::Post),
            other => bail!("不支持的插件 API 方法 `{other}`（只接受 get / post）"),
        }
    }
}

/// 插件清单里的一条 API 端点声明。
///
/// 端点地址是 `/ext/{id}/api/{path}`，`path` 只接受静态段（小写字母/数字/`-`/`_`，
/// 可用 `/` 分层）；查询参数与请求体经 `props.api` 交给模板，路径参数刻意不支持——
/// 那会把分发器变成一台小型路由器，白名单校验的面积跟着翻倍。
#[derive(Debug, Clone, Deserialize)]
pub struct ApiEntry {
    pub method: String,
    /// 相对 `/ext/{id}/api/` 的路径，如 `info` 或 `compute/ratio`。
    pub path: String,
    /// 相对插件根目录的 TSX 模板；入口函数**返回什么就序列化什么**（不走 JSX）。
    pub template: String,
}

impl ApiEntry {
    pub fn parsed_method(&self) -> Result<ApiMethod> {
        ApiMethod::parse(&self.method)
    }

    pub fn validate(&self) -> Result<()> {
        self.parsed_method()?;
        let path = self.path.trim();
        if path.is_empty()
            || !path
                .split('/')
                .all(|segment| {
                    !segment.is_empty()
                        && segment
                            .chars()
                            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
                })
        {
            bail!(
                "插件 API 路径 `{}` 不合法（只接受小写字母/数字/-/_，用 / 分层，且不含空段）",
                self.path
            );
        }
        if !self.template.trim().ends_with(".tsx") {
            bail!("插件 API `{}` 的 template 必须是 .tsx", self.path);
        }
        Ok(())
    }
}

/// 统一包 `plugin.json` 的 `backend` 段。
///
/// 目前只声明“这个包有后端入口”；动作本身由后端代码的 `ada.registerAction`
/// 声明（`mode` 以后端为准，前端不能改）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendSection {
    /// 后端插件 id；缺省为 `global:<包目录名>`。一般不需要写。
    #[serde(default)]
    pub plugin_id: Option<String>,
}

/// 前端声明的一条后端动作绑定。
///
/// `read=true` 只表示**允许首屏预取**；真正的只读/写入由后端动作自己的 `mode`
/// 决定，后端会在 `intent=read` 时拒绝写动作。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrontendActionBinding {
    pub action: String,
    #[serde(default)]
    pub read: bool,
    /// 写入动作的交互提示（不是权限边界）。
    #[serde(default)]
    pub confirm: Option<String>,
}

/// 统一包 `plugin.json` 的 `frontend` 段：字段与旧版扁平清单一致，另加 `actions`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendSection {
    #[serde(default)]
    pub nav: Vec<NavEntry>,
    #[serde(default)]
    pub pages: Vec<PageEntry>,
    #[serde(default)]
    pub dashboard: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub slots: Vec<SlotEntry>,
    #[serde(default)]
    pub styles: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub api: Vec<ApiEntry>,
    /// 前端可调用的后端动作：键是 URL 里的 binding 名。
    #[serde(default)]
    pub actions: BTreeMap<String, FrontendActionBinding>,
}

/// 归一化后的前端声明：统一包取 `frontend` 段，旧包取顶层字段。
#[derive(Debug, Clone, Default)]
pub struct FrontendSpec {
    pub nav: Vec<NavEntry>,
    pub pages: Vec<PageEntry>,
    pub dashboard: Option<String>,
    pub assets: Option<String>,
    pub slots: Vec<SlotEntry>,
    pub styles: Vec<String>,
    pub capabilities: Vec<String>,
    pub scripts: Vec<String>,
    pub api: Vec<ApiEntry>,
    pub actions: BTreeMap<String, FrontendActionBinding>,
}

/// `plugin.json`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub nav: Vec<NavEntry>,
    #[serde(default)]
    pub pages: Vec<PageEntry>,
    #[serde(default)]
    pub dashboard: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    /// v0.6 新增：挂载点声明。
    #[serde(default)]
    pub slots: Vec<SlotEntry>,
    /// 插件 CSS（相对 assets 目录；只接受 `.css`，例如 `weather.css`）。
    #[serde(default)]
    pub styles: Vec<String>,
    /// 插件能力声明（目前只用于宿主动作/数据注入的显式授权）。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// 插件 JS（相对 assets 目录；只接受 `.js`，例如 `todo-card.js`）。
    ///
    /// 由服务端注入所有页面 `<head>`（`defer`），用于给 SSR 出来的插槽补交互。
    #[serde(default)]
    pub scripts: Vec<String>,
    /// 插件自带的 HTTP API 端点（`/ext/{id}/api/{path}`，TSX 处理器返回 JSON）。
    #[serde(default)]
    pub api: Vec<ApiEntry>,
    /// 统一包的前端声明。与顶层扁平字段**二选一**；同时写会被 `validate` 拒绝。
    #[serde(default)]
    pub frontend: Option<FrontendSection>,
    /// 统一包的后端声明。有这个段才允许前端调用后端动作。
    #[serde(default)]
    pub backend: Option<BackendSection>,
}

impl Manifest {
    /// 读取并校验清单。
    pub fn read(root: &Path) -> Result<Option<Manifest>> {
        let manifest_path = root.join("plugin.json");
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("读取 {} 失败", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .with_context(|| format!("解析 {} 失败", manifest_path.display()))?;
        manifest.validate()?;
        Ok(Some(manifest))
    }

    /// 归一化前端声明：统一包取 `frontend` 段，旧包取顶层字段。
    pub fn frontend_spec(&self) -> FrontendSpec {
        match &self.frontend {
            Some(frontend) => FrontendSpec {
                nav: frontend.nav.clone(),
                pages: frontend.pages.clone(),
                dashboard: frontend.dashboard.clone(),
                assets: frontend.assets.clone(),
                slots: frontend.slots.clone(),
                styles: frontend.styles.clone(),
                capabilities: frontend.capabilities.clone(),
                scripts: frontend.scripts.clone(),
                api: frontend.api.clone(),
                actions: frontend.actions.clone(),
            },
            None => FrontendSpec {
                nav: self.nav.clone(),
                pages: self.pages.clone(),
                dashboard: self.dashboard.clone(),
                assets: self.assets.clone(),
                slots: self.slots.clone(),
                styles: self.styles.clone(),
                capabilities: self.capabilities.clone(),
                scripts: self.scripts.clone(),
                api: self.api.clone(),
                actions: BTreeMap::new(),
            },
        }
    }

    /// 这个包是否有前端声明（统一包的 `frontend` 段，或旧版顶层字段）。
    pub fn has_frontend(&self) -> bool {
        self.frontend.is_some() || self.has_flat_frontend_fields()
    }

    /// 后端插件 id：显式 `backend.pluginId`，否则 `global:<包目录名>`。
    pub fn backend_plugin_id(&self, root: &Path) -> Option<String> {
        let backend = self.backend.as_ref()?;
        if let Some(id) = backend
            .plugin_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(id.to_string());
        }
        root.file_name()
            .and_then(|name| name.to_str())
            .map(|name| format!("global:{name}"))
    }

    fn has_flat_frontend_fields(&self) -> bool {
        !self.nav.is_empty()
            || !self.pages.is_empty()
            || self.dashboard.is_some()
            || self.assets.is_some()
            || !self.slots.is_empty()
            || !self.styles.is_empty()
            || !self.capabilities.is_empty()
            || !self.scripts.is_empty()
            || !self.api.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        let id = self.id.trim().to_ascii_lowercase();
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            bail!("插件 id 只能包含小写字母、数字与连字符");
        }
        if self.name.trim().is_empty() {
            bail!("插件 name 不能为空");
        }

        // 统一包把前端字段放在 `frontend`，旧包放在顶层；两处同时写会导致
        // “到底哪份生效”说不清，加载期直接拒绝。
        if self.frontend.is_some() && self.has_flat_frontend_fields() {
            bail!("统一插件请把前端声明放进 `frontend`，不要同时使用顶层 nav/pages/slots 等字段");
        }
        if self.backend.is_some() && self.frontend.is_none() && self.has_flat_frontend_fields() {
            bail!("声明了 `backend` 的统一插件，其前端声明也必须放进 `frontend`");
        }

        let frontend = self.frontend_spec();
        for entry in &frontend.slots {
            // 插槽名写错要在加载阶段就报错，而不是等到页面上"插件没出现"。
            entry.parsed_slot()?;
            if !entry.template.trim().ends_with(".tsx") {
                bail!("插槽 {} 的 template 必须是 .tsx", entry.slot);
            }
        }
        for page in &frontend.pages {
            if !page.template.trim().ends_with(".tsx") {
                bail!("页面 `{}` 的 template 必须是 .tsx", page.slug);
            }
        }
        if let Some(dashboard) = &frontend.dashboard {
            if !dashboard.trim().ends_with(".tsx") {
                bail!("dashboard 必须是 .tsx 文件");
            }
        }
        for style in &frontend.styles {
            validate_asset_path(style, ".css", "样式")?;
        }
        for capability in &frontend.capabilities {
            if !matches!(capability.as_str(), "providers.read" | "processes.read") {
                bail!("不支持的插件能力: {capability}");
            }
        }
        for script in &frontend.scripts {
            validate_asset_path(script, ".js", "脚本")?;
        }
        for entry in &frontend.api {
            entry.validate()?;
        }
        // 同一 (method, path) 声明两次是清单笔误，加载期就报出来
        let mut seen = HashSet::new();
        for entry in &frontend.api {
            if !seen.insert((entry.parsed_method().ok(), entry.path.trim().to_string())) {
                bail!("插件 API `{} {}` 重复声明", entry.method, entry.path);
            }
        }

        for (binding, action) in &frontend.actions {
            validate_binding_name(binding)?;
            validate_action_name(&action.action)?;
        }
        if !frontend.actions.is_empty() && self.backend.is_none() {
            bail!("`frontend.actions` 需要同时声明 `backend` 段（否则动作没有后端入口）");
        }
        if let Some(backend) = &self.backend {
            if let Some(plugin_id) = backend.plugin_id.as_deref() {
                let plugin_id = plugin_id.trim();
                if plugin_id.is_empty()
                    || !plugin_id.contains(':')
                    || plugin_id.chars().any(char::is_whitespace)
                {
                    bail!("`backend.pluginId` 必须是非空的 `scope:name` 形式");
                }
            }
        }
        Ok(())
    }
}

/// binding 名走 URL 段：小写字母/数字/`-`/`_`，首字符必须是字母或数字。
fn validate_binding_name(raw: &str) -> Result<()> {
    let name = raw.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        bail!("动作 binding 名不合法: {name}（小写字母/数字/-/_，首字符为字母或数字）");
    }
    Ok(())
}

/// 后端动作名允许下划线（与 binding 名同一字符集，但更宽松的长度上限）。
fn validate_action_name(raw: &str) -> Result<()> {
    let name = raw.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        bail!("后端动作名不合法: {name}");
    }
    Ok(())
}

/// 插件资源路径校验（CSS / JS 共用一套口径）。
fn validate_asset_path(relative: &str, extension: &str, label: &str) -> Result<()> {
    let relative = relative.trim();
    if relative.is_empty() || !relative.ends_with(extension) {
        bail!("插件{label}必须是 {extension} 文件");
    }
    if relative.starts_with('/')
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part == ".." || part.is_empty())
    {
        bail!("插件{label}路径不合法: {relative}");
    }
    if !relative
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
    {
        bail!("插件{label}路径只能包含字母、数字、_ - . /: {relative}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(template: &str) -> Manifest {
        serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "version": "0.1.0",
            "pages": [{ "slug": "home", "title": "Home", "template": template }],
        }))
        .expect("manifest")
    }

    #[test]
    fn only_tsx_templates_are_accepted() {
        assert!(manifest("page.tsx").validate().is_ok());
        let error = manifest("page.slang").validate().unwrap_err();
        assert!(format!("{error}").contains(".tsx"), "{error}");
        let error = manifest("page.html").validate().unwrap_err();
        assert!(format!("{error}").contains(".tsx"), "{error}");
    }

    fn manifest_with_styles(styles: Vec<&str>) -> Manifest {
        serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "version": "0.1.0",
            "pages": [{ "slug": "home", "title": "Home", "template": "page.tsx" }],
            "styles": styles,
        }))
        .expect("manifest")
    }

    #[test]
    fn styles_must_be_relative_css_files() {
        assert!(
            manifest_with_styles(vec!["weather.css", "nested/badge.css"])
                .validate()
                .is_ok()
        );
        for bad in ["style.scss", "../style.css", "/style.css", "a//b.css"] {
            let error = manifest_with_styles(vec![bad]).validate().unwrap_err();
            assert!(format!("{error}").contains("样式"), "{bad}: {error}");
        }
    }

    fn manifest_with_scripts(scripts: Vec<&str>) -> Manifest {
        serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "version": "0.1.0",
            "pages": [{ "slug": "home", "title": "Home", "template": "page.tsx" }],
            "scripts": scripts,
        }))
        .expect("manifest")
    }

    #[test]
    fn scripts_must_be_relative_js_files() {
        // 插件 JS 由服务端注入所有页面，所以路径口径与样式完全一致。
        assert!(manifest_with_scripts(vec!["todo.js", "lib/card.js"])
            .validate()
            .is_ok());
        for bad in ["todo.ts", "../todo.js", "/todo.js", "a//b.js"] {
            let error = manifest_with_scripts(vec![bad]).validate().unwrap_err();
            assert!(format!("{error}").contains("脚本"), "{bad}: {error}");
        }
    }

    #[test]
    fn chat_corner_is_a_known_slot() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "slots": [{ "slot": "chat.corner", "template": "slot.tsx" }],
        }))
        .expect("manifest");
        assert!(manifest.validate().is_ok());
        assert_eq!(Slot::parse("chat.corner").unwrap(), Slot::ChatCorner);
    }
}

#[test]
fn capabilities_are_an_explicit_allowlist() {
    let mut ok: Manifest = serde_json::from_value(serde_json::json!({
        "id": "demo",
        "name": "Demo",
        "version": "0.1.0",
        "capabilities": ["providers.read"],
        "pages": [{ "slug": "home", "title": "Home", "template": "page.tsx" }],
    }))
    .expect("manifest");
    assert!(ok.validate().is_ok());
    ok.capabilities.push("providers.write".to_string());
    assert!(ok.validate().is_err());
}

#[test]
fn api_entries_are_validated() {
    let ok: Manifest = serde_json::from_value(serde_json::json!({
        "id": "demo",
        "name": "Demo",
        "api": [
            { "method": "get", "path": "info", "template": "api/info.tsx" },
            { "method": "post", "path": "compute/ratio", "template": "api/echo.tsx" },
        ],
    }))
    .expect("manifest");
    assert!(ok.validate().is_ok());

    // 路径穿越 / 大写 / 空段都拒绝
    for bad in ["../etc", "Info", "a//b", "/abs"] {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "api": [{ "method": "get", "path": bad, "template": "api/info.tsx" }],
        }))
        .expect("manifest");
        assert!(manifest.validate().is_err(), "path `{bad}` 应被拒绝");
    }

    // 方法白名单：只接受 get / post
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "demo",
        "name": "Demo",
        "api": [{ "method": "delete", "path": "info", "template": "api/info.tsx" }],
    }))
    .expect("manifest");
    assert!(manifest.validate().is_err());

    // 同一 (method, path) 重复声明是清单笔误
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "demo",
        "name": "Demo",
        "api": [
            { "method": "get", "path": "info", "template": "api/a.tsx" },
            { "method": "get", "path": "info", "template": "api/b.tsx" },
        ],
    }))
    .expect("manifest");
    assert!(manifest.validate().is_err());
}

#[test]
fn unified_manifest_keeps_frontend_and_backend_together() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "unified-demo",
        "name": "Unified Demo",
        "version": "0.1.0",
        "frontend": {
            "slots": [{ "slot": "sidebar.bottom", "template": "slot.tsx" }],
            "actions": {
                "status": { "action": "status", "read": true },
                "refresh": { "action": "refresh", "confirm": "刷新？" }
            }
        },
        "backend": {}
    }))
    .expect("manifest");
    assert!(manifest.validate().is_ok());
    assert!(manifest.has_frontend());
    assert_eq!(
        manifest.backend_plugin_id(Path::new("/tmp/unified-demo")),
        Some("global:unified-demo".to_string())
    );
    let frontend = manifest.frontend_spec();
    assert_eq!(frontend.slots.len(), 1);
    assert_eq!(frontend.actions.len(), 2);
    assert!(frontend.actions["status"].read);
    assert!(!frontend.actions["refresh"].read);
}

#[test]
fn backend_only_unified_manifest_has_no_frontend() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "backend-only",
        "name": "Backend Only",
        "backend": {}
    }))
    .expect("manifest");
    assert!(manifest.validate().is_ok());
    assert!(!manifest.has_frontend());
}

#[test]
fn unified_manifest_rejects_mixed_flat_and_nested_frontend() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "mixed",
        "name": "Mixed",
        "pages": [{ "slug": "p", "title": "P", "template": "page.tsx" }],
        "frontend": {},
        "backend": {}
    }))
    .expect("manifest");
    assert!(manifest.validate().is_err());
}

#[test]
fn frontend_actions_require_a_backend_section() {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "no-backend",
        "name": "No Backend",
        "frontend": {
            "actions": { "status": { "action": "status", "read": true } }
        }
    }))
    .expect("manifest");
    let error = manifest.validate().unwrap_err().to_string();
    assert!(error.contains("backend"), "{error}");
}

#[test]
fn action_binding_names_are_url_safe() {
    for bad in ["", "Status", "a b", "-status", "../status"] {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "demo",
            "name": "Demo",
            "frontend": {
                "actions": { bad: { "action": "status", "read": true } }
            },
            "backend": {}
        }))
        .expect("manifest");
        assert!(manifest.validate().is_err(), "binding `{bad}` 应被拒绝");
    }
}

/// 仓库自带插件必须使用统一清单形状；旧扁平形状只保留兼容。
#[test]
fn bundled_plugin_manifests_use_the_unified_shape() {
    let roots = [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins"),
    ];
    let mut checked = 0;
    for root in roots {
        let entries = std::fs::read_dir(&root).expect("插件目录必须存在");
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("plugin.json");
            if !manifest_path.is_file() {
                continue;
            }
            let raw = std::fs::read_to_string(&manifest_path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let has_frontend = value.get("frontend").is_some_and(|frontend| frontend.is_object());
            let has_backend = value.get("backend").is_some_and(|backend| backend.is_object());
            if !has_frontend {
                assert!(
                    has_backend,
                    "{} 既没有 frontend 也没有 backend",
                    manifest_path.display()
                );
                continue;
            }
            for legacy in [
                "nav",
                "pages",
                "dashboard",
                "assets",
                "slots",
                "styles",
                "capabilities",
                "scripts",
                "api",
            ] {
                assert!(
                    value.get(legacy).is_none(),
                    "{} 仍在使用顶层 `{legacy}`；请收进 frontend",
                    manifest_path.display()
                );
            }
            let manifest = Manifest::read(&entry.path()).unwrap().expect("manifest");
            manifest.validate().unwrap();
            checked += 1;
        }
    }
    assert!(checked >= 7, "至少要检查到 7 个自带前端插件，实际 {checked}");
}
