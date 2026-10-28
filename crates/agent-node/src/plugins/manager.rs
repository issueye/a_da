use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use base64::Engine as _;

use super::builtins::get_builtin_plugin_items;
use super::types::{
    LoadedPlugin, LoadedPluginContributions, PluginActionError, PluginActionIntent,
    PluginCapabilities, PluginDiagnostic, PluginItem, PluginManifest, PluginRouteResponse,
    PluginScope, ResolvedPluginCapabilitiesDto,
};
use crate::session::{get_app_home, get_config_path};

/// 插件管理器。
///
/// 🔴 为什么带一个 `home`：早先它是纯 unit struct，内部直接 `get_config_path()` /
/// `get_app_home()`。于是**跑一次插件相关单测就会改写用户真实的 `~/.a-da/config.json`**
/// （同一进程里 `set_var("A_DA_HOME")` 对 `OnceLock` 化的端口无效，测试注入不进去）。
/// 现在测试/嵌入式宿主用 [`PluginManager::with_home`] 注入自己的应用目录，生产路径不变。
pub struct PluginManager {
    /// `None` = 进程默认应用目录（生产）；`Some` = 注入目录（测试 / 嵌入式宿主）。
    home: Option<PathBuf>,
    /// 统一包根覆盖（测试 / 嵌入式宿主）；`None` 时读环境变量或 `<app_home>/plugins`。
    unified_dir: Option<PathBuf>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            home: None,
            unified_dir: None,
        }
    }

    /// 注入应用目录（测试 / 嵌入式宿主用；生产路径走 [`PluginManager::new`]）。
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
            unified_dir: None,
        }
    }

    /// 注入统一包根（测试 / 嵌入式宿主；生产默认 `<app_home>/plugins`）。
    pub fn with_unified_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.unified_dir = Some(dir.into());
        self
    }

    /// `config.json` 落点
    fn config_path(&self) -> PathBuf {
        match &self.home {
            Some(h) => h.join("config.json"),
            None => get_config_path(),
        }
    }

    /// 应用目录根
    /// 应用目录（`~/.a-da`）——**对外只读视图**，给工具装配用。
    ///
    /// `create_plugin` 的 `scope="global"` 要落到 `<app_home>/agent-coding/extensions`，
    /// 而这个路径只能由持有应用目录端口的一方给出：`with_home` 注入的实例（测试）返回
    /// 临时目录，生产返回真实的 `~/.a-da`——工具层因此不需要、也不许自己读环境变量。
    pub fn app_home_dir(&self) -> PathBuf {
        self.app_home()
    }

    fn app_home(&self) -> PathBuf {
        match &self.home {
            Some(h) => h.clone(),
            None => get_app_home(),
        }
    }

    /// Canonical workspace extension directory used by the agent runtime.
    pub fn agent_extensions_dir(workspace: &str) -> PathBuf {
        PathBuf::from(workspace).join(".ada").join("agent-coding").join("extensions")
    }

    /// 统一插件包根：一个目录里同时有 `plugin.json` 的 `frontend` / `backend` 声明。
    ///
    /// 顺序：显式注入 → 环境变量 → 开发树（仅 debug）→ `<app_home>/plugins` →
    /// 与 exe 同级的 `plugins/`。前端注册表用同一个根扫描，于是同一个包只装一次、
    /// 两侧都能发现。
    pub fn unified_extensions_dir(&self) -> PathBuf {
        if let Some(dir) = &self.unified_dir {
            return dir.clone();
        }
        if let Ok(dir) = std::env::var("A_DA_UNIFIED_PLUGINS_DIR") {
            if !dir.trim().is_empty() {
                return PathBuf::from(dir.trim());
            }
        }
        // 注入了 home 的实例（测试 / 嵌入式宿主）只认自己的 plugins 目录：
        // 否则 debug 构建的开发树回退会把仓库插件带进隔离测试。
        if self.home.is_some() {
            return self.app_home().join("plugins");
        }
        let dev_tree = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        if cfg!(debug_assertions) && dev_tree.is_dir() {
            return dev_tree;
        }
        let home = self.app_home().join("plugins");
        if home.is_dir() {
            return home;
        }
        if let Some(exe_side) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join("plugins")))
            .filter(|path| path.is_dir())
        {
            return exe_side;
        }
        home
    }

    /// Validate an explicit plugin entry: only canonical package-root
    /// `index.ts` / `index.js` files are accepted, never loose legacy scripts.
    pub fn is_extension_entry(&self, workspace: Option<&str>, entry: &Path) -> bool {
        if !entry.is_file()
            || !entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "index.ts" | "index.js"))
        {
            return false;
        }
        let Ok(entry) = fs::canonicalize(entry) else { return false };
        let mut roots = vec![
            self.app_home().join("agent-coding").join("extensions"),
            self.unified_extensions_dir(),
        ];
        if let Some(workspace) = workspace.filter(|workspace| !workspace.trim().is_empty()) {
            roots.push(Self::agent_extensions_dir(workspace));
        }
        roots.into_iter().any(|root| {
            fs::canonicalize(root)
                .ok()
                .is_some_and(|root| entry.parent().and_then(Path::parent) == Some(root.as_path()))
        })
    }

    /// Load the current plugin view from the canonical agent directory.
    pub fn load_plugins(&self, workspace: Option<&str>) -> Vec<PluginItem> {
        self.scan_plugins(workspace)
    }

    /// Explicit reload entry point. Kept separate from `load_plugins` so RPC
    /// callers can express intent and future caching can be added in one place.
    pub fn reload_plugins(&self, workspace: Option<&str>) -> Vec<PluginItem> {
        self.scan_plugins(workspace)
    }

    /// 调用一个**面向界面**的插件动作（`plugin.invoke` 的实现）。
    ///
    /// 动作不是模型工具：这里不碰审批、不产生工具卡、不进入轮次。调用方只负责
    /// 声明意图（read/write）；动作自己的 `mode` 由沙箱在执行前校验。
    pub async fn invoke_action(
        &self,
        plugin_id: &str,
        action: &str,
        input: serde_json::Value,
        intent: PluginActionIntent,
        workspace: &str,
    ) -> Result<serde_json::Value, PluginActionError> {
        const MAX_INPUT_BYTES: usize = 64 * 1024;
        const MAX_OUTPUT_BYTES: usize = 256 * 1024;
        const ACTION_TIMEOUT_SECS: u64 = 5;

        let plugin_id = plugin_id.trim();
        let action = action.trim();
        let workspace = workspace.trim();
        if plugin_id.is_empty() || action.is_empty() {
            return Err(PluginActionError::Invalid(
                "plugin.invoke 需要非空的 pluginId 与 action".to_string(),
            ));
        }
        if workspace.is_empty() {
            return Err(PluginActionError::Invalid(
                "plugin.invoke 需要非空的工作区".to_string(),
            ));
        }
        let input_bytes = serde_json::to_vec(&input)
            .map_err(|error| PluginActionError::Invalid(format!("动作入参不是合法 JSON: {error}")))?
            .len();
        if input_bytes > MAX_INPUT_BYTES {
            return Err(PluginActionError::TooLarge);
        }

        let (path, needs) = self.ready_plugin_entry(plugin_id, workspace).await?;
        let context = serde_json::json!({
            "workspace": workspace,
            "caller": "web",
            "now": agent_toolkit::now_ms(),
        });

        let call = super::PluginSandbox::call_action_budgeted(
            &path,
            action,
            input,
            context,
            intent.as_str(),
            Path::new(workspace),
            ACTION_TIMEOUT_SECS,
            needs,
            ada_ts_engine::Limits::default(),
        );
        let result = match tokio::time::timeout(
            Duration::from_secs(ACTION_TIMEOUT_SECS + 1),
            call,
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                let message = error.to_string();
                if message.contains("超时") {
                    return Err(PluginActionError::Timeout);
                }
                return Err(PluginActionError::Failed(message));
            }
            Err(_) => return Err(PluginActionError::Timeout),
        };

        match result.get("ok").and_then(|value| value.as_bool()) {
            Some(true) => {
                let data = result.get("data").cloned().unwrap_or(serde_json::Value::Null);
                let data_bytes = serde_json::to_vec(&data)
                    .map_err(|error| {
                        PluginActionError::Failed(format!("动作返回值不是合法 JSON: {error}"))
                    })?
                    .len();
                if data_bytes > MAX_OUTPUT_BYTES {
                    return Err(PluginActionError::TooLarge);
                }
                Ok(serde_json::json!({ "ok": true, "data": data }))
            }
            Some(false) => {
                let message = result
                    .get("error")
                    .and_then(|value| value.as_str())
                    .unwrap_or("动作执行失败")
                    .to_string();
                match result.get("kind").and_then(|value| value.as_str()) {
                    Some("not_found") => Err(PluginActionError::NotFound(message)),
                    Some("denied") => Err(PluginActionError::Denied(message)),
                    _ => Err(PluginActionError::Failed(message)),
                }
            }
            None => Err(PluginActionError::Failed(
                "动作返回缺少 ok 字段".to_string(),
            )),
        }
    }

    /// 调用插件声明的**动态 HTTP 路由**（`plugin.route` 的实现）。
    ///
    /// 路由表在 TS 侧（`ada.route`），所以这里只负责：找到插件入口、按声明的能力
    /// 建沙箱、把请求原样递进去、把归一化后的 HTTP 响应带回来。加一条后端接口
    /// 不需要改这一层。
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke_route(
        &self,
        plugin_id: &str,
        method: &str,
        path: &str,
        query: serde_json::Value,
        headers: serde_json::Value,
        body: serde_json::Value,
        body_kind: &str,
        thread_id: Option<&str>,
        workspace: &str,
    ) -> Result<PluginRouteResponse, PluginActionError> {
        const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
        const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
        const ROUTE_TIMEOUT_SECS: u64 = 15;

        let plugin_id = plugin_id.trim();
        let method = method.trim().to_ascii_uppercase();
        let path = path.trim();
        let workspace = workspace.trim();
        let body_kind = body_kind.trim().to_ascii_lowercase();
        if plugin_id.is_empty() || method.is_empty() || path.is_empty() {
            return Err(PluginActionError::Invalid(
                "plugin.route 需要非空的 pluginId / method / path".to_string(),
            ));
        }
        if workspace.is_empty() {
            return Err(PluginActionError::Invalid(
                "plugin.route 需要非空的工作区".to_string(),
            ));
        }
        if !matches!(
            method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
        ) {
            return Err(PluginActionError::Invalid(format!(
                "不支持的 HTTP 方法: {method}"
            )));
        }
        if path.len() > 2048
            || !path.starts_with('/')
            || path.contains('\0')
            || path.split('/').any(|part| part == "." || part == "..")
        {
            return Err(PluginActionError::Invalid(format!(
                "路由路径不合法: {path}"
            )));
        }
        if !matches!(
            body_kind.as_str(),
            "" | "empty" | "json" | "form" | "text" | "base64"
        ) {
            return Err(PluginActionError::Invalid(format!(
                "不支持的请求体类型: {body_kind}"
            )));
        }

        let now = agent_toolkit::now_ms();
        let request = serde_json::json!({
            "method": method,
            "path": path,
            "query": query,
            "headers": headers,
            "body": body,
            "bodyKind": body_kind,
            "workspace": workspace,
            "threadId": thread_id,
            "now": now,
        });
        let input_bytes = serde_json::to_vec(&request)
            .map_err(|error| PluginActionError::Invalid(format!("路由入参不是合法 JSON: {error}")))?
            .len();
        if input_bytes > MAX_INPUT_BYTES {
            return Err(PluginActionError::TooLarge);
        }

        let (plugin_path, needs) = self.ready_plugin_entry(plugin_id, workspace).await?;
        let call = super::PluginSandbox::call_route_budgeted(
            &plugin_path,
            &method,
            path,
            request,
            Path::new(workspace),
            ROUTE_TIMEOUT_SECS,
            needs,
            ada_ts_engine::Limits::default(),
        );
        let result = match tokio::time::timeout(
            Duration::from_secs(ROUTE_TIMEOUT_SECS + 1),
            call,
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                let message = error.to_string();
                if message.contains("超时") {
                    return Err(PluginActionError::Timeout);
                }
                return Err(PluginActionError::Failed(message));
            }
            Err(_) => return Err(PluginActionError::Timeout),
        };

        match result.get("ok").and_then(|value| value.as_bool()) {
            Some(true) => {
                let response = result
                    .get("response")
                    .and_then(|value| value.as_object())
                    .ok_or_else(|| {
                        PluginActionError::Failed("路由返回缺少 response 对象".to_string())
                    })?;
                let status = response
                    .get("status")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(200);
                if !(200..=599).contains(&status) {
                    return Err(PluginActionError::Invalid(format!(
                        "路由返回了非法 HTTP 状态码: {status}"
                    )));
                }
                let body = response
                    .get("body")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                if body.len() > MAX_OUTPUT_BYTES {
                    return Err(PluginActionError::TooLarge);
                }
                let body_kind = match response.get("bodyKind").and_then(|value| value.as_str()) {
                    Some("base64") => "base64",
                    _ => "text",
                };
                if body_kind == "base64"
                    && base64::engine::general_purpose::STANDARD
                        .decode(body.as_bytes())
                        .is_err()
                {
                    return Err(PluginActionError::Failed(
                        "路由返回的 base64 响应体不合法".to_string(),
                    ));
                }
                let content_type = response
                    .get("contentType")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let headers = normalize_route_headers(
                    response.get("headers"),
                    content_type,
                    body_kind,
                )?;
                Ok(PluginRouteResponse {
                    status: status as u16,
                    headers,
                    body,
                    body_kind: body_kind.to_string(),
                })
            }
            Some(false) => {
                let message = result
                    .get("error")
                    .and_then(|value| value.as_str())
                    .unwrap_or("路由执行失败")
                    .to_string();
                match result.get("kind").and_then(|value| value.as_str()) {
                    Some("not_found") => Err(PluginActionError::NotFound(message)),
                    Some("denied") => Err(PluginActionError::Denied(message)),
                    Some("invalid") => Err(PluginActionError::Invalid(message)),
                    Some("too_large") => Err(PluginActionError::TooLarge),
                    _ => Err(PluginActionError::Failed(message)),
                }
            }
            None => Err(PluginActionError::Failed(
                "路由返回缺少 ok 字段".to_string(),
            )),
        }
    }

    /// 找到插件入口并读出它声明的沙箱能力（动作与动态路由共用）。
    async fn ready_plugin_entry(
        &self,
        plugin_id: &str,
        workspace: &str,
    ) -> Result<(PathBuf, ada_ts_engine::Needs), PluginActionError> {
        let item = self
            .scan_plugins(Some(workspace))
            .into_iter()
            .find(|item| item.id == plugin_id)
            .ok_or_else(|| {
                PluginActionError::NotFound(format!("插件不存在: {plugin_id}"))
            })?;
        if !item.enabled || item.status != "ready" {
            return Err(PluginActionError::NotReady(format!(
                "插件未启用或未就绪: {plugin_id}"
            )));
        }
        if item.file_path.trim().is_empty() {
            return Err(PluginActionError::NotReady(format!(
                "插件没有可装载的入口: {plugin_id}"
            )));
        }

        let path = PathBuf::from(&item.file_path);
        let source = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| {
                PluginActionError::Failed(format!("读取插件失败（{}）：{error}", path.display()))
            })?;
        let declared = crate::scripts::needs::parse_needs(&source);
        let needs = crate::scripts::to_engine_needs(declared.needs);
        Ok((path, needs))
    }

    /// 获取配置中已禁用的插件集合
    pub fn load_disabled_plugins(&self) -> HashSet<String> {
        let mut disabled = HashSet::new();
        let cfg_file = self.config_path();
        if cfg_file.exists() {
            if let Ok(content) = fs::read_to_string(&cfg_file) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(arr) = val.get("disabledPlugins").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                disabled.insert(s.to_string());
                            }
                        }
                    }
                }
            }
        }
        disabled
    }

    /// 切换插件启停状态并持久化至 ~/.a-da/config.json
    pub fn toggle_plugin(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        let cfg_file = self.config_path();
        if let Some(parent) = cfg_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut val = if cfg_file.exists() {
            let content = fs::read_to_string(&cfg_file)?;
            serde_json::from_str::<serde_json::Value>(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let mut current_disabled: Vec<String> = val
            .get("disabledPlugins")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if enabled {
            current_disabled.retain(|id| id != plugin_id);
        } else {
            if !current_disabled.contains(&plugin_id.to_string()) {
                current_disabled.push(plugin_id.to_string());
            }
        }

        val["disabledPlugins"] = serde_json::json!(current_disabled);
        let output = serde_json::to_string_pretty(&val)?;
        fs::write(&cfg_file, output)?;
        Ok(())
    }

    /// 读取插件能力开关
    pub fn get_capabilities(&self, _workspace: Option<&str>) -> ResolvedPluginCapabilitiesDto {
        let mut caps = PluginCapabilities::default();
        let cfg_file = self.config_path();
        if cfg_file.exists() {
            if let Ok(content) = fs::read_to_string(&cfg_file) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(pc) = val.get("pluginCapabilities").and_then(|v| v.as_object()) {
                        if let Some(v) = pc.get("allowSystemPromptReplace").and_then(|b| b.as_bool()) {
                            caps.allow_system_prompt_replace = v;
                        }
                        if let Some(v) = pc.get("allowTextRewrite").and_then(|b| b.as_bool()) {
                            caps.allow_text_rewrite = v;
                        }
                        if let Some(v) = pc.get("allowThreadDeleteBlock").and_then(|b| b.as_bool()) {
                            caps.allow_thread_delete_block = v;
                        }
                        if let Some(v) = pc.get("allowCompactionReplace").and_then(|b| b.as_bool()) {
                            caps.allow_compaction_replace = v;
                        }
                        if let Some(v) = pc
                            .get("allowThirdPartyEvents")
                            .or_else(|| pc.get("allowThirdPartyHooks"))
                            .and_then(|b| b.as_bool())
                        {
                            caps.allow_third_party_events = v;
                        }
                        if let Some(v) = pc.get("allowBuiltinShadow").and_then(|b| b.as_bool()) {
                            caps.allow_builtin_shadow = v;
                        }
                        if let Some(v) = pc
                            .get("eventTimeoutMs")
                            .or_else(|| pc.get("hookTimeoutMs"))
                            .and_then(|n| n.as_u64())
                        {
                            caps.event_timeout_ms = v;
                        }
                    }
                }
            }
        }

        ResolvedPluginCapabilitiesDto {
            capabilities: caps,
            invalid: Vec::new(),
            overrides: HashMap::new(),
        }
    }

    /// 保存插件能力设置
    pub fn save_capabilities(&self, patch: &serde_json::Value) -> Result<()> {
        let cfg_file = self.config_path();
        if let Some(parent) = cfg_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut val = if cfg_file.exists() {
            let content = fs::read_to_string(&cfg_file)?;
            serde_json::from_str::<serde_json::Value>(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        if !val.is_object() {
            val = serde_json::json!({});
        }
        let root = val.as_object_mut().unwrap();
        let existing = root.entry("pluginCapabilities").or_insert_with(|| serde_json::json!({}));
        if let (Some(existing_obj), Some(patch_obj)) = (existing.as_object_mut(), patch.as_object()) {
            for (k, v) in patch_obj {
                existing_obj.insert(k.clone(), v.clone());
            }
        }

        let output = serde_json::to_string_pretty(&val)?;
        fs::write(&cfg_file, output)?;
        Ok(())
    }

    /// 保存插件特定配置
    pub fn save_config(&self, plugin_id: &str, values: &serde_json::Value) -> Result<()> {
        let cfg_file = self.config_path();
        if let Some(parent) = cfg_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut val = if cfg_file.exists() {
            let content = fs::read_to_string(&cfg_file)?;
            serde_json::from_str::<serde_json::Value>(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        if !val.is_object() {
            val = serde_json::json!({});
        }
        let root = val.as_object_mut().unwrap();
        let configs = root.entry("pluginConfig").or_insert_with(|| serde_json::json!({}));
        if let Some(configs_obj) = configs.as_object_mut() {
            let plugin_val = configs_obj.entry(plugin_id).or_insert_with(|| serde_json::json!({}));
            if let (Some(target_obj), Some(new_obj)) = (plugin_val.as_object_mut(), values.as_object()) {
                for (k, v) in new_obj {
                    target_obj.insert(k.clone(), v.clone());
                }
            } else {
                configs_obj.insert(plugin_id.to_string(), values.clone());
            }
        }

        let output = serde_json::to_string_pretty(&val)?;
        fs::write(&cfg_file, output)?;
        Ok(())
    }

    /// 读取全部插件配置
    pub fn read_configs(&self) -> serde_json::Value {
        let cfg_file = self.config_path();
        if cfg_file.exists() {
            if let Ok(content) = fs::read_to_string(&cfg_file) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(c) = val.get("pluginConfig") {
                        return c.clone();
                    }
                }
            }
        }
        serde_json::json!({})
    }

    /// 保存插件密钥
    pub fn save_secret(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        let secrets_dir = self.app_home().join("secrets");
        fs::create_dir_all(&secrets_dir)?;
        let file_path = secrets_dir.join(format!("{}_{}", plugin_id, key));
        if value.trim().is_empty() {
            let _ = fs::remove_file(&file_path);
        } else {
            fs::write(&file_path, value.trim())?;
        }
        Ok(())
    }

    /// 读取插件密钥是否存在
    pub fn check_secret(&self, plugin_id: &str, key: &str) -> bool {
        let file_path = self.app_home().join("secrets").join(format!("{}_{}", plugin_id, key));
        if file_path.exists() {
            if let Ok(content) = fs::read_to_string(&file_path) {
                return !content.trim().is_empty();
            }
        }
        false
    }

    /// 扫描并获取所有可用插件（包含系统 9 大内置插件 + 用户全局扩展 + 工作区扩展）
    pub fn scan_plugins(&self, workspace: Option<&str>) -> Vec<PluginItem> {
        let disabled = self.load_disabled_plugins();
        let mut items = Vec::new();
        let mut seen_ids = HashSet::new();

        // 1. 系统 9 大官方内置插件
        let builtin_items = get_builtin_plugin_items(&disabled);
        for item in builtin_items {
            seen_ids.insert(item.id.clone());
            items.push(item);
        }

        // 2. Scan canonical package roots only. The Extension Factory is the
        // source of truth; loose files and alternate folders are not discovered.
        // 第三项 `unified` 表示“统一插件包根”：只收 plugin.json 里声明了 `backend`
        // 的包；只有前端声明的同目录包由 agent-web 处理，不在这里报 broken。
        let mut scan_dirs: Vec<(PathBuf, &'static str, bool)> = Vec::new();

        if let Some(ws) = workspace {
            if !ws.trim().is_empty() {
                scan_dirs.push((Self::agent_extensions_dir(ws), "workspace", false));
            }
        }

        let app_home = self.app_home();
        scan_dirs.push((app_home.join("agent-coding").join("extensions"), "global", false));
        scan_dirs.push((self.unified_extensions_dir(), "global", true));

        for (dir_path, scope, unified) in scan_dirs {
            if !dir_path.exists() {
                continue;
            }

            let entries = match fs::read_dir(&dir_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = match path.file_name().and_then(|s| s.to_str()) {
                    Some(name) => name,
                    None => continue,
                };

                if file_name.starts_with('.') || file_name == "node_modules" || file_name == "target" {
                    continue;
                }
                if !path.is_dir() {
                    continue;
                }

                let unified_manifest = if unified {
                    let manifest_path = path.join("plugin.json");
                    let Ok(raw) = fs::read_to_string(&manifest_path) else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        continue;
                    };
                    // 只有显式声明了 backend 对象的统一包才归 agent 管。
                    // 前端-only 包不在这里列成“missing index.ts”的坏插件。
                    if !value.get("backend").is_some_and(|backend| backend.is_object()) {
                        continue;
                    }
                    Some(value)
                } else {
                    None
                };

                let id = if let Some(manifest) = &unified_manifest {
                    manifest
                        .get("backend")
                        .and_then(|backend| backend.get("pluginId"))
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("global:{}", file_name.trim_end_matches(".ts").trim_end_matches(".js")))
                } else {
                    format!(
                        "{}:{}",
                        scope,
                        file_name.trim_end_matches(".ts").trim_end_matches(".js")
                    )
                };
                if seen_ids.contains(&id) {
                    continue;
                }
                seen_ids.insert(id.clone());

                let mut size_bytes = 0u64;
                let mut updated_at = 1727740800000u64;
                if let Ok(meta) = path.metadata() {
                    size_bytes = meta.len();
                    if let Ok(mtime) = meta.modified() {
                        if let Ok(duration) = mtime.duration_since(std::time::UNIX_EPOCH) {
                            updated_at = duration.as_millis() as u64;
                        }
                    }
                }

                let plugin_name = unified_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.get("name"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| file_name.to_string());

                let enabled = !disabled.contains(&id);

                // Contributions belong to the extension factory runtime. Do not
                // infer registrations by grepping JavaScript source here.
                let tools = Vec::new();
                let tools_decl = Vec::new();
                let script_path = [path.join("index.ts"), path.join("index.js")]
                    .into_iter()
                    .find(|index| index.is_file());

                let desc = unified_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.get("description"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("{} 扩展插件", if scope == "global" { "全局" } else { "工作区" })
                    });
                let version = unified_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.get("version"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "1.0.0".to_string());

                // 埋点：把"这个插件为什么不会生效"如实写进诊断字段。
                //
                // 以前这里恒为 `Vec::new()`，于是插件卡永远显示不出任何问题。P0 当时还补过一条
                // 「脚本执行面尚未接线，工具不会被模型调用」——**P1 已经把它接上了**
                // （`ScriptHost::load` 会真的执行一遍插件，贡献进装配），
                // 那条诊断等于对着已经生效的插件喊"不生效"，已删除。
                let mut diagnostics: Vec<PluginDiagnostic> = Vec::new();
                let package_entry_missing = script_path.is_none();
                let plugin_status = if package_entry_missing { "broken" } else { "ready" };
                if package_entry_missing {
                    diagnostics.push(PluginDiagnostic {
                        plugin_id: id.clone(),
                        level: "error".to_string(),
                        message: "插件包根目录必须包含 index.ts 或 index.js".to_string(),
                        hint: Some("请将 pi-style Extension Factory 放在包根 index.ts（或编译后的 index.js）".to_string()),
                    });
                }
                if !enabled {
                    diagnostics.push(PluginDiagnostic {
                        plugin_id: id.clone(),
                        level: "warn".to_string(),
                        message: "插件已停用：它的贡献不会参与装配".to_string(),
                        hint: None,
                    });
                }
                if script_path.is_some() {
                    diagnostics.push(PluginDiagnostic {
                        plugin_id: id.clone(),
                        level: "info".to_string(),
                        message: "静态目录只列包与入口状态；实际贡献由 Extension Factory 执行后确定（见工具/技能/钩子计数）".to_string(),
                        hint: Some("此处的空数组不代表插件没有贡献——贡献计数由装配期执行结果填入".to_string()),
                    });
                }

                let manifest = PluginManifest {
                    id: id.clone(),
                    name: plugin_name.clone(),
                    description: desc.clone(),
                    version: Some(version.clone()),
                    author: None,
                    scope: Some(if scope == "global" {
                        PluginScope::Global
                    } else {
                        PluginScope::Workspace
                    }),
                };

                let loaded_plugin = LoadedPlugin {
                    manifest,
                    contributions: LoadedPluginContributions {
                        tools: tools_decl,
                        config_schema: None,
                    },
                    declarative: false,
                    status: plugin_status.to_string(),
                    diagnostics: diagnostics.clone(),
                };

                items.push(PluginItem {
                    plugin: loaded_plugin,
                    id,
                    name: plugin_name,
                    file_name: file_name.to_string(),
                    file_path: script_path.unwrap_or(path).to_string_lossy().to_string(),
                    scope: scope.to_string(),
                    enabled,
                    status: plugin_status.to_string(),
                    version: Some(version),
                    diagnostics,
                    tools,
                    skills: Vec::new(),
                    prompts: Vec::new(),
                    events: Vec::new(),
                    is_package: Some(true),
                    error: if package_entry_missing {
                        Some("missing index.ts/index.js".to_string())
                    } else {
                        None
                    },
                    size_bytes,
                    updated_at,
                });
            }
        }

        items
    }

    /// 创建插件模板文件
    pub fn create_plugin_template(
        &self,
        workspace: Option<&str>,
        scope: &str,
        name: &str,
        code: Option<&str>,
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty()
            || name == "."
            || name == ".."
            || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            anyhow::bail!("插件包名只能包含字母、数字、点、下划线和连字符");
        }
        let dir = if scope == "workspace" && workspace.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            Self::agent_extensions_dir(workspace.unwrap())
        } else {
            self.app_home().join("agent-coding").join("extensions")
        };

        fs::create_dir_all(&dir)?;
        let package_dir = dir.join(name);
        fs::create_dir(&package_dir)?;
        let target = package_dir.join("index.ts");

        let default_code = format!(
            r#"const metadata = {{
  name: {name:?},
  description: '自定义 Agent 扩展',
  // 发布到插件市场（plugin-hub）时 author / version 是必填项
  author: 'a_da',
  version: '0.1.0',
  tools: {{ count: 0, names: [] }},
  systemPrompt: {{ count: 0, names: [] }},
  skills: {{ count: 0, names: [] }},
  events: {{ count: 0, names: [] }},
}};

export default function (ada) {{
  ada.metadata = metadata;
  // Pi-style registration API:
  // ada.registerTool({{ name, description, parameters, execute }});
  // ada.registerSystemPrompt({{ id, content }});
  // ada.on('event_after_turn', async (event, ctx) => {{ ... }});
  // 技能是目录约定：skills/<name>/SKILL.md
}}
"#,
            name = name
        );

        fs::write(&target, code.unwrap_or(&default_code))?;
        Ok(target.to_string_lossy().to_string())
    }

    /// 删除插件文件
    pub fn delete_plugin(&self, file_path: &str, _workspace: Option<&str>) -> Result<bool> {
        let p = PathBuf::from(file_path);
        if p.exists() {
            if p.is_file() {
                let is_package_entry = p.file_name().and_then(|name| name.to_str()).is_some_and(|name| matches!(name, "index.ts" | "index.js"))
                    && p.parent()
                        .and_then(|parent| parent.parent())
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name == "extensions");
                if is_package_entry {
                    if let Some(package_dir) = p.parent() {
                        fs::remove_dir_all(package_dir)?;
                    }
                } else {
                    fs::remove_file(&p)?;
                }
            } else if p.is_dir() {
                fs::remove_dir_all(&p)?;
            }
            return Ok(true);
        }
        Ok(false)
    }
}

/// 归一化动态路由返回的响应头。
///
/// 这里是**最后一道**头校验：头名小写、拒绝 CR/LF、丢弃逐跳头、限制数量与体积。
/// 浏览器侧还会再按 axum 的 `HeaderName/HeaderValue` 解析一次；两层都过才落到响应上。
fn normalize_route_headers(
    raw: Option<&serde_json::Value>,
    content_type: Option<&str>,
    body_kind: &str,
) -> Result<BTreeMap<String, Vec<String>>, PluginActionError> {
    const MAX_HEADERS: usize = 64;
    const MAX_HEADER_BYTES: usize = 32 * 1024;
    const HOP_BY_HOP: &[&str] = &[
        "content-length",
        "transfer-encoding",
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "upgrade",
        "host",
    ];

    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut total = 0usize;
    if let Some(object) = raw.and_then(|value| value.as_object()) {
        for (name, value) in object {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty()
                || name.len() > 100
                || !name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            {
                return Err(PluginActionError::Invalid(format!(
                    "响应头名不合法: {name}"
                )));
            }
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            if out.len() >= MAX_HEADERS {
                return Err(PluginActionError::TooLarge);
            }
            let mut values = Vec::new();
            match value {
                serde_json::Value::String(text) => values.push(text.clone()),
                serde_json::Value::Array(items) => {
                    for item in items {
                        let Some(text) = item.as_str() else {
                            return Err(PluginActionError::Invalid(format!(
                                "响应头 {name} 的值必须是字符串或字符串数组"
                            )));
                        };
                        values.push(text.to_string());
                    }
                }
                _ => {
                    return Err(PluginActionError::Invalid(format!(
                        "响应头 {name} 的值必须是字符串或字符串数组"
                    )))
                }
            }
            for text in &values {
                if text.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
                    return Err(PluginActionError::Invalid(format!(
                        "响应头 {name} 含有换行"
                    )));
                }
                total += name.len() + text.len();
            }
            if total > MAX_HEADER_BYTES {
                return Err(PluginActionError::TooLarge);
            }
            out.insert(name, values);
        }
    }

    if !out.contains_key("content-type") {
        let fallback = if body_kind == "base64" {
            "application/octet-stream"
        } else {
            "text/plain; charset=utf-8"
        };
        let text = content_type.unwrap_or(fallback).trim();
        if text.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
            return Err(PluginActionError::Invalid(
                "响应 Content-Type 含有换行".to_string(),
            ));
        }
        out.insert("content-type".to_string(), vec![text.to_string()]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "a_da_plugin_manager_{tag}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn scanner_ignores_legacy_loose_files_and_alternate_plugin_directory() {
        let workspace = temp_dir("legacy_scan");
        let canonical = PluginManager::agent_extensions_dir(&workspace.to_string_lossy());
        fs::create_dir_all(&canonical).unwrap();
        fs::write(canonical.join("loose.ts"), "export default function (ada) {} ").unwrap();
        let alternate = workspace.join(".ada").join("plugins");
        fs::create_dir_all(&alternate).unwrap();
        fs::write(alternate.join("old.ts"), "export default function (ada) {} ").unwrap();

        let manager = PluginManager::with_home(temp_dir("legacy_scan_home"));
        let plugins = manager.scan_plugins(Some(&workspace.to_string_lossy()));
        assert!(plugins.iter().all(|plugin| plugin.scope == "builtin"));
        assert!(!manager.is_extension_entry(Some(&workspace.to_string_lossy()), &canonical.join("loose.ts")));
        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn template_is_pi_style_package_and_delete_removes_package_root() {
        let workspace = temp_dir("workspace");
        let home = temp_dir("home");
        let manager = PluginManager::with_home(&home);
        let entry = manager
            .create_plugin_template(Some(workspace.to_string_lossy().as_ref()), "workspace", "factory-demo", None)
            .expect("create package template");
        let entry = PathBuf::from(entry);
        assert_eq!(entry.file_name().and_then(|name| name.to_str()), Some("index.ts"));
        let source = fs::read_to_string(&entry).unwrap();
        assert!(source.contains("export default function (ada)"));
        assert!(source.contains("ada.metadata = metadata"));

        let inspected = crate::plugins::PluginSandbox::inspect_plugin_full(
            &entry,
            &workspace,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("generated package should satisfy the metadata contract");
        assert_eq!(inspected.metadata.as_ref().and_then(|m| m.get("name")).and_then(|v| v.as_str()), Some("factory-demo"));
        assert!(inspected.tools.is_empty());

        assert!(manager.delete_plugin(entry.to_string_lossy().as_ref(), None).unwrap());
        assert!(!entry.parent().unwrap().exists(), "deleting package entry removes its package root");
        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn unified_package_is_scanned_and_actions_enforce_intent() {
        let workspace = temp_dir("unified_ws");
        let home = temp_dir("unified_home");
        let unified = temp_dir("unified_root");
        let package = unified.join("unified-demo");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "id": "unified-demo",
                "name": "Unified Demo",
                "version": "0.1.0",
                "frontend": { "slots": [] },
                "backend": {}
            }"#,
        )
        .unwrap();
        fs::write(
            package.join("index.ts"),
            r#"
            // @ada-needs none
            const metadata = {
              name: 'unified-demo',
              description: '统一包动作测试',
              tools: { count: 0, names: [] },
              systemPrompt: { count: 0, names: [] },
              events: { count: 0, names: [] },
              actions: { count: 2, names: ['status', 'refresh'] },
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.registerAction({ name: 'status', description: '读', mode: 'read', async execute() { return { ok: true, data: { value: 1 } }; } });
              ada.registerAction({ name: 'refresh', description: '写', mode: 'write', async execute() { return { ok: true, data: { value: 2 } }; } });
            }
            "#,
        )
        .unwrap();

        let manager = PluginManager::with_home(&home).with_unified_dir(&unified);
        let items = manager.scan_plugins(Some(workspace.to_string_lossy().as_ref()));
        let item = items
            .iter()
            .find(|item| item.id == "global:unified-demo")
            .expect("统一包必须被 agent 侧扫描到");
        assert_eq!(item.name, "Unified Demo");
        assert_eq!(item.version.as_deref(), Some("0.1.0"));
        assert!(item.enabled);

        let read = manager
            .invoke_action(
                "global:unified-demo",
                "status",
                serde_json::json!({}),
                PluginActionIntent::Read,
                workspace.to_string_lossy().as_ref(),
            )
            .await
            .expect("只读动作应成功");
        assert_eq!(read["data"]["value"], 1);

        let denied = manager
            .invoke_action(
                "global:unified-demo",
                "refresh",
                serde_json::json!({}),
                PluginActionIntent::Read,
                workspace.to_string_lossy().as_ref(),
            )
            .await
            .expect_err("read 意图不能执行 write 动作");
        assert!(matches!(denied, PluginActionError::Denied(_)));

        let written = manager
            .invoke_action(
                "global:unified-demo",
                "refresh",
                serde_json::json!({}),
                PluginActionIntent::Write,
                workspace.to_string_lossy().as_ref(),
            )
            .await
            .expect("write 动作应成功");
        assert_eq!(written["data"]["value"], 2);

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(unified);
    }

    #[test]
    fn route_headers_are_normalized_and_crlf_rejected() {
        let raw = serde_json::json!({
            "Set-Cookie": ["a=1", "b=2"],
            "x-ok": "1",
            "content-length": "10",
        });
        let headers = normalize_route_headers(Some(&raw), None, "text").unwrap();
        assert_eq!(
            headers.get("set-cookie").map(Vec::as_slice),
            Some(&["a=1".to_string(), "b=2".to_string()][..])
        );
        assert!(headers.get("content-length").is_none(), "逐跳头必须丢弃");
        assert_eq!(
            headers.get("content-type").map(Vec::as_slice),
            Some(&["text/plain; charset=utf-8".to_string()][..])
        );

        let bad = serde_json::json!({ "x-bad": "a\r\nb" });
        assert!(normalize_route_headers(Some(&bad), None, "text").is_err());
    }

    #[tokio::test]
    async fn unified_package_dynamic_routes_go_through_the_ts_engine() {        let workspace = temp_dir("routes_ws");
        let home = temp_dir("routes_home");
        let unified = temp_dir("routes_root");
        let package = unified.join("route-demo");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "id": "route-demo",
                "name": "Route Demo",
                "version": "0.1.0",
                "frontend": { "slots": [] },
                "backend": {}
            }"#,
        )
        .unwrap();
        fs::write(
            package.join("index.ts"),
            r#"
            const metadata = {
              name: 'route-demo',
              description: '动态路由测试',
              tools: { count: 0, names: [] },
              systemPrompt: { count: 0, names: [] },
              events: { count: 0, names: [] },
              routes: { count: 2, names: ['GET /hello/:name', 'POST /echo'] },
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.route({
                method: 'GET',
                path: '/hello/:name',
                async handler(req) {
                  return { hello: req.params.name, q: req.query.q, saw: req.headers['x-test'] || null };
                },
              });
              ada.route({
                method: 'POST',
                path: '/echo',
                async handler(req) {
                  return ada.response({
                    status: 202,
                    headers: { 'x-route': 'demo' },
                    body: JSON.stringify({ body: req.body, kind: req.bodyKind }),
                    contentType: 'application/json; charset=utf-8',
                  });
                },
              });
            }
            "#,
        )
        .unwrap();

        let manager = PluginManager::with_home(&home).with_unified_dir(&unified);
        let workspace_text = workspace.to_string_lossy().to_string();
        let hello = manager
            .invoke_route(
                "global:route-demo",
                "GET",
                "/hello/ada",
                serde_json::json!({ "q": "1" }),
                serde_json::json!({ "x-test": "yes" }),
                serde_json::Value::Null,
                "empty",
                Some("thread-1"),
                &workspace_text,
            )
            .await
            .expect("GET route should run");
        assert_eq!(hello.status, 200, "{hello:?}");
        let body: serde_json::Value = serde_json::from_str(&hello.body).unwrap();
        assert_eq!(body["hello"], "ada");
        assert_eq!(body["q"], "1");
        assert_eq!(body["saw"], "yes");
        assert_eq!(
            hello.headers.get("content-type").map(Vec::as_slice),
            Some(&["application/json; charset=utf-8".to_string()][..])
        );

        let echo = manager
            .invoke_route(
                "global:route-demo",
                "POST",
                "/echo",
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::json!({ "a": 1 }),
                "json",
                None,
                &workspace_text,
            )
            .await
            .expect("POST route should run");
        assert_eq!(echo.status, 202);
        assert_eq!(
            echo.headers.get("x-route").map(Vec::as_slice),
            Some(&["demo".to_string()][..])
        );
        let body: serde_json::Value = serde_json::from_str(&echo.body).unwrap();
        assert_eq!(body["body"]["a"], 1);
        assert_eq!(body["kind"], "json");

        let missing = manager
            .invoke_route(
                "global:route-demo",
                "GET",
                "/nope",
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::Value::Null,
                "empty",
                None,
                &workspace_text,
            )
            .await
            .expect_err("unknown route must be reported as not found");
        assert!(matches!(missing, PluginActionError::NotFound(_)));

        let invalid = manager
            .invoke_route(
                "global:route-demo",
                "TRACE",
                "/hello/ada",
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::Value::Null,
                "empty",
                None,
                &workspace_text,
            )
            .await
            .expect_err("unsupported method must be rejected");
        assert!(matches!(invalid, PluginActionError::Invalid(_)));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(unified);
    }

    /// `@ada-needs sqlite` 从源码声明一路走到引擎注入：动作里真的能建表、写入、读回。
    #[tokio::test]
    async fn sqlite_action_runs_only_when_declared_and_writes_inside_the_workspace() {
        let workspace = temp_dir("sqlite_ws");
        let home = temp_dir("sqlite_home");
        let unified = temp_dir("sqlite_root");
        let package = unified.join("sqlite-demo");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "id": "sqlite-demo",
                "name": "SQLite Demo",
                "version": "0.1.0",
                "frontend": { "slots": [] },
                "backend": {}
            }"#,
        )
        .unwrap();
        fs::write(
            package.join("index.ts"),
            r#"
            // @ada-needs sqlite
            const metadata = {
              name: 'sqlite-demo',
              description: 'SQLite 能力测试',
              tools: { count: 0, names: [] },
              systemPrompt: { count: 0, names: [] },
              events: { count: 0, names: [] },
              actions: { count: 1, names: ['bump'] },
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.registerAction({
                name: 'bump',
                description: '在 SQLite 里累加计数',
                mode: 'write',
                async execute() {
                  const { DatabaseSync } = require('node:sqlite');
                  const db = new DatabaseSync('.ada-sqlite-demo.db');
                  db.exec('CREATE TABLE IF NOT EXISTS counts(name TEXT PRIMARY KEY, n INTEGER NOT NULL)');
                  db.prepare('INSERT INTO counts(name, n) VALUES (?, 1) ON CONFLICT(name) DO UPDATE SET n = n + 1').run('demo');
                  const row = db.prepare('SELECT n FROM counts WHERE name = ?').get('demo');
                  db.close();
                  return { count: row.n };
                },
              });
            }
            "#,
        )
        .unwrap();

        let manager = PluginManager::with_home(&home).with_unified_dir(&unified);
        let workspace_text = workspace.to_string_lossy().to_string();
        let first = manager
            .invoke_action(
                "global:sqlite-demo",
                "bump",
                serde_json::json!({}),
                PluginActionIntent::Write,
                &workspace_text,
            )
            .await
            .expect("SQLite 动作应成功");
        assert_eq!(first["data"]["count"], 1, "{first}");
        let second = manager
            .invoke_action(
                "global:sqlite-demo",
                "bump",
                serde_json::json!({}),
                PluginActionIntent::Write,
                &workspace_text,
            )
            .await
            .expect("SQLite 动作第二次应成功");
        assert_eq!(second["data"]["count"], 2, "{second}");
        assert!(
            workspace.join(".ada-sqlite-demo.db").is_file(),
            "数据库必须落在工作区内"
        );

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(unified);
    }

    /// `@ada-needs exec` 从源码声明走到引擎注入：动作里真的能起子进程并读回 stdout。
    #[tokio::test]
    async fn exec_action_runs_only_when_declared() {
        let workspace = temp_dir("exec_ws");
        let home = temp_dir("exec_home");
        let unified = temp_dir("exec_root");
        let package = unified.join("exec-demo");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("plugin.json"),
            r#"{
                "id": "exec-demo",
                "name": "Exec Demo",
                "version": "0.1.0",
                "frontend": { "slots": [] },
                "backend": {}
            }"#,
        )
        .unwrap();
        fs::write(
            package.join("index.ts"),
            r#"
            // @ada-needs exec
            const metadata = {
              name: 'exec-demo',
              description: 'child_process 能力测试',
              tools: { count: 0, names: [] },
              systemPrompt: { count: 0, names: [] },
              events: { count: 0, names: [] },
              actions: { count: 1, names: ['echo'] },
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.registerAction({
                name: 'echo',
                description: '执行 echo 并返回 stdout',
                mode: 'write',
                async execute() {
                  const { execFileSync } = require('node:child_process');
                  const isWin = process.platform === 'win32';
                  const output = isWin
                    ? execFileSync('cmd.exe', ['/d', '/s', '/c', 'echo exec-ok'], { encoding: 'utf8' })
                    : execFileSync('/bin/echo', ['exec-ok'], { encoding: 'utf8' });
                  return { output: output };
                },
              });
            }
            "#,
        )
        .unwrap();

        let manager = PluginManager::with_home(&home).with_unified_dir(&unified);
        let workspace_text = workspace.to_string_lossy().to_string();
        let result = manager
            .invoke_action(
                "global:exec-demo",
                "echo",
                serde_json::json!({}),
                PluginActionIntent::Write,
                &workspace_text,
            )
            .await
            .expect("exec 动作应成功");
        assert!(
            result["data"]["output"]
                .as_str()
                .unwrap_or_default()
                .contains("exec-ok"),
            "{result}"
        );

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(unified);
    }
}
