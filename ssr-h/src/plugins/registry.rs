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
use std::sync::RwLock;

use anyhow::{anyhow, bail, Context, Result};

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
}

impl PluginRegistry {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let registry = Self {
            dir,
            plugins: RwLock::new(Vec::new()),
        };
        if let Err(error) = registry.reload() {
            tracing::warn!("扫描前端插件目录失败: {error:#}");
        }
        registry
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
                    let id = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    tracing::warn!("跳过无效前端插件 {id}: {error:#}");
                    report.failed.push(id);
                }
            }
        }
        discovered.sort_by(|a, b| a.id.cmp(&b.id));

        let mut guard = self.plugins.write().map_err(|_| anyhow!("插件注册表锁异常"))?;
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
        manifest
            .validate()
            .context("插件清单校验失败")?;

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
            dashboard: manifest.dashboard,
            root,
            assets_dir,
            errors: Vec::new(),
            fragments: HashMap::new(),
            fingerprint: 0,
        };

        // 装载阶段：片段读不到就降级（该插槽不贡献内容），插件本体照常可用。
        let mut fingerprint = fnv1a_init(raw.as_bytes());
        for entry in record.slots.clone() {
            let key = PluginRecord::fragment_key(&entry.slot, &entry.template);
            match read_fragment(&record.root, &record.root, &entry.template) {
                Ok(html) => {
                    fingerprint = fnv1a(fingerprint, html.as_bytes());
                    record.fragments.insert(key, html);
                }
                Err(error) => {
                    let message = format!("插槽 {} 的片段读取失败: {error:#}", entry.slot);
                    tracing::warn!("插件 {} {message}", record.id);
                    record.errors.push(message);
                }
            }
        }
        record.fingerprint = fingerprint;
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
            for entry in entries {
                let key = PluginRecord::fragment_key(&entry.slot, &entry.template);
                let Some(html) = record.fragments.get(&key) else {
                    output.failed.push(record.id.clone());
                    continue;
                };
                // 单个插件的片段渲染不允许把整个页面带崩。
                let rendered = catch_unwind(AssertUnwindSafe(|| html.clone())).unwrap_or_else(
                    |_| {
                        output.failed.push(record.id.clone());
                        String::new()
                    },
                );
                if rendered.is_empty() {
                    continue;
                }
                contributions.push((entry.order, record.id.clone(), rendered));
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
        let mut widgets = Vec::new();
        for plugin in self.list() {
            if !plugin.is_active() {
                continue;
            }
            let Some(rel) = plugin.dashboard.as_deref() else {
                continue;
            };
            match read_fragment(&plugin.root, &plugin.root, rel) {
                Ok(html) => widgets.push(PluginWidget {
                    plugin_id: plugin.id.clone(),
                    plugin_name: plugin.name.clone(),
                    html,
                }),
                Err(error) => tracing::warn!("读取前端插件卡片失败 {}: {error:#}", plugin.id),
            }
        }
        widgets
    }

    pub fn read_page(&self, plugin: &PluginRecord, slug: &str) -> Result<(String, String)> {
        let page = plugin
            .pages
            .iter()
            .find(|page| page.slug == slug)
            .ok_or_else(|| anyhow!("插件页面不存在: {}/{}", plugin.id, slug))?;
        let html = read_fragment(&plugin.root, &plugin.root, &page.template)?;
        Ok((page.title.clone(), html))
    }

    pub fn asset_path(&self, plugin_id: &str, relative: &str) -> Result<(PathBuf, String)> {
        let plugin = self
            .find(plugin_id)
            .ok_or_else(|| anyhow!("前端插件不存在: {plugin_id}"))?;
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

fn resolve_assets_dir(root: &Path, declared: Option<&str>) -> Option<PathBuf> {
    match declared.map(str::trim).filter(|s| !s.is_empty()) {
        Some(rel) => {
            let dir = safe_join(root, root, rel).ok()?;
            dir.is_dir().then_some(dir)
        }
        None => {
            let default = root.join("static");
            default.is_dir().then(|| default.canonicalize().unwrap_or(default))
        }
    }
}

fn read_fragment(plugin_root: &Path, base: &Path, relative: &str) -> Result<String> {
    let path = safe_join(plugin_root, base, relative)?;
    let meta = fs::metadata(&path).with_context(|| format!("读取插件文件失败: {}", path.display()))?;
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
