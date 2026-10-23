//! 前端插件的清单格式。
//!
//! 在原有能力（导航 / 独立页面 / 仪表盘卡片 / 静态资源）之上，
//! 插件模板只接受 `.tsx`；页面、插槽、仪表盘卡片都由 TSX 在服务端渲染。

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
        for entry in &self.slots {
            // 插槽名写错要在加载阶段就报错，而不是等到页面上"插件没出现"。
            entry.parsed_slot()?;
            if !entry.template.trim().ends_with(".tsx") {
                bail!("插槽 {} 的 template 必须是 .tsx", entry.slot);
            }
        }
        for page in &self.pages {
            if !page.template.trim().ends_with(".tsx") {
                bail!("页面 `{}` 的 template 必须是 .tsx", page.slug);
            }
        }
        if let Some(dashboard) = &self.dashboard {
            if !dashboard.trim().ends_with(".tsx") {
                bail!("dashboard 必须是 .tsx 文件");
            }
        }
        for style in &self.styles {
            validate_asset_path(style, ".css", "样式")?;
        }
        for capability in &self.capabilities {
            if !matches!(capability.as_str(), "providers.read") {
                bail!("不支持的插件能力: {capability}");
            }
        }
        for script in &self.scripts {
            validate_asset_path(script, ".js", "脚本")?;
        }
        Ok(())
    }
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
