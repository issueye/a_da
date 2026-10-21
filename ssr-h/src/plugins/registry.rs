//! 插件注册表：插槽、生命周期、失败隔离与热插拔。
//!
//! 生命周期（每个插件独立走一遍，任何一步失败都只影响它自己）：
//!
//! ```text
//! register → load → mount → render → unmount → destroy
//!    │        │       │        │
//!    └────────┴───────┴────────┴── 任一步失败：标记 Failed，降级为"不贡献内容"
//! ```
//!
//! - **register**：读清单、校验 id/插槽名；清单坏掉 → `Failed`，不影响其它插件；
//! - **load**：把插槽/页面/卡片片段读进内存缓存；某个片段读失败 → 该插槽降级为空；
//! - **mount**：进入注册表的插槽表，开始对页面可见；
//! - **render**：SSR 期把各插件的片段拼进插槽，**逐插件隔离**（panic 也会被兜住）；
//! - **unmount**：停用或重载时摘掉贡献，界面回到默认实现；
//! - **destroy**：释放缓存与资源（插件目录被删掉时走这条）。
//!
//! 热插拔：`reload()` 重新扫描目录并与当前状态做差分——
//! 新增的注册、有变更的重载、消失的销毁、停用的保持停用。

use std::collections::HashMap;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, bail, Context, Result};

use crate::slang::{
    self, AppMeta, CompiledTemplate, Engine, PageMeta, PluginMeta, RenderContext, SlotMeta, Trust,
};

use super::manifest::{Manifest, NavEntry, PageEntry, Slot, SlotEntry};

/// 单个片段的上限，防止插件把整页撑爆。
const MAX_PARTIAL_BYTES: u64 = 512 * 1024;

/// 插件状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// 正常加载，可贡献内容。
    Ready,
    /// 被手动停用（贡献已卸载，界面回到默认实现）。
    Disabled,
    /// 加载失败，`Failed` 里是原因。
    Failed(String),
}

impl PluginStatus {
    pub fn as_str(&self) -> &str {
        match self {
            PluginStatus::Ready => "ready",
            PluginStatus::Disabled => "disabled",
            PluginStatus::Failed(_) => "failed",
        }
    }
}

/// 一个已注册的插件。
#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub status: PluginStatus,
    pub slots: Vec<SlotEntry>,
    pub nav: Vec<NavEntry>,
    pub pages: Vec<PageEntry>,
    pub dashboard: Option<String>,
    pub root: PathBuf,
    pub assets_dir: Option<PathBuf>,
    /// 加载期/运行期的问题（逐条展示，不吞异常）。
    pub errors: Vec<String>,
    /// 片段缓存：`slot|template` → HTML。
    fragments: HashMap<String, String>,
    /// Slang 编译产物：模板相对路径 → 编译后的 AST。
    pub slang_templates: HashMap<String, Arc<CompiledTemplate>>,
    /// 清单声明的组合组件文件（已编译的也在 slang_templates 里）。
    pub components: Vec<String>,
    /// 清单与片段内容的指纹，用来判断"插件是不是改过了"。
    fingerprint: u64,
}

impl PluginRecord {
    /// 插件是否能为页面贡献内容。
    pub fn is_active(&self) -> bool {
        self.enabled && matches!(self.status, PluginStatus::Ready)
    }

    /// 片段缓存键。
    fn fragment_key(slot: &str, template: &str) -> String {
        format!("{slot}|{template}")
    }
}

/// 插槽渲染结果。
#[derive(Debug, Clone, Default)]
pub struct SlotOutput {
    /// 拼好的 HTML。
    pub html: String,
    /// 参与贡献的插件数。
    pub contributors: usize,
    /// 贡献失败的插件（已隔离，界面照常）。
    pub failed: Vec<String>,
}

/// 插件注册表。
pub struct PluginRegistry {
    dir: PathBuf,
    engine: Arc<Engine>,
    plugins: RwLock<Vec<PluginRecord>>,
}

/// 一次重载的差分结果。
#[derive(Debug, Clone, Default)]
pub struct ReloadReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<String>,
    pub total: usize,
}

/// 插件状态快照（`/api/plugins`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginSnapshot {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub status: String,
    pub slots: Vec<String>,
    pub errors: Vec<String>,
    /// `slang` / `html`：这个插件是否使用了运行时组件语言。
    pub engine: String,
    /// 已编译的 Slang 模板数。
    pub slang_compiled: usize,
}

/// 目录名 → 插件 id（只用于"加载失败的插件"占位，正常插件以清单 id 为准）。
fn dir_plugin_id(root: &Path) -> String {
    let raw = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    let mut id = String::new();
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() || lower == '-' {
            id.push(lower);
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let trimmed = id.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "invalid".to_string()
    } else {
        trimmed
    }
}

/// 加载失败的插件占位：没有贡献，但**出现在列表里并带原因**。
fn failed_record(root: &Path, id: &str, message: &str) -> PluginRecord {
    PluginRecord {
        id: id.to_string(),
        name: id.to_string(),
        version: String::new(),
        description: String::new(),
        enabled: true,
        status: PluginStatus::Failed(message.to_string()),
        slots: Vec::new(),
        nav: Vec::new(),
        pages: Vec::new(),
        dashboard: None,
        root: root.to_path_buf(),
        assets_dir: None,
        errors: vec![message.to_string()],
        fragments: HashMap::new(),
        slang_templates: HashMap::new(),
        components: Vec::new(),
        fingerprint: 0,
    }
}

impl PluginRegistry {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::with_engine(dir, Arc::new(slang::default_engine()))
    }

    pub fn with_engine(dir: impl Into<PathBuf>, engine: Arc<Engine>) -> Self {
        let dir = dir.into();
        let registry = Self {
            dir,
            engine,
            plugins: RwLock::new(Vec::new()),
        };
        if let Err(error) = registry.reload() {
            tracing::warn!("扫描前端插件目录失败: {error:#}");
        }
        registry
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    // ── 生命周期 ──────────────────────────────────────────────────────────

    /// 重新扫描目录并做差分：新增 → 注册，更新 → 重载，消失 → 销毁。
    pub fn reload(&self) -> Result<ReloadReport> {
        let mut report = ReloadReport::default();
        let entries = fs::read_dir(&self.dir)
            .with_context(|| format!("读取前端插件目录失败: {}", self.dir.display()))?;

        let mut discovered: Vec<PluginRecord> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match self.register(&path) {
                Ok(Some(record)) => discovered.push(record),
                Ok(None) => {}
                Err(error) => {
                    // 坏插件不能"静默消失"：保留成 Failed 记录，让扩展页把原因显示出来。
                    let id = dir_plugin_id(&path);
                    let message = format!("{error:#}");
                    tracing::warn!("前端插件 {id} 加载失败: {message}");
                    discovered.push(failed_record(&path, &id, &message));
                    report.failed.push(id);
                }
            }
        }
        discovered.sort_by(|a, b| a.id.cmp(&b.id));

        let mut guard = self
            .plugins
            .write()
            .map_err(|_| anyhow!("插件注册表锁异常"))?;
        let previous = std::mem::take(&mut *guard);

        for mut record in discovered {
            match previous.iter().find(|old| old.id == record.id) {
                Some(old) => {
                    // 清单没变就保留原状态（用户手动停用的插件不能被"悄悄恢复"）。
                    if old.fingerprint == record.fingerprint {
                        record.enabled = old.enabled;
                        if !record.enabled {
                            record.status = PluginStatus::Disabled;
                        }
                        report.total += 1;
                    } else {
                        // 有变更：卸载旧贡献 → 装载新内容 → 重新挂载。
                        record.enabled = old.enabled;
                        if !record.enabled {
                            record.status = PluginStatus::Disabled;
                        }
                        report.updated.push(record.id.clone());
                        report.total += 1;
                    }
                }
                None => {
                    report.added.push(record.id.clone());
                    report.total += 1;
                }
            }
            guard.push(record);
        }

        for old in previous {
            if !guard.iter().any(|record| record.id == old.id) {
                // 目录里没有了 → destroy（缓存与贡献一并消失）。
                report.removed.push(old.id.clone());
            }
        }
        Ok(report)
    }

    /// 读清单并装载片段，生成一条插件记录。
    fn register(&self, root: &Path) -> Result<Option<PluginRecord>> {
        let manifest_path = root.join("plugin.json");
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("读取 {} 失败", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .with_context(|| format!("解析 {} 失败", manifest_path.display()))?;
        manifest.validate().context("插件清单校验失败")?;

        let root = root
            .canonicalize()
            .with_context(|| format!("规范化插件目录失败: {}", root.display()))?;
        let assets_dir = resolve_assets_dir(&root, manifest.assets.as_deref());

        let mut record = PluginRecord {
            id: manifest.id.trim().to_ascii_lowercase(),
            name: manifest.name.trim().to_string(),
            version: manifest.version.trim().to_string(),
            description: manifest.description.trim().to_string(),
            enabled: true,
            status: PluginStatus::Ready,
            slots: manifest.slots,
            nav: manifest.nav,
            pages: manifest.pages,
            dashboard: manifest.dashboard.clone(),
            components: manifest.components.clone(),
            root,
            assets_dir,
            errors: Vec::new(),
            fragments: HashMap::new(),
            slang_templates: HashMap::new(),
            fingerprint: 0,
        };

        // 1) 编译所有 `.slang` 模板（插槽 / 页面 / 仪表盘 / 组合组件）。
        let mut slang_paths: Vec<String> = Vec::new();
        for entry in &record.slots {
            if is_slang(&entry.template) {
                slang_paths.push(entry.template.clone());
            }
        }
        for page in &record.pages {
            if is_slang(&page.template) {
                slang_paths.push(page.template.clone());
            }
        }
        if let Some(dashboard) = &record.dashboard {
            if is_slang(dashboard) {
                slang_paths.push(dashboard.clone());
            }
        }
        for component in &record.components {
            if is_slang(component) {
                slang_paths.push(component.clone());
            }
        }
        slang_paths.sort();
        slang_paths.dedup();
        for path in &slang_paths {
            let full = record.root.join(path);
            match self.engine.compile_file(&full, &record.root, Trust::Local) {
                Ok(template) => {
                    record.slang_templates.insert(path.clone(), template);
                }
                Err(error) => {
                    let message = format!("Slang 模板 `{path}` 编译失败：{}", error.render());
                    tracing::warn!("插件 {} {message}", record.id);
                    if !record.errors.contains(&message) {
                        record.errors.push(message);
                    }
                }
            }
        }

        // 2) `.html` 插槽仍走原始片段；`.slang` 插槽不读成字符串。
        for entry in record.slots.clone() {
            if is_slang(&entry.template) {
                continue;
            }
            let key = PluginRecord::fragment_key(&entry.slot, &entry.template);
            match read_fragment(&record.root, &record.root, &entry.template) {
                Ok(html) => {
                    record.fragments.insert(key, html);
                }
                Err(error) => {
                    let message = format!("插槽 {} 的片段读取失败: {error:#}", entry.slot);
                    tracing::warn!("插件 {} {message}", record.id);
                    record.errors.push(message);
                }
            }
        }

        // 3) 指纹包含清单与所有 `.slang`/`.html`/`.json` 文件：
        //    @include/@use 的依赖变更也能触发重载。
        record.fingerprint = plugin_fingerprint(&record.root, raw.as_bytes());
        Ok(Some(record))
    }

    /// 停用 / 启用插件（热插拔的最小单元）。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut guard = self
            .plugins
            .write()
            .map_err(|_| anyhow!("插件注册表锁异常"))?;
        let record = guard
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| anyhow!("前端插件不存在: {id}"))?;
        if let PluginStatus::Failed(reason) = &record.status {
            bail!("插件 {id} 加载失败，无法启用：{reason}");
        }
        record.enabled = enabled;
        record.status = if enabled {
            PluginStatus::Ready
        } else {
            PluginStatus::Disabled
        };
        Ok(())
    }

    // ── 渲染 ──────────────────────────────────────────────────────────────

    /// 渲染一个插槽：逐插件隔离，失败只跳过它自己。
    pub fn render_slot(&self, slot: Slot) -> SlotOutput {
        self.render_slot_with(slot, &RenderContext::default())
    }

    /// 带上下文的插槽渲染：`.slang` 片段可以读到 `plugin.*` / `slot.*` / `app.*`。
    pub fn render_slot_with(&self, slot: Slot, cx: &RenderContext) -> SlotOutput {
        let mut output = SlotOutput::default();
        let Ok(guard) = self.plugins.read() else {
            return output;
        };
        let mut contributions: Vec<(i32, String, String)> = Vec::new();
        for record in guard.iter() {
            if !record.is_active() {
                continue;
            }
            let mut entries: Vec<&SlotEntry> = record
                .slots
                .iter()
                .filter(|entry| entry.parsed_slot().ok() == Some(slot))
                .collect();
            entries.sort_by_key(|entry| entry.order);
            for (index, entry) in entries.into_iter().enumerate() {
                let html = if let Some(template) = record.slang_templates.get(&entry.template) {
                    let mut plugin_cx = cx.clone();
                    plugin_cx.plugin = Some(PluginMeta {
                        id: record.id.clone(),
                        name: record.name.clone(),
                        version: record.version.clone(),
                        description: record.description.clone(),
                    });
                    plugin_cx.slot = Some(SlotMeta {
                        name: slot.as_str().to_string(),
                        order: entry.order,
                        index,
                    });
                    match catch_unwind(AssertUnwindSafe(|| {
                        self.engine.render(template, &plugin_cx)
                    })) {
                        Ok(Ok(html)) => html,
                        Ok(Err(error)) => {
                            output.failed.push(record.id.clone());
                            tracing::warn!(
                                "插件 {} 插槽 {} 渲染失败: {}",
                                record.id,
                                slot.as_str(),
                                error.render()
                            );
                            continue;
                        }
                        Err(_) => {
                            output.failed.push(record.id.clone());
                            tracing::warn!(
                                "插件 {} 插槽 {} 渲染 panic，已隔离",
                                record.id,
                                slot.as_str()
                            );
                            continue;
                        }
                    }
                } else {
                    let key = PluginRecord::fragment_key(&entry.slot, &entry.template);
                    match record.fragments.get(&key) {
                        Some(html) => html.clone(),
                        None => {
                            output.failed.push(record.id.clone());
                            continue;
                        }
                    }
                };
                if html.is_empty() {
                    continue;
                }
                contributions.push((entry.order, record.id.clone(), html));
            }
        }
        drop(guard);
        contributions.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _id, html) in contributions {
            output.contributors += 1;
            output.html.push_str(&html);
        }
        output
    }

    // ── 既有能力（导航 / 页面 / 卡片 / 资源） ────────────────────────────

    pub fn list(&self) -> Vec<PluginRecord> {
        self.plugins.read().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.plugins.read().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn find(&self, id: &str) -> Option<PluginRecord> {
        self.plugins
            .read()
            .ok()
            .and_then(|g| g.iter().find(|p| p.id == id).cloned())
    }

    /// 插件状态快照，供 `/api/plugins` 与客户端使用。
    pub fn snapshot(&self) -> Vec<PluginSnapshot> {
        self.list()
            .into_iter()
            .map(|record| PluginSnapshot {
                id: record.id,
                name: record.name,
                version: record.version,
                description: record.description,
                enabled: record.enabled,
                status: record.status.as_str().to_string(),
                slots: record
                    .slots
                    .iter()
                    .map(|entry| entry.slot.clone())
                    .collect(),
                errors: record.errors,
                engine: if record.slang_templates.is_empty() {
                    "html".to_string()
                } else {
                    "slang".to_string()
                },
                slang_compiled: record.slang_templates.len(),
            })
            .collect()
    }

    pub fn nav_links(&self) -> Vec<NavLink> {
        let mut links = Vec::new();
        for plugin in self.list() {
            if !plugin.is_active() {
                continue;
            }
            for nav in &plugin.nav {
                links.push(NavLink {
                    plugin_id: plugin.id.clone(),
                    label: nav.label.clone(),
                    href: format!("/ext/{}/{}", plugin.id, nav.slug),
                });
            }
        }
        links
    }

    pub fn dashboard_widgets(&self) -> Vec<PluginWidget> {
        self.dashboard_widgets_with(&RenderContext::default())
    }

    pub fn dashboard_widgets_with(&self, cx: &RenderContext) -> Vec<PluginWidget> {
        let mut widgets = Vec::new();
        for plugin in self.list() {
            if !plugin.is_active() {
                continue;
            }
            let Some(rel) = plugin.dashboard.as_deref() else {
                continue;
            };
            let html = if is_slang(rel) {
                let Some(template) = plugin.slang_templates.get(rel) else {
                    tracing::warn!("插件 {} 的 Slang 卡片未编译成功", plugin.id);
                    continue;
                };
                let mut plugin_cx = cx.clone();
                plugin_cx.plugin = Some(PluginMeta {
                    id: plugin.id.clone(),
                    name: plugin.name.clone(),
                    version: plugin.version.clone(),
                    description: plugin.description.clone(),
                });
                match catch_unwind(AssertUnwindSafe(|| {
                    self.engine.render(template, &plugin_cx)
                })) {
                    Ok(Ok(html)) => html,
                    Ok(Err(error)) => {
                        tracing::warn!(
                            "插件 {} 的 Slang 卡片渲染失败: {}",
                            plugin.id,
                            error.render()
                        );
                        continue;
                    }
                    Err(_) => {
                        tracing::warn!("插件 {} 的 Slang 卡片渲染 panic，已隔离", plugin.id);
                        continue;
                    }
                }
            } else {
                match read_fragment(&plugin.root, &plugin.root, rel) {
                    Ok(html) => html,
                    Err(error) => {
                        tracing::warn!("读取前端插件卡片失败 {}: {error:#}", plugin.id);
                        continue;
                    }
                }
            };
            widgets.push(PluginWidget {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
                html,
            });
        }
        widgets
    }

    pub fn read_page(&self, plugin: &PluginRecord, slug: &str) -> Result<(String, String)> {
        self.read_page_with(plugin, slug, &RenderContext::default())
    }

    pub fn read_page_with(
        &self,
        plugin: &PluginRecord,
        slug: &str,
        cx: &RenderContext,
    ) -> Result<(String, String)> {
        // 停用/加载失败的插件不再对外提供页面：否则"停用后回到默认实现"只是把
        // 导航藏了，直链仍然能打开。
        if !plugin.is_active() {
            bail!("插件未启用或加载失败: {}", plugin.id);
        }
        let page = plugin
            .pages
            .iter()
            .find(|page| page.slug == slug)
            .ok_or_else(|| anyhow!("插件页面不存在: {}/{}", plugin.id, slug))?;
        let html = if is_slang(&page.template) {
            let template = plugin.slang_templates.get(&page.template).ok_or_else(|| {
                anyhow!(
                    "插件页面 `{}` 的 Slang 模板未编译成功，见扩展页诊断",
                    page.template
                )
            })?;
            let mut plugin_cx = cx.clone();
            plugin_cx.plugin = Some(PluginMeta {
                id: plugin.id.clone(),
                name: plugin.name.clone(),
                version: plugin.version.clone(),
                description: plugin.description.clone(),
            });
            plugin_cx.page = Some(PageMeta {
                slug: page.slug.clone(),
                title: page.title.clone(),
            });
            self.engine.render(template, &plugin_cx).map_err(|error| {
                anyhow!("插件页面 `{}` 渲染失败：{}", page.template, error.render())
            })?
        } else {
            read_fragment(&plugin.root, &plugin.root, &page.template)?
        };
        Ok((page.title.clone(), html))
    }

    pub fn asset_path(&self, plugin_id: &str, relative: &str) -> Result<(PathBuf, String)> {
        let plugin = self
            .find(plugin_id)
            .ok_or_else(|| anyhow!("前端插件不存在: {plugin_id}"))?;
        if !plugin.is_active() {
            bail!("插件未启用或加载失败: {plugin_id}");
        }
        let assets = plugin
            .assets_dir
            .as_ref()
            .ok_or_else(|| anyhow!("插件没有 assets 目录: {plugin_id}"))?;
        let relative = relative.trim_start_matches('/');
        let path = safe_join(&plugin.root, assets, relative)?;
        if !path.is_file() {
            bail!("插件资源不存在: {plugin_id}/{relative}");
        }
        let mime = content_type(&path);
        Ok((path, mime))
    }
}

/// 侧栏导航链接。
#[derive(Debug, Clone)]
pub struct NavLink {
    pub plugin_id: String,
    pub label: String,
    pub href: String,
}

/// 仪表盘卡片。
#[derive(Debug, Clone)]
pub struct PluginWidget {
    pub plugin_id: String,
    pub plugin_name: String,
    pub html: String,
}

// ── 工具函数 ──────────────────────────────────────────────────────────────

/// 是否是 Slang 模板（按扩展名分派）。
fn is_slang(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("slang"))
}

/// 插件指纹：清单 + 插件目录里所有 `.slang` / `.html` / `.json` 文件。
///
/// 这样 `@include` / `@use` 的依赖变更也会触发重载，而不是只盯清单和插槽片段。
fn plugin_fingerprint(root: &Path, manifest: &[u8]) -> u64 {
    let mut hash = fnv1a_init(manifest);
    let mut files = Vec::new();
    collect_fingerprint_files(root, &mut files, 0);
    files.sort();
    for path in files {
        if let Ok(relative) = path.strip_prefix(root) {
            hash = fnv1a(hash, relative.to_string_lossy().as_bytes());
        }
        if let Ok(bytes) = fs::read(&path) {
            hash = fnv1a(hash, &bytes);
        }
    }
    hash
}

fn collect_fingerprint_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 || out.len() > 256 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fingerprint_files(&path, out, depth + 1);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                matches!(ext.to_ascii_lowercase().as_str(), "slang" | "html" | "json")
            })
        {
            out.push(path);
        }
    }
}

fn resolve_assets_dir(root: &Path, declared: Option<&str>) -> Option<PathBuf> {
    match declared.map(str::trim).filter(|s| !s.is_empty()) {
        Some(rel) => {
            let dir = safe_join(root, root, rel).ok()?;
            dir.is_dir().then_some(dir)
        }
        None => {
            let default = root.join("static");
            default
                .is_dir()
                .then(|| default.canonicalize().unwrap_or(default))
        }
    }
}

fn read_fragment(plugin_root: &Path, base: &Path, relative: &str) -> Result<String> {
    let path = safe_join(plugin_root, base, relative)?;
    let meta =
        fs::metadata(&path).with_context(|| format!("读取插件文件失败: {}", path.display()))?;
    if meta.len() > MAX_PARTIAL_BYTES {
        bail!("插件 HTML 片段过大（上限 {} KB）", MAX_PARTIAL_BYTES / 1024);
    }
    fs::read_to_string(&path).with_context(|| format!("读取插件文件失败: {}", path.display()))
}

/// `base` 是允许的根；`relative` 不允许绝对路径、`..`、Windows 前缀。
fn safe_join(plugin_root: &Path, base: &Path, relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("非法的插件相对路径: {relative}");
    }
    let joined = base.join(rel);
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("插件路径不存在: {}", joined.display()))?;
    if !canonical.starts_with(plugin_root) {
        bail!("插件路径越界: {relative}");
    }
    Ok(canonical)
}

fn content_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// FNV-1a：给清单算指纹，用来判断插件有没有变。
fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 初始指纹。
fn fnv1a_init(bytes: &[u8]) -> u64 {
    fnv1a(0xcbf2_9ce4_8422_2325, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "ssr_h_plugin_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_demo_plugin(root: &Path) {
        let plugin = root.join("demo");
        fs::create_dir_all(plugin.join("static")).unwrap();
        fs::write(
            plugin.join("plugin.json"),
            r#"{
                "id": "demo",
                "name": "Demo",
                "pages": [{"slug":"p","title":"P","template":"page.html"}],
                "assets": "static"
            }"#,
        )
        .unwrap();
        fs::write(plugin.join("page.html"), "<p>page</p>").unwrap();
        fs::write(plugin.join("static").join("x.js"), "console.log(1);").unwrap();
    }

    #[test]
    fn broken_plugin_is_visible_as_failed_instead_of_silently_disappearing() {
        let root = temp_root("broken");
        let plugin = root.join("broken");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.json"),
            r#"{"id":"broken","name":"Broken","slots":[{"slot":"nope.slot","template":"x.html"}]}"#,
        )
        .unwrap();
        fs::write(plugin.join("x.html"), "<p>x</p>").unwrap();

        let registry = PluginRegistry::new(&root);
        let plugins = registry.list();
        assert_eq!(plugins.len(), 1, "坏插件也要留在注册表里");
        assert!(
            matches!(plugins[0].status, PluginStatus::Failed(_)),
            "状态必须是 Failed，不能消失"
        );
        assert!(!plugins[0].is_active());
        assert!(!plugins[0].errors.is_empty());
        assert_eq!(registry.snapshot()[0].status, "failed");
        assert!(registry.set_enabled("broken", true).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_plugin_page_and_assets_are_rejected() {
        let root = temp_root("disabled");
        write_demo_plugin(&root);
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("demo").expect("插件应当加载成功");
        assert!(plugin.is_active());
        assert!(registry.read_page(&plugin, "p").is_ok());
        assert!(registry.asset_path("demo", "x.js").is_ok());

        registry.set_enabled("demo", false).unwrap();
        let plugin = registry.find("demo").unwrap();
        assert!(!plugin.is_active());
        assert!(registry.read_page(&plugin, "p").is_err());
        assert!(registry.asset_path("demo", "x.js").is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn asset_path_rejects_traversal_and_absolute_paths() {
        let root = temp_root("traversal");
        write_demo_plugin(&root);
        let registry = PluginRegistry::new(&root);
        assert!(registry.asset_path("demo", "../plugin.json").is_err());
        assert!(registry.asset_path("demo", "C:/Windows/win.ini").is_err());
        assert!(registry.asset_path("demo", "/etc/passwd").is_err());

        let root_canonical = root.join("demo").canonicalize().unwrap();
        assert!(safe_join(&root_canonical, &root_canonical, "../plugin.json").is_err());
        assert!(safe_join(&root_canonical, &root_canonical, "page.html").is_ok());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn directory_without_manifest_is_ignored() {
        let root = temp_root("empty");
        fs::create_dir_all(root.join("not-a-plugin")).unwrap();
        let registry = PluginRegistry::new(&root);
        assert!(registry.list().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    fn write_slang_plugin(root: &Path) -> PathBuf {
        let plugin = root.join("slang-demo");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.json"),
            r#"{
                "id": "slang-demo",
                "name": "Slang Demo",
                "version": "0.1.0",
                "description": "运行时组件语言示例",
                "slang": 2,
                "components": ["components.slang"],
                "pages": [{"slug":"hello","title":"Hello","template":"page.slang"}],
                "dashboard": "dashboard.slang",
                "slots": [{"slot":"composer.left","template":"slot.slang","order":10}]
            }"#,
        )
        .unwrap();
        fs::write(
            plugin.join("components.slang"),
            r#"@def Stat { @props { label: String } ::span(class: "stat") { {{ label }} } }"#,
        )
        .unwrap();
        fs::write(
            plugin.join("page.slang"),
            r#"@slang 2
@use "components.slang"
::div(class: "page") {
  ::h1 { {{ plugin.name }} }
  ::Stat(label: data.label)
}"#,
        )
        .unwrap();
        fs::write(
            plugin.join("slot.slang"),
            r#"@slang 2
::span(class: "slot") { {{ plugin.name }} · {{ slot.name }} }"#,
        )
        .unwrap();
        fs::write(
            plugin.join("dashboard.slang"),
            r#"@slang 2
::div(class: "widget") { v{{ plugin.version }} }"#,
        )
        .unwrap();
        plugin
    }

    #[test]
    fn slang_plugin_renders_page_slot_dashboard_with_context() {
        use crate::slang::{AppMeta, RenderContext};

        let root = temp_root("slang");
        write_slang_plugin(&root);
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("slang-demo").expect("plugin loads");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        assert!(
            plugin.slang_templates.len() >= 4,
            "templates: {:?}",
            plugin.slang_templates.keys().collect::<Vec<_>>()
        );

        let cx = RenderContext::new(AppMeta::default())
            .with_data(serde_json::json!({"label": "会话"}).into());
        let (title, html) = registry
            .read_page_with(&plugin, "hello", &cx)
            .expect("page renders");
        assert_eq!(title, "Hello");
        assert!(html.contains("Slang Demo"), "{html}");
        assert!(html.contains("会话"), "{html}");
        assert!(html.contains("class=\"stat\""), "{html}");

        let slot = registry.render_slot_with(Slot::ComposerLeft, &cx);
        assert_eq!(slot.contributors, 1, "{slot:?}");
        assert!(
            slot.html.contains("Slang Demo · composer.left"),
            "{}",
            slot.html
        );

        let widgets = registry.dashboard_widgets_with(&cx);
        assert_eq!(widgets.len(), 1);
        assert!(widgets[0].html.contains("v0.1.0"), "{}", widgets[0].html);

        let snapshot = registry.snapshot();
        assert_eq!(snapshot[0].engine, "slang");
        assert!(snapshot[0].slang_compiled >= 4);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn slang_plugin_reloads_included_component_changes() {
        let root = temp_root("slang_reload");
        let plugin_root = write_slang_plugin(&root);
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("slang-demo").unwrap();
        let cx = crate::slang::RenderContext::default();
        let (_, before) = registry.read_page_with(&plugin, "hello", &cx).unwrap();
        assert!(before.contains("class=\"stat\""), "{before}");

        fs::write(
            plugin_root.join("components.slang"),
            r#"@def Stat { @props { label: String } ::span(class: "stat-v2") { {{ label }} } }"#,
        )
        .unwrap();
        let report = registry.reload().unwrap();
        assert!(
            report.updated.contains(&"slang-demo".to_string()),
            "{report:?}"
        );
        let plugin = registry.find("slang-demo").unwrap();
        let (_, after) = registry.read_page_with(&plugin, "hello", &cx).unwrap();
        assert!(after.contains("class=\"stat-v2\""), "{after}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bundled_hello_slang_example_compiles_and_renders() {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let registry = PluginRegistry::new(&plugins_dir);
        let plugin = registry
            .find("hello-slang")
            .expect("hello-slang 示例插件应当被扫描到");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        assert!(
            plugin.slang_templates.len() >= 4,
            "templates: {:?}; errors: {:?}",
            plugin.slang_templates.keys().collect::<Vec<_>>(),
            plugin.errors
        );
        let cx = crate::slang::RenderContext::default();
        let (_, html) = registry
            .read_page_with(&plugin, "hello", &cx)
            .expect("示例页面应当能渲染");
        assert!(html.contains("前端插件页面（Slang）"), "{html}");
        assert!(html.contains("hello-slang"), "{html}");
    }

    #[test]
    fn broken_slang_template_is_visible_in_plugin_errors() {
        let root = temp_root("slang_broken");
        let plugin_root = write_slang_plugin(&root);
        fs::write(
            plugin_root.join("page.slang"),
            r#"@slang 2
::Nope()"#,
        )
        .unwrap();
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("slang-demo").expect("plugin still loads");
        assert!(!plugin.errors.is_empty(), "errors: {:?}", plugin.errors);
        assert!(
            plugin.errors.iter().any(|error| error.contains("未知组件")),
            "errors: {:?}",
            plugin.errors
        );
        let error = registry
            .read_page_with(&plugin, "hello", &crate::slang::RenderContext::default())
            .unwrap_err();
        assert!(format!("{error:#}").contains("未编译成功"), "{error:#}");

        let _ = fs::remove_dir_all(&root);
    }
}
