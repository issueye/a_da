//! 前端插件的清单格式。
//!
//! 在原有能力（导航 / 独立页面 / 仪表盘卡片 / 静态资源）之上，
//! v0.6 增加了**插槽**（slots）：插件可以把 HTML 片段挂到界面的固定位置上。

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
    /// 相对插件根目录的 HTML 片段。
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
            if entry.template.trim().is_empty() {
                bail!("插槽 {} 的 template 不能为空", entry.slot);
            }
        }
        Ok(())
    }
}
