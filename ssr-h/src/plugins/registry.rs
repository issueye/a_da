//! 前端插件注册表：TSX 页面 / 插槽 / 仪表盘卡片。
//!
//! 插件只接受 `.tsx` 模板；加载/重载时由 OXC 递归转译相对模块，运行时用 Boa 的
//! ESM loader 执行，再交给 Rust 组件库渲染。没有 `.slang` / `.html` 引擎。

use std::collections::HashMap;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, bail, Context, Result};

use crate::context::RenderContext;
use crate::tsx::{self, CompiledTsx};

use super::manifest::{Manifest, NavEntry, PageEntry, Slot, SlotEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    Ready,
    Disabled,
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
    pub errors: Vec<String>,
    /// 模板相对路径 → 已编译 TSX。
    pub tsx_templates: HashMap<String, Arc<CompiledTsx>>,
    fingerprint: u64,
}

impl PluginRecord {
    pub fn is_active(&self) -> bool {
        self.enabled && matches!(self.status, PluginStatus::Ready)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SlotOutput {
    pub html: String,
    pub contributors: usize,
    pub failed: Vec<String>,
}

pub struct PluginRegistry {
    dir: PathBuf,
    plugins: RwLock<Vec<PluginRecord>>,
}

#[derive(Debug, Clone, Default)]
pub struct ReloadReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<String>,
    pub total: usize,
}

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
    pub tsx_compiled: usize,
}

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
        tsx_templates: HashMap::new(),
        fingerprint: 0,
    }
}

impl PluginRegistry {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let registry = Self {
            dir: dir.into(),
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
                    if old.fingerprint == record.fingerprint {
                        record.enabled = old.enabled;
                        if !record.enabled {
                            record.status = PluginStatus::Disabled;
                        }
                        report.total += 1;
                    } else {
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
                report.removed.push(old.id.clone());
            }
        }
        Ok(report)
    }

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
            root,
            assets_dir,
            errors: Vec::new(),
            tsx_templates: HashMap::new(),
            fingerprint: 0,
        };

        let mut paths: Vec<String> = Vec::new();
        for entry in &record.slots {
            paths.push(entry.template.clone());
        }
        for page in &record.pages {
            paths.push(page.template.clone());
        }
        if let Some(dashboard) = &record.dashboard {
            paths.push(dashboard.clone());
        }
        paths.sort();
        paths.dedup();
        for path in &paths {
            let full = record.root.join(path);
            match tsx::compile_file(&full, &record.root) {
                Ok(template) => {
                    record.tsx_templates.insert(path.clone(), template);
                }
                Err(error) => {
                    let message = format!("TSX 模板 `{path}` 编译失败：{error}");
                    tracing::warn!("插件 {} {message}", record.id);
                    if !record.errors.contains(&message) {
                        record.errors.push(message);
                    }
                }
            }
        }

        record.fingerprint = plugin_fingerprint(&record.root, raw.as_bytes());
        Ok(Some(record))
    }

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

    pub fn render_slot(&self, slot: Slot) -> SlotOutput {
        self.render_slot_with(slot, &RenderContext::default())
    }

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
                let Some(template) = record.tsx_templates.get(&entry.template) else {
                    output.failed.push(record.id.clone());
                    continue;
                };
                let props = tsx_props(record, cx, None, Some((slot, entry.order, index)));
                match catch_unwind(AssertUnwindSafe(|| tsx::render(template, &props))) {
                    Ok(Ok(html)) => {
                        if !html.is_empty() {
                            contributions.push((entry.order, record.id.clone(), html));
                        }
                    }
                    Ok(Err(error)) => {
                        output.failed.push(record.id.clone());
                        tracing::warn!(
                            "插件 {} 插槽 {} 的 TSX 渲染失败: {}",
                            record.id,
                            slot.as_str(),
                            error
                        );
                    }
                    Err(_) => {
                        output.failed.push(record.id.clone());
                        tracing::warn!(
                            "插件 {} 插槽 {} 的 TSX 渲染 panic，已隔离",
                            record.id,
                            slot.as_str()
                        );
                    }
                }
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

    pub fn list(&self) -> Vec<PluginRecord> {
        self.plugins
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.plugins.read().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn find(&self, id: &str) -> Option<PluginRecord> {
        self.plugins
            .read()
            .ok()
            .and_then(|guard| guard.iter().find(|plugin| plugin.id == id).cloned())
    }

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
                tsx_compiled: record.tsx_templates.len(),
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
            let Some(template) = plugin.tsx_templates.get(rel) else {
                tracing::warn!("插件 {} 的 TSX 卡片未编译成功", plugin.id);
                continue;
            };
            let props = tsx_props(&plugin, cx, None, None);
            match catch_unwind(AssertUnwindSafe(|| tsx::render(template, &props))) {
                Ok(Ok(html)) => widgets.push(PluginWidget {
                    plugin_id: plugin.id.clone(),
                    plugin_name: plugin.name.clone(),
                    html,
                }),
                Ok(Err(error)) => {
                    tracing::warn!("插件 {} 的 TSX 卡片渲染失败: {}", plugin.id, error);
                }
                Err(_) => {
                    tracing::warn!("插件 {} 的 TSX 卡片渲染 panic，已隔离", plugin.id);
                }
            }
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
        if !plugin.is_active() {
            bail!("插件未启用或加载失败: {}", plugin.id);
        }
        let page = plugin
            .pages
            .iter()
            .find(|page| page.slug == slug)
            .ok_or_else(|| anyhow!("插件页面不存在: {}/{}", plugin.id, slug))?;
        let template = plugin.tsx_templates.get(&page.template).ok_or_else(|| {
            anyhow!(
                "插件页面 `{}` 的 TSX 模板未编译成功，见扩展页诊断",
                page.template
            )
        })?;
        let props = tsx_props(plugin, cx, Some(page), None);
        let html = tsx::render(template, &props)
            .map_err(|error| anyhow!("插件页面 `{}` 渲染失败：{error}", page.template))?;
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

#[derive(Debug, Clone)]
pub struct NavLink {
    pub plugin_id: String,
    pub label: String,
    pub href: String,
}

#[derive(Debug, Clone)]
pub struct PluginWidget {
    pub plugin_id: String,
    pub plugin_name: String,
    pub html: String,
}

fn tsx_props(
    record: &PluginRecord,
    cx: &RenderContext,
    page: Option<&PageEntry>,
    slot: Option<(Slot, i32, usize)>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "plugin": {
            "id": record.id,
            "name": record.name,
            "version": record.version,
            "description": record.description,
        },
        "app": {
            "workspace": cx.app.workspace,
            "gateway": cx.app.gateway,
            "product": cx.app.product,
            "version": cx.app.version,
        },
        "route": {
            "path": cx.route.path,
            "query": cx.route.query,
        },
        "data": cx.data,
    });
    if let Some(page) = page {
        value["page"] = serde_json::json!({
            "slug": page.slug,
            "title": page.title,
        });
        value["route"]["path"] = serde_json::json!(format!("/ext/{}/{}", record.id, page.slug));
    }
    if let Some((slot, order, index)) = slot {
        value["slot"] = serde_json::json!({
            "name": slot.as_str(),
            "order": order,
            "index": index,
        });
    }
    value
}

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
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "tsx" | "ts" | "js" | "jsx" | "mjs" | "cjs" | "html" | "json"
                )
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

fn safe_join(plugin_root: &Path, base: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("插件资源路径越界: {}", relative.display());
    }
    let path = base.join(relative);
    let canonical = path
        .canonicalize()
        .with_context(|| format!("解析插件资源失败: {}", path.display()))?;
    if !canonical.starts_with(plugin_root) {
        bail!("插件资源路径越界: {}", relative.display());
    }
    Ok(canonical)
}

fn content_type(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
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

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn fnv1a_init(bytes: &[u8]) -> u64 {
    fnv1a(0xcbf29ce484222325, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ssr_h_registry_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_tsx_plugin(root: &Path) -> PathBuf {
        let plugin_root = root.join("tsx-demo");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{
                "id": "tsx-demo",
                "name": "TSX Demo",
                "version": "0.1.0",
                "pages": [{"slug":"p","title":"P","template":"page.tsx"}],
                "dashboard": "dashboard.tsx",
                "slots": [{"slot":"composer.left","template":"slot.tsx","order":10}]
            }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("page.tsx"),
            r#"function Page(props) { return <div class="page">{props.plugin.id}</div>; }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("slot.tsx"),
            r#"function Page() { return <span class="slot">slot</span>; }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("dashboard.tsx"),
            r#"function Page() { return <div class="card">card</div>; }"#,
        )
        .unwrap();
        plugin_root
    }

    #[test]
    fn broken_plugin_is_visible_as_failed_instead_of_silently_disappearing() {
        let root = temp_root("broken");
        let plugin_root = root.join("bad");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(plugin_root.join("plugin.json"), "{not json").unwrap();
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("bad").expect("failed record exists");
        assert!(matches!(plugin.status, PluginStatus::Failed(_)));
        assert!(!plugin.errors.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_plugin_page_and_assets_are_rejected() {
        let root = temp_root("disabled");
        let plugin_root = write_tsx_plugin(&root);
        fs::create_dir_all(plugin_root.join("static")).unwrap();
        fs::write(plugin_root.join("static/app.js"), "console.log(1)").unwrap();
        let registry = PluginRegistry::new(&root);
        registry.set_enabled("tsx-demo", false).unwrap();
        let plugin = registry.find("tsx-demo").unwrap();
        let error = registry
            .read_page_with(&plugin, "p", &RenderContext::default())
            .unwrap_err();
        assert!(error.to_string().contains("未启用"));
        assert!(registry.asset_path("tsx-demo", "app.js").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn asset_path_rejects_traversal_and_absolute_paths() {
        let root = temp_root("assets");
        let plugin_root = write_tsx_plugin(&root);
        fs::create_dir_all(plugin_root.join("static")).unwrap();
        fs::write(plugin_root.join("static/app.js"), "console.log(1)").unwrap();
        let registry = PluginRegistry::new(&root);
        assert!(registry.asset_path("tsx-demo", "app.js").is_ok());
        assert!(registry.asset_path("tsx-demo", "../plugin.json").is_err());
        assert!(registry
            .asset_path("tsx-demo", "C:/Windows/win.ini")
            .is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn directory_without_manifest_is_ignored() {
        let root = temp_root("empty");
        fs::create_dir_all(root.join("empty")).unwrap();
        let registry = PluginRegistry::new(&root);
        assert!(registry.find("empty").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tsx_plugin_renders_page_slot_and_dashboard() {
        let root = temp_root("render");
        let _ = write_tsx_plugin(&root);
        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("tsx-demo").expect("plugin loads");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        let cx = RenderContext::default().with_data(serde_json::json!({"plugin_count": 1}));
        let (title, html) = registry.read_page_with(&plugin, "p", &cx).unwrap();
        assert_eq!(title, "P");
        assert!(html.contains("tsx-demo"), "{html}");
        let slot = registry.render_slot_with(Slot::ComposerLeft, &cx);
        assert!(
            slot.html.contains("slot"),
            "slot: {}; failed: {:?}",
            slot.html,
            slot.failed
        );
        let widgets = registry.dashboard_widgets_with(&cx);
        assert!(widgets.iter().any(|widget| widget.html.contains("card")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tsx_plugin_reloads_when_template_changes() {
        let root = temp_root("reload");
        let plugin_root = write_tsx_plugin(&root);
        let registry = PluginRegistry::new(&root);
        let cx = RenderContext::default();
        let plugin = registry.find("tsx-demo").unwrap();
        let (_, before) = registry.read_page_with(&plugin, "p", &cx).unwrap();
        assert!(before.contains("tsx-demo"), "{before}");

        fs::write(
            plugin_root.join("page.tsx"),
            r#"function Page() { return <div class="v2">v2</div>; }"#,
        )
        .unwrap();
        let report = registry.reload().unwrap();
        assert!(
            report.updated.contains(&"tsx-demo".to_string()),
            "{report:?}"
        );
        let plugin = registry.find("tsx-demo").unwrap();
        let (_, after) = registry.read_page_with(&plugin, "p", &cx).unwrap();
        assert!(after.contains("v2"), "{after}");
        assert!(!after.contains("tsx-demo"), "{after}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hello_tsx_example_compiles_and_renders() {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let registry = PluginRegistry::new(&plugins_dir);
        let plugin = registry.find("hello-tsx").expect("hello-tsx exists");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        assert!(
            plugin.tsx_templates.len() >= 3,
            "templates: {:?}",
            plugin.tsx_templates.keys()
        );
        let cx = RenderContext::default().with_data(serde_json::json!({"plugin_count": 3}));
        let (title, html) = registry.read_page_with(&plugin, "hello", &cx).unwrap();
        assert_eq!(title, "Hello TSX");
        assert!(html.contains("页面由 TSX 在服务端渲染"), "{html}");
        assert!(html.contains("插件 3"), "{html}");
        let slot = registry.render_slot_with(Slot::ComposerLeft, &cx);
        assert!(
            slot.html.contains("TSX 插槽"),
            "slot: {}; failed: {:?}",
            slot.html,
            slot.failed
        );
        let widgets = registry.dashboard_widgets_with(&cx);
        assert!(widgets
            .iter()
            .any(|widget| widget.html.contains("TSX 运行时卡片")));
    }

    #[test]
    fn tsx_plugin_with_imports_loads_and_renders() {
        let root = temp_root("modules");
        let plugin_root = root.join("tsx-modules");
        fs::create_dir_all(plugin_root.join("components")).unwrap();
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{
                "id": "tsx-modules",
                "name": "TSX Modules",
                "pages": [{"slug":"p","title":"P","template":"page.tsx"}]
            }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("label.ts"),
            r#"export const label = "来自 label.ts";"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("components/card.tsx"),
            r#"
import { label } from "../label";
import { Chip } from "ssr-h:components";
export function Card() {
  return <Chip text={label} tone="brand" />;
}
"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("page.tsx"),
            r#"
import { label } from "./label";
import { Card } from "./components/card";
export function Page() {
  return <div><span>{label}</span><Card /></div>;
}
"#,
        )
        .unwrap();

        let registry = PluginRegistry::new(&root);
        let plugin = registry.find("tsx-modules").expect("plugin loads");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        let cx = RenderContext::default();
        let (_, html) = registry
            .read_page_with(&plugin, "p", &cx)
            .expect("module page renders");
        assert!(html.contains("来自 label.ts"), "{html}");
        assert!(html.contains("ada-chip--brand"), "{html}");

        fs::write(
            plugin_root.join("label.ts"),
            r#"export const label = "更新后的 label";"#,
        )
        .unwrap();
        let report = registry.reload().unwrap();
        assert!(
            report.updated.contains(&"tsx-modules".to_string()),
            "{report:?}"
        );
        let plugin = registry.find("tsx-modules").unwrap();
        let (_, html) = registry.read_page_with(&plugin, "p", &cx).unwrap();
        assert!(html.contains("更新后的 label"), "{html}");
        let _ = fs::remove_dir_all(&root);
    }
}
