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

use super::manifest::{ApiEntry, ApiMethod, Manifest, NavEntry, PageEntry, Slot, SlotEntry};

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
    /// 相对 assets 目录的 CSS 路径。
    pub styles: Vec<String>,
    /// 相对 assets 目录的 JS 路径（注入页面 `<head>`）。
    pub scripts: Vec<String>,
    /// 插件能力声明（决定宿主是否注入供应商只读数据）。
    pub capabilities: Vec<String>,
    /// 插件自带的 HTTP API 端点（`/ext/{id}/api/{path}`）。
    pub api: Vec<ApiEntry>,
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
    /// 停用状态文件。与插件目录分开，安装目录只读时也能记住用户选择。
    state_path: PathBuf,
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
    pub capabilities: Vec<String>,
    pub tsx_compiled: usize,
}

/// 默认状态文件：`A_DA_HOME/web-ui-state.json` 或 `~/.a-da/web-ui-state.json`。
fn default_state_path() -> PathBuf {
    if let Ok(home) = std::env::var("A_DA_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join("web-ui-state.json");
        }
    }
    let user = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    if user.is_empty() {
        PathBuf::from("web-ui-state.json")
    } else {
        PathBuf::from(user).join(".a-da").join("web-ui-state.json")
    }
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
        styles: Vec::new(),
        scripts: Vec::new(),
        api: Vec::new(),
        capabilities: Vec::new(),
        errors: vec![message.to_string()],
        tsx_templates: HashMap::new(),
        fingerprint: 0,
    }
}

impl PluginRegistry {
    /// 生产默认：插件状态写用户目录（`~/.a-da/web-ui-state.json`），
    /// 插件目录本身可以随包只读分发。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::with_state_path(dir, default_state_path())
    }

    /// 指定状态文件（测试必须用临时目录，绝不能写用户真实的 `~/.a-da`）。
    pub fn with_state_path(dir: impl Into<PathBuf>, state_path: impl Into<PathBuf>) -> Self {
        let registry = Self {
            dir: dir.into(),
            state_path: state_path.into(),
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

    pub fn state_path(&self) -> &Path {
        &self.state_path
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
            // 加载期也要隔离 panic：OXC/Boa 遇到诡异输入可能直接 panic，
            // 一次 reload 不能把整个进程带走（渲染路径早就按同样口径处理了）。
            match catch_unwind(AssertUnwindSafe(|| self.register(&path))) {
                Ok(Ok(Some(record))) => discovered.push(record),
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    let id = dir_plugin_id(&path);
                    let message = format!("{error:#}");
                    tracing::warn!("前端插件 {id} 加载失败: {message}");
                    discovered.push(failed_record(&path, &id, &message));
                    report.failed.push(id);
                }
                Err(_) => {
                    let id = dir_plugin_id(&path);
                    let message = "插件加载过程中 panic，已隔离".to_string();
                    tracing::warn!("前端插件 {id} {message}");
                    discovered.push(failed_record(&path, &id, &message));
                    report.failed.push(id);
                }
            }
        }
        // (id, 路径) 双键排序：同 id 的两个目录谁胜出必须稳定，否则每次重载"随机"一个生效。
        discovered.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.root.cmp(&b.root)));

        let mut guard = self
            .plugins
            .write()
            .map_err(|_| anyhow!("插件注册表锁异常"))?;
        let previous = std::mem::take(&mut *guard);
        // 持久化的停用集合：热重载的停用态由下面的 previous 分支保留（内存），
        // 这里补的是**跨进程**的那份——重启后首次扫描也要保持停用。
        let persisted_disabled = self.load_disabled_state();

        for mut record in discovered {
            // 两个目录声明同一个 id 时不能"都塞进去"：`find` 只会命中第一个，
            // 导航/插槽却会重复渲染，排查起来像见鬼。重复的那个按加载失败处理并说明原因。
            if let Some(existing) = guard.iter().find(|other| other.id == record.id) {
                let message = format!(
                    "插件 id `{}` 与 `{}` 重复，该目录已忽略",
                    record.id,
                    existing.root.display()
                );
                tracing::warn!("{message}");
                report.failed.push(record.id.clone());
                guard.push(failed_record(&record.root, &record.id, &message));
                continue;
            }
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
                    // 首次加载（重启后的首次扫描 / 新插件）：应用持久化的停用状态
                    if persisted_disabled.contains(&record.id) {
                        record.enabled = false;
                        record.status = PluginStatus::Disabled;
                    }
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
            styles: Vec::new(),
            scripts: Vec::new(),
            capabilities: manifest.capabilities.clone(),
            api: manifest.api,
            errors: Vec::new(),
            tsx_templates: HashMap::new(),
            fingerprint: 0,
        };

        let styles = collect_assets(&mut record, &manifest.styles, "样式");
        record.styles = styles;
        let scripts = collect_assets(&mut record, &manifest.scripts, "脚本");
        record.scripts = scripts;

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
        for entry in &record.api {
            paths.push(entry.template.clone());
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

    /// 停用状态文件（默认 `<应用目录>/web-ui-state.json`）。
    ///
    /// 放在用户目录而不是插件目录：安装目录可能只读，且插件目录可能随包分发。
    /// 格式刻意最小：`{ "disabled": ["id", …] }`，缺省即启用。
    fn state_file(&self) -> &Path {
        &self.state_path
    }

    fn load_disabled_state(&self) -> Vec<String> {
        let Ok(raw) = fs::read_to_string(self.state_file()) else {
            return Vec::new();
        };
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => value
                .get("disabled")
                .and_then(|list| list.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .filter(|id| !id.trim().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            Err(error) => {
                // 坏文件只警告并当作"全启用"：状态文件坏了不该把插件锁死在停用态
                tracing::warn!(
                    "插件停用状态文件解析失败（忽略，{}）: {error}",
                    self.state_file().display()
                );
                Vec::new()
            }
        }
    }

    fn persist_disabled_state(&self, disabled: &[String]) -> Result<()> {
        let body = serde_json::json!({ "disabled": disabled }).to_string();
        let target = self.state_file();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建插件状态目录失败: {}", parent.display()))?;
        }
        let tmp = target.with_extension("json.tmp");
        fs::write(&tmp, body)
            .with_context(|| format!("写入插件停用状态失败: {}", tmp.display()))?;
        // Windows 的 rename 不覆盖已存在目标，先删再换
        let _ = fs::remove_file(target);
        fs::rename(&tmp, target)
            .with_context(|| format!("落盘插件停用状态失败: {}", target.display()))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        {
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
        }
        // 停用状态落盘：重启后仍生效。文件里只记**停用**的 id（缺省即启用）；
        // 插件目录被删不清条目——重新放回来时保持原状态是合理语义。
        let mut disabled = self.load_disabled_state();
        if enabled {
            disabled.retain(|item| item != id);
        } else if !disabled.iter().any(|item| item == id) {
            disabled.push(id.to_string());
        }
        self.persist_disabled_state(&disabled)
    }

    pub fn render_slot(&self, slot: Slot) -> SlotOutput {
        self.render_slot_with(slot, &RenderContext::default())
    }

    pub fn render_slot_with(&self, slot: Slot, cx: &RenderContext) -> SlotOutput {
        let mut output = SlotOutput::default();
        // 先把"要渲染什么"在锁内取出来，再在锁外渲染。
        // 渲染是 CPU 密集的（Boa + OXC，可能几十毫秒），攥着读锁会让
        // `reload` 的写锁一直排队，界面表现为"点重载没反应"。
        let targets: Vec<(i32, String, Arc<CompiledTsx>, serde_json::Value)> = {
            let Ok(guard) = self.plugins.read() else {
                return output;
            };
            let mut targets = Vec::new();
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
                    match record.tsx_templates.get(&entry.template) {
                        Some(template) => targets.push((
                            entry.order,
                            record.id.clone(),
                            Arc::clone(template),
                            tsx_props(record, cx, None, Some((slot, entry.order, index))),
                        )),
                        None => output.failed.push(record.id.clone()),
                    }
                }
            }
            targets
        };

        let mut contributions: Vec<(i32, String, String)> = Vec::new();
        for (order, id, template, props) in targets {
            match catch_unwind(AssertUnwindSafe(|| tsx::render(template.clone(), &props))) {
                Ok(Ok(html)) => {
                    if !html.is_empty() {
                        contributions.push((order, id, html));
                    }
                }
                Ok(Err(error)) => {
                    output.failed.push(id.clone());
                    tracing::warn!(
                        "插件 {} 插槽 {} 的 TSX 渲染失败: {}",
                        id,
                        slot.as_str(),
                        error
                    );
                }
                Err(_) => {
                    output.failed.push(id.clone());
                    tracing::warn!(
                        "插件 {} 插槽 {} 的 TSX 渲染 panic，已隔离",
                        id,
                        slot.as_str()
                    );
                }
            }
        }
        contributions.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, id, html) in contributions {
            output.contributors += 1;
            output
                .html
                .push_str(&format!("<div data-plugin-scope=\"{}\">{}</div>", id, html));
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

    /// 是否有**已启用**的插件声明了某个能力。
    ///
    /// 宿主据此决定要不要为插件抓取额外数据（例如 `processes.read` 才会去查网关
    /// 进程列表）；停用/加载失败的插件不算，避免白跑一轮并绕过"停用即消失"。
    pub fn has_capability(&self, capability: &str) -> bool {
        self.plugins
            .read()
            .map(|guard| {
                guard.iter().any(|record| {
                    record.is_active()
                        && record
                            .capabilities
                            .iter()
                            .any(|declared| declared == capability)
                })
            })
            .unwrap_or(false)
    }

    pub fn find(&self, id: &str) -> Option<PluginRecord> {
        self.plugins
            .read()
            .ok()
            .and_then(|guard| guard.iter().find(|plugin| plugin.id == id).cloned())
    }

    /// 查找插件的 API 端点（**热**：注册表是换帧级重扫的，重载后新端点立即生效）。
    ///
    /// 方法与路径都精确匹配；插件未启用/未就绪一律 None——API 面与页面/插槽
    /// 共享同一条「停用即消失」的降级规则。
    pub fn find_api(
        &self,
        id: &str,
        method: ApiMethod,
        path: &str,
    ) -> Option<(ApiEntry, std::sync::Arc<tsx::CompiledTsx>)> {
        let plugin = self.find(id)?;
        if !plugin.is_active() {
            return None;
        }
        let entry = plugin
            .api
            .iter()
            .find(|entry| entry.parsed_method().ok() == Some(method) && entry.path.trim() == path.trim())?
            .clone();
        let template = plugin.tsx_templates.get(&entry.template)?.clone();
        Some((entry, template))
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
                capabilities: record.capabilities,
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

    /// 所有启用插件的 CSS 链接（带插件指纹，重载后浏览器会重新取）。
    pub fn styles(&self) -> Vec<String> {
        self.active_assets(AssetKind::Style)
    }

    /// 所有启用插件的 JS 链接（注入 `<head>` 并 `defer`，同样带指纹）。
    pub fn scripts(&self) -> Vec<String> {
        self.active_assets(AssetKind::Script)
    }

    fn active_assets(&self, kind: AssetKind) -> Vec<String> {
        let mut links = Vec::new();
        for plugin in self.list() {
            if !plugin.is_active() {
                continue;
            }
            let files = match kind {
                AssetKind::Style => &plugin.styles,
                AssetKind::Script => &plugin.scripts,
            };
            for file in files {
                links.push(format!(
                    "/ext/{}/assets/{}?v={:x}",
                    plugin.id, file, plugin.fingerprint
                ));
            }
        }
        links.sort();
        links.dedup();
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
            match catch_unwind(AssertUnwindSafe(|| tsx::render(template.clone(), &props))) {
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
        // 页面渲染也要隔离 panic：插槽/卡片早就这么做了，页面漏掉的话
        // 一个诡异模板会让连接直接断掉（用户只看到空白，日志里也没有 500 可读）。
        let html = match catch_unwind(AssertUnwindSafe(|| tsx::render(template.clone(), &props))) {
            Ok(Ok(html)) => html,
            Ok(Err(error)) => {
                return Err(anyhow!("插件页面 `{}` 渲染失败：{error}", page.template))
            }
            Err(_) => {
                return Err(anyhow!(
                    "插件页面 `{}` 渲染 panic，已隔离（详见服务端日志）",
                    page.template
                ))
            }
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

// ── 异步包装 ─────────────────────────────────────────────────────────────
//
// 渲染是**同步 CPU 活**（OXC + Boa，单页可到几十毫秒）：直接在 axum handler 里跑
// 会占住 tokio 工作线程，live 轮询每 2 秒来一次，多几个插件就把响应拖慢。
// 统一丢进 `spawn_blocking`，并保证一批插槽只占一个阻塞线程。

/// 渲染一个插槽（阻塞线程池）。
pub async fn render_slot_off_thread(
    registry: Arc<PluginRegistry>,
    slot: Slot,
    cx: RenderContext,
) -> SlotOutput {
    let fallback_name = slot.as_str();
    match tokio::task::spawn_blocking(move || registry.render_slot_with(slot, &cx)).await {
        Ok(output) => output,
        Err(error) => {
            tracing::warn!("插槽 {fallback_name} 渲染任务失败: {error}");
            SlotOutput::default()
        }
    }
}

/// 顺序渲染一批插槽，只占一个阻塞线程（页面外壳一次要渲染十一个插槽）。
pub async fn render_slots_off_thread(
    registry: Arc<PluginRegistry>,
    slots: &[Slot],
    cx: RenderContext,
) -> Vec<SlotOutput> {
    let slots = slots.to_vec();
    let count = slots.len();
    let task = move || {
        slots
            .iter()
            .map(|slot| registry.render_slot_with(*slot, &cx))
            .collect::<Vec<_>>()
    };
    match tokio::task::spawn_blocking(task).await {
        Ok(outputs) => outputs,
        Err(error) => {
            tracing::warn!("插槽批量渲染任务失败: {error}");
            vec![SlotOutput::default(); count]
        }
    }
}

/// TSX/Boa 的递归求值比 Tokio 阻塞池默认栈更深；页面/插槽渲染放到独立大栈线程，
/// 避免 `tokio-rt-worker` 的 2MB 栈被一整页嵌套组件打爆。
async fn run_with_large_stack<T, F>(name: &'static str, task: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name(format!("ssr-tsx-{name}"))
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let _ = sender.send(task());
        })
        .map_err(|error| anyhow!("创建 TSX 渲染线程失败: {error}"))?;
    receiver
        .await
        .map_err(|error| anyhow!("TSX 渲染线程异常退出: {error}"))
}
/// 渲染插件独立页面（独立大栈线程；Boa 递归深度高于 Tokio 阻塞池默认栈）。
pub async fn read_page_off_thread(
    registry: Arc<PluginRegistry>,
    plugin_id: String,
    slug: String,
    cx: RenderContext,
) -> Result<(String, String)> {
    let task = move || {
        let plugin = registry
            .find(&plugin_id)
            .ok_or_else(|| anyhow!("前端插件不存在: {plugin_id}"))?;
        registry.read_page_with(&plugin, &slug, &cx)
    };
    run_with_large_stack("page", task)
        .await
        .map_err(|error| anyhow!("插件页面渲染任务失败: {error}"))?
}

/// 渲染仪表盘 TSX 卡片（阻塞线程池）。
pub async fn dashboard_widgets_off_thread(
    registry: Arc<PluginRegistry>,
    cx: RenderContext,
) -> Vec<PluginWidget> {
    match tokio::task::spawn_blocking(move || registry.dashboard_widgets_with(&cx)).await {
        Ok(widgets) => widgets,
        Err(error) => {
            tracing::warn!("插件卡片渲染任务失败: {error}");
            Vec::new()
        }
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

/// API 处理器的 props：与页面/插槽同源（plugin/app/route/data），额外带 `api` 请求面
/// （method / path / query / body）。
///
/// `data` 只到 `plugin_count` 为止——API 可能被客户端高频调用，不该每次拖一份会话
/// 快照；需要会话数据的端点让调用方带 id 走核心路由，模板自己不碰会话。
pub fn api_props(
    record: &PluginRecord,
    cx: &RenderContext,
    entry: &ApiEntry,
    query: serde_json::Value,
    body: serde_json::Value,
) -> serde_json::Value {
    let mut value = tsx_props(record, cx, None, None);
    value["api"] = serde_json::json!({
        "method": entry.method.as_str(),
        "path": entry.path,
        "query": query,
        "body": body,
    });
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
                    "tsx" | "ts" | "js" | "jsx" | "mjs" | "cjs" | "css" | "html" | "json"
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

/// 插件资源的两类清单：样式与脚本。
#[derive(Debug, Clone, Copy)]
enum AssetKind {
    Style,
    Script,
}

/// 校验并收集一组资源路径（缺失/越界都记进插件的 `errors`，不拖垮整个插件）。
fn collect_assets(record: &mut PluginRecord, files: &[String], label: &str) -> Vec<String> {
    let mut collected = Vec::new();
    if files.is_empty() {
        return collected;
    }
    let Some(assets) = record.assets_dir.clone() else {
        record.errors.push(format!(
            "插件声明了{label}，但没有可用的 assets 目录（默认 static/）"
        ));
        return collected;
    };
    for file in files {
        match safe_join(&record.root, assets.as_path(), file) {
            Ok(path) if path.is_file() => collected.push(file.trim().to_string()),
            Ok(_) => record.errors.push(format!("插件{label}不存在: {file}")),
            Err(error) => record
                .errors
                .push(format!("插件{label}路径不合法 `{file}`: {error}")),
        }
    }
    collected
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

    /// 测试注册表：状态文件放临时目录（**绝不能**落到用户真实的 `~/.a-da`）。
    fn registry_at(dir: impl Into<PathBuf>) -> PluginRegistry {
        let dir = dir.into();
        let state = dir.join(".ssr-h-state.json");
        PluginRegistry::with_state_path(dir, state)
    }

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
        let registry = registry_at(&root);
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
        let registry = registry_at(&root);
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
        let registry = registry_at(&root);
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
        let registry = registry_at(&root);
        assert!(registry.find("empty").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tsx_plugin_renders_page_slot_and_dashboard() {
        let root = temp_root("render");
        let _ = write_tsx_plugin(&root);
        let registry = registry_at(&root);
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
        assert!(
            slot.html.contains(r#"data-plugin-scope="tsx-demo""#),
            "{}",
            slot.html
        );
        let widgets = registry.dashboard_widgets_with(&cx);
        assert!(widgets.iter().any(|widget| widget.html.contains("card")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_styles_are_validated_and_exposed() {
        let root = temp_root("styles");
        let plugin_root = root.join("styled");
        fs::create_dir_all(plugin_root.join("static")).unwrap();
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{
                "id": "styled",
                "name": "Styled",
                "assets": "static",
                "styles": ["plugin.css"],
                "pages": [{"slug":"p","title":"P","template":"page.tsx"}]
            }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("page.tsx"),
            r#"function Page() { return <div>styled</div>; }"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("static/plugin.css"),
            ".styled { color: red; }",
        )
        .unwrap();
        let registry = registry_at(&root);
        let plugin = registry.find("styled").expect("plugin loads");
        assert_eq!(plugin.styles, vec!["plugin.css"]);
        let styles = registry.styles();
        assert_eq!(styles.len(), 1, "{styles:?}");
        assert!(styles[0].starts_with("/ext/styled/assets/plugin.css?v="));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tsx_plugin_reloads_when_template_changes() {
        let root = temp_root("reload");
        let plugin_root = write_tsx_plugin(&root);
        let registry = registry_at(&root);
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

    /// 真实示例必须能编译并渲染。🔴 在**临时副本**上测：真实目录里可能有用户的
    /// 停用状态（.ssr-h-state.json，运行时产物）——直接读它，示例测试就会跟着
    /// 用户点过什么而随机红；测试也绝不能反过来写用户的状态文件。
    #[test]
    fn hello_tsx_example_compiles_and_renders() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let root = temp_root("example-copy");
        copy_tree(&source, &root);
        let registry = registry_at(&root);
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
        // hello-tsx 只是最小示例：不再占用 composer.left，避免工具栏常驻一个演示胶囊。
        assert!(plugin.slots.is_empty(), "slots: {:?}", plugin.slots);
        let slot = registry.render_slot_with(Slot::ComposerLeft, &cx);
        assert!(
            !slot.html.contains("TSX 插槽"),
            "hello-tsx 不应再占用 composer.left：{}",
            slot.html
        );
        let widgets = registry.dashboard_widgets_with(&cx);
        assert!(widgets
            .iter()
            .any(|widget| widget.html.contains("TSX 运行时卡片")));
        let _ = fs::remove_dir_all(&root);
    }

    /// 进程管理插件：合成一份 `processes` 数据，守住 TSX 能编译、槽位能渲染、
    /// 且不会把 token 带进页面。
    #[test]
    fn process_manager_example_renders_injected_processes() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let root = temp_root("process-manager-copy");
        copy_tree(&source, &root);
        let registry = registry_at(&root);
        let plugin = registry
            .find("process-manager")
            .expect("process-manager exists");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);

        let cx = RenderContext::default().with_data(serde_json::json!({
            "plugin_count": 6,
            "render": { "at": 1_761_000_000_000u64 },
            "processes": {
                "status": {
                    "pid": 4321,
                    "product": "ada-coding",
                    "defaultWorkspace": "E:/codes/demo",
                    "agentCount": 1,
                    "routableCount": 1
                },
                "agents": [{
                    "id": "ada-coding::abc",
                    "product": "ada-coding",
                    "workspace": "E:/codes/demo",
                    "endpoint": "ws://127.0.0.1:5555/rpc",
                    "status": "ready",
                    "pid": 777,
                    "lastActiveMs": 1_760_999_999_000u64
                }],
                "count": 1,
                "error": null
            }
        }));
        let slot = registry.render_slot_with(Slot::TopbarRight, &cx);
        assert_eq!(slot.failed, Vec::<String>::new(), "{slot:?}");
        assert!(slot.html.contains("process-manager"), "{}", slot.html);
        assert!(slot.html.contains("ada-gateway"), "{}", slot.html);
        assert!(slot.html.contains("AGENT 实例"), "{}", slot.html);
        assert!(slot.html.contains("demo"), "{}", slot.html);
        assert!(slot.html.contains("PID 777"), "{}", slot.html);
        assert!(!slot.html.contains("token="), "{}", slot.html);
        let _ = fs::remove_dir_all(&root);
    }

    /// 供应商管理页：合成数据渲染真实 TSX——既守住示例没有语法/校验错误，
    /// 也守住页头「当前模型」药丸的新结构（旧的两张 stat 大卡已废）。
    #[test]
    fn provider_admin_example_renders_with_current_model_chip() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let root = temp_root("example-provider");
        copy_tree(&source, &root);
        let registry = registry_at(&root);
        let plugin = registry.find("provider-admin").expect("provider-admin exists");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        let cx = RenderContext::default().with_data(serde_json::json!({
            "plugin_count": 3,
            "providerAdmin": {
                "providers": [
                    { "id": "prov_a", "name": "主力", "protocol": "openai_chat",
                      "protocolLabel": "OpenAI Chat", "baseUrl": "https://api.example.com/v1",
                      "proxyUrl": "", "headers": "", "apiKeySet": true, "apiKeyHint": "sk-t…7890",
                      "modelCount": 2, "active": true,
                      "models": [
                        { "index": 0, "id": "m1", "name": "M1", "contextWindow": "1000000",
                          "maxOutputTokens": "65536", "supportsImages": false, "current": true },
                        { "index": 1, "id": "m2", "name": "M2", "contextWindow": "1000000",
                          "maxOutputTokens": "65536", "supportsImages": true, "current": false }
                      ] },
                    { "id": "prov_b", "name": "备用", "protocol": "openai_chat",
                      "protocolLabel": "OpenAI Chat", "baseUrl": "https://b.example.com/v1",
                      "proxyUrl": "", "headers": "", "apiKeySet": false, "apiKeyHint": "",
                      "modelCount": 1, "active": false,
                      "models": [ { "index": 0, "id": "m3", "name": "M3", "contextWindow": "",
                                    "maxOutputTokens": "", "supportsImages": false, "current": false } ] }
                ],
                "selectedId": "prov_a",
                "activeTab": "connection",
                "currentModel": "m1",
                "currentBaseUrl": "https://api.example.com/v1",
                "canDeleteProviders": true,
                "protocols": [ { "value": "openai_chat", "label": "OpenAI Chat" } ]
            }
        }));
        // 渲染自带 32MB 大栈线程与 wall-clock 预算（tsx::run_with_budget），
        // 测试线程不再需要手动给栈。
        let (title, html) = registry.read_page_with(&plugin, "providers", &cx).unwrap();
        assert_eq!(title, "供应商");
        // 页头当前模型药丸（可点，去它的模型页签）；旧的两张 stat 大卡不再渲染
        assert!(html.contains("pa-current"), "{html}");
        assert!(html.contains("当前模型"), "{html}");
        assert!(html.contains(r#"href="/ext/provider-admin/providers?provider=prov_a&amp;tab=models""#), "{html}");
        assert!(!html.contains("pa-stats"), "{html}");
        // Master-Detail 与页签照旧
        assert!(html.contains("pa-provider"), "{html}");
        assert!(html.contains("模型列表"), "{html}");
        let _ = fs::remove_dir_all(&root);
    }

    /// 递归复制目录（测试用：把真实示例拷进临时目录，与用户运行时状态隔离）。
    fn copy_tree(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap().flatten() {
            let to = target.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &to);
            } else if entry.file_name() != ".ssr-h-state.json" {
                fs::copy(entry.path(), &to).unwrap();
            }
        }
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

        let registry = registry_at(&root);
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

    /// Token 天气插件：把"上下文用量"按固定分档渲染成天气。
    ///
    /// 分档口径：0-30 晴天 / 31-60 多云 / 61-85 阴天 / >85 暴雨（边界归上一档）。
    #[test]
    fn token_weather_plugin_maps_percent_to_weather() {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let registry = registry_at(&plugins_dir);
        let plugin = registry
            .find("token-weather")
            .expect("token-weather 插件应当被发现");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);

        let context = |percent: u64| {
            RenderContext::default().with_data(serde_json::json!({
                "tokens": {
                    "used": percent * 1280,
                    "window": 128_000,
                    "percent": percent,
                    "threadId": "t1",
                    "threadTitle": "主会话",
                    "model": "deepseek-chat",
                }
            }))
        };

        for (percent, label) in [
            (0u64, "晴天"),
            (30, "晴天"),
            (31, "多云"),
            (60, "多云"),
            (61, "阴天"),
            (85, "阴天"),
            (86, "暴雨"),
            (100, "暴雨"),
        ] {
            let (_, html) = registry
                .read_page_with(&plugin, "weather", &context(percent))
                .expect("weather page renders");
            assert!(
                html.contains(label),
                "percent={percent} 期望 {label}: {html}"
            );
            assert!(
                html.contains(&format!("{percent}%")),
                "percent={percent} 的百分比没有渲染: {html}"
            );
        }

        // 自刷新走白名单里的 hx-*：hx-select 从整页里取回这个根节点。
        let (_, html) = registry
            .read_page_with(&plugin, "weather", &context(70))
            .expect("weather page renders");
        assert!(html.contains(r#"hx-trigger="every 5s""#), "{html}");
        assert!(html.contains(r##"hx-select="#token-weather""##), "{html}");

        // 网关拿不到用量时：不能 panic，页面上要如实说"无数据"。
        let (_, html) = registry
            .read_page_with(&plugin, "weather", &RenderContext::default())
            .expect("weather page renders without usage");
        assert!(html.contains("无数据"), "{html}");
        assert!(!html.contains("100%"), "{html}");

        // 概览卡片用同一套分档。
        let widgets = registry.dashboard_widgets_with(&context(5));
        assert!(
            widgets
                .iter()
                .any(|widget| widget.plugin_id == "token-weather" && widget.html.contains("晴天")),
            "widgets: {widgets:?}"
        );

        // 输入框上方（composer.above）：一颗胶囊，点进去是完整页面。
        let slot = registry.render_slot_with(Slot::ComposerAbove, &context(70));
        assert_eq!(slot.contributors, 1, "slot: {slot:?}");
        assert!(slot.html.contains("阴天"), "{}", slot.html);
        assert!(slot.html.contains("70%"), "{}", slot.html);
        assert!(
            slot.html.contains(r#"class="ada-icon tw-chip__caret""#),
            "{}",
            slot.html
        );
        assert!(!slot.html.contains("⌄"), "{}", slot.html);
        assert!(
            slot.html.contains(r#"href="/ext/token-weather/weather""#),
            "{}",
            slot.html
        );
        // 没有用量时也不能空着：给一颗"Token 用量"的兜底胶囊。
        let slot = registry.render_slot_with(Slot::ComposerAbove, &RenderContext::default());
        assert!(slot.html.contains("Token 用量"), "{}", slot.html);
    }

    /// 天气插件的展示细节：M/K 缩写 + 缓存命中率（都来自 `props.data.tokens`）。
    #[test]
    fn token_weather_plugin_formats_tokens_and_cache() {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let registry = registry_at(&plugins_dir);
        let plugin = registry
            .find("token-weather")
            .expect("token-weather exists");
        let context = RenderContext::default().with_data(serde_json::json!({
            "tokens": {
                "available": true,
                "used": 18684,
                "window": 1_000_000,
                "percent": 2,
                "cached": 16000,
                "cachePercent": 86,
                "completion": 569,
                "threadId": "t1",
                "threadTitle": "提交代码",
                "model": "senseaudio/deepseek-v4.1-flash"
            }
        }));
        let (_, html) = registry
            .read_page_with(&plugin, "weather", &context)
            .expect("weather page renders");
        assert!(html.contains("18.7K"), "已用应缩写成 18.7K: {html}");
        assert!(html.contains("1M"), "窗口应缩写成 1M: {html}");
        assert!(html.contains("86%"), "缓存命中率没渲染: {html}");
        assert!(html.contains("16K"), "缓存 token 数没渲染: {html}");

        // 输入框上方的浮窗：同一份数据 + 输出 tokens。
        let slot = registry.render_slot_with(Slot::ComposerAbove, &context);
        assert!(slot.html.contains("18.7K / 1M"), "{}", slot.html);
        assert!(slot.html.contains("86%"), "{}", slot.html);
        assert!(
            slot.html.contains("569"),
            "输出 token 数没渲染: {}",
            slot.html
        );
    }

    /// 任务卡片插件：把核心推演好的 `data.todo` 渲染成会话区右上角的悬浮卡。
    #[test]
    fn task_plan_plugin_renders_the_derived_plan() {
        let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend-plugins");
        let registry = registry_at(&plugins_dir);
        let plugin = registry
            .find("task-plan")
            .expect("task-plan 插件应当被发现");
        assert!(plugin.is_active(), "errors: {:?}", plugin.errors);
        assert_eq!(plugin.scripts, vec!["task-plan.js"]);
        assert!(
            registry
                .scripts()
                .iter()
                .any(|script| script.starts_with("/ext/task-plan/assets/task-plan.js?v=")),
            "scripts: {:?}",
            registry.scripts()
        );

        let context = RenderContext::default().with_data(serde_json::json!({
            "todo": {
                "done": 2,
                "total": 3,
                "allDone": false,
                "active": "写条目",
                "notes": "先读再写",
                "markdown": "### 任务规划步骤 (2/3)\n- [x] 读列表\n- [x] 建表\n- [ ] 写条目",
                "steps": [
                    { "id": "t1", "title": "读列表", "status": "completed" },
                    { "id": "t2", "title": "建表", "status": "completed" },
                    { "id": "t3", "title": "写条目", "status": "in_progress" }
                ]
            }
        }));
        let slot = registry.render_slot_with(Slot::ChatCorner, &context);
        assert_eq!(slot.contributors, 1, "slot: {slot:?}");
        assert!(slot.html.contains("任务规划"), "{}", slot.html);
        assert!(slot.html.contains("2/3"), "{}", slot.html);
        assert!(slot.html.contains("2 / 3 已完成"), "{}", slot.html);
        assert!(slot.html.contains("写条目"), "{}", slot.html);
        assert!(slot.html.contains("先读再写"), "{}", slot.html);
        // 步骤清单复用组件库（核心只管数据，标题/状态由插件决定）。
        assert!(slot.html.contains("is-done"), "{}", slot.html);
        assert!(slot.html.contains("is-running"), "{}", slot.html);
        assert!(slot.html.contains("- [x] 读列表"), "{}", slot.html);
        assert!(
            slot.html.contains(r#"data-plugin-scope="task-plan""#),
            "{}",
            slot.html
        );

        // 没有计划时卡片根本不出现：不画 0/0 的空面板。
        let empty = registry.render_slot_with(Slot::ChatCorner, &RenderContext::default());
        assert!(empty.html.is_empty(), "{:?}", empty.html);

        // 默认只挂在 chat.corner；composer.above 不受影响（改 plugin.json 才换位置）。
        let above = registry.render_slot_with(Slot::ComposerAbove, &context);
        assert!(!above.html.contains("任务规划"), "{}", above.html);
    }

    use super::*;

    /// 停用状态必须跨「重启」存活：新建注册表 = 模拟一次重启。
    #[test]
    fn disabled_state_survives_a_registry_restart() {
        let root = temp_root("persist-disabled");
        let _ = write_tsx_plugin(&root);
        let registry = registry_at(&root);
        registry.set_enabled("tsx-demo", false).unwrap();
        assert!(!registry.find("tsx-demo").unwrap().is_active());

        let restarted = registry_at(&root);
        let plugin = restarted.find("tsx-demo").unwrap();
        assert!(!plugin.is_active(), "重启后停用状态必须保留");
        assert!(matches!(plugin.status, PluginStatus::Disabled));

        // 重新启用后重启，同样保持启用
        restarted.set_enabled("tsx-demo", true).unwrap();
        let again = registry_at(&root);
        assert!(again.find("tsx-demo").unwrap().is_active());
        let _ = fs::remove_dir_all(&root);
    }

    /// 状态文件坏了不锁死插件：当作全启用处理，且下次停用会重写文件。
    #[test]
    fn corrupt_state_file_does_not_block_loading() {
        let root = temp_root("corrupt-state");
        let _ = write_tsx_plugin(&root);
        fs::write(root.join(".ssr-h-state.json"), "{not json").unwrap();
        let registry = registry_at(&root);
        assert!(registry.find("tsx-demo").unwrap().is_active());
        registry.set_enabled("tsx-demo", false).unwrap();
        let restarted = registry_at(&root);
        assert!(!restarted.find("tsx-demo").unwrap().is_active());
        let _ = fs::remove_dir_all(&root);
    }

    /// 热重载（同一注册表实例 reload）也要保持停用态——此前靠内存 previous，
    /// 现在两份来源必须一致。
    #[test]
    fn hot_reload_keeps_disabled_state() {
        let root = temp_root("reload-disabled");
        let _ = write_tsx_plugin(&root);
        let registry = registry_at(&root);
        registry.set_enabled("tsx-demo", false).unwrap();
        registry.reload().unwrap();
        assert!(!registry.find("tsx-demo").unwrap().is_active());
        let _ = fs::remove_dir_all(&root);
    }

}
