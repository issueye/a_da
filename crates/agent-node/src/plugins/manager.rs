use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use super::builtins::get_builtin_plugin_items;
use super::types::{
    LoadedPlugin, LoadedPluginContributions, PluginCapabilities, PluginDiagnostic, PluginItem,
    PluginManifest, PluginScope, PluginToolDeclaration, PluginToolInfo,
    ResolvedPluginCapabilitiesDto,
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
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        Self { home: None }
    }

    /// 注入应用目录（测试 / 嵌入式宿主用；生产路径走 [`PluginManager::new`]）。
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }

    /// `config.json` 落点
    fn config_path(&self) -> PathBuf {
        match &self.home {
            Some(h) => h.join("config.json"),
            None => get_config_path(),
        }
    }

    /// 应用目录根
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

    /// Load the current plugin view from the canonical agent directory and
    /// compatible legacy directories. The manager is intentionally stateless;
    /// each call is a fresh load.
    pub fn load_plugins(&self, workspace: Option<&str>) -> Vec<PluginItem> {
        self.scan_plugins(workspace)
    }

    /// Explicit reload entry point. Kept separate from `load_plugins` so RPC
    /// callers can express intent and future caching can be added in one place.
    pub fn reload_plugins(&self, workspace: Option<&str>) -> Vec<PluginItem> {
        self.scan_plugins(workspace)
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
                        if let Some(v) = pc.get("allowPlanModeHooks").and_then(|b| b.as_bool()) {
                            caps.allow_plan_mode_hooks = v;
                        }
                        if let Some(v) = pc.get("allowThirdPartyHooks").and_then(|b| b.as_bool()) {
                            caps.allow_third_party_hooks = v;
                        }
                        if let Some(v) = pc.get("allowBuiltinShadow").and_then(|b| b.as_bool()) {
                            caps.allow_builtin_shadow = v;
                        }
                        if let Some(v) = pc.get("hookTimeoutMs").and_then(|n| n.as_u64()) {
                            caps.hook_timeout_ms = v;
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

        // 2. 扫描待探测的目录集合：(目录路径, 作用域)
        let mut scan_dirs: Vec<(PathBuf, &'static str)> = Vec::new();

        // 工作区扩展目录：<workspace>/.ada/extensions 与 <workspace>/.ada/plugins
        if let Some(ws) = workspace {
            let ws_path = PathBuf::from(ws);
            if !ws.trim().is_empty() {
                scan_dirs.push((Self::agent_extensions_dir(ws), "workspace"));
                scan_dirs.push((ws_path.join(".ada").join("extensions"), "workspace"));
                scan_dirs.push((ws_path.join(".ada").join("plugins"), "workspace"));
            }
        }

        // 全局扩展目录：~/.a-da/extensions 与 ~/.a-da/plugins
        let app_home = self.app_home();
        scan_dirs.push((app_home.join("agent-coding").join("extensions"), "global"));
        scan_dirs.push((app_home.join("extensions"), "global"));
        scan_dirs.push((app_home.join("plugins"), "global"));

        for (dir_path, scope) in scan_dirs {
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

                let id = format!("{}:{}", scope, file_name.trim_end_matches(".ts").trim_end_matches(".js"));
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

                let is_package = path.is_dir();
                let plugin_name = if is_package {
                    file_name.to_string()
                } else {
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name).to_string()
                };

                let enabled = !disabled.contains(&id);

                // 智能抽取工具声明（若是代码文件，扫描导出工具名称）
                let mut tools = Vec::new();
                let mut tools_decl = Vec::new();
                let script_path = if is_package {
                    let candidates = [
                        path.join("index.ts"),
                        path.join("index.js"),
                        path.join("tools.ts"),
                        path.join(format!("{}.ts", plugin_name)),
                    ];
                    candidates.into_iter().find(|p| p.exists())
                } else if file_name.ends_with(".ts") || file_name.ends_with(".js") {
                    Some(path.clone())
                } else {
                    None
                };

                let mut desc = format!("{} 扩展插件", if scope == "global" { "全局" } else { "工作区" });

                if let Some(ref sp) = script_path {
                    if let Ok(content) = fs::read_to_string(sp) {
                        // 快速尝试从注释或 export 中提取说明
                        for line in content.lines().take(20) {
                            let trimmed = line.trim();
                            if trimmed.starts_with("*") || trimmed.starts_with("//") {
                                let c = trimmed.trim_start_matches(|c| c == '*' || c == '/' || c == ' ');
                                if !c.is_empty() && !c.starts_with('@') && desc.ends_with("扩展插件") {
                                    desc = c.to_string();
                                }
                            }
                        }

                        // 探查导出的工具名：优先从源码中匹配 name: 'xxx'，回退到 plugin_name.replace('-', "_")
                        let tool_name = if let Some(cap) = content.find("name: '").or_else(|| content.find("name: \"")) {
                            let after = &content[cap + 7..];
                            if let Some(end) = after.find('\'').or_else(|| after.find('"')) {
                                after[..end].to_string()
                            } else {
                                plugin_name.replace('-', "_")
                            }
                        } else {
                            plugin_name.replace('-', "_")
                        };

                        let params = if tool_name == "web_search" {
                            serde_json::json!({
                                "type": "object",
                                "properties": {
                                    "query": { "type": "string", "description": "需要联网检索的关键词或查询语句" }
                                },
                                "required": ["query"]
                            })
                        } else {
                            serde_json::json!({ "type": "object", "properties": {} })
                        };

                        // `is_write` 一律按 `ToolDescriptor` 真源判定，**不信第三方插件自述**
                        // （W2-T7）：原先这里硬编码 `false`，于是第三方插件的**写工具在界面上
                        // 被当成只读**，在只读档位/plan 模式/只读子智能体里也可能被放行。
                        // 未知工具**失败安全地当作写操作**（AGENTS.md §2）。
                        let is_write = super::plugin_tool_is_write(&tool_name);

                        tools.push(PluginToolInfo {
                            name: tool_name.clone(),
                            description: desc.clone(),
                            parameters: Some(params.clone()),
                            is_write,
                        });
                        tools_decl.push(PluginToolDeclaration {
                            name: tool_name,
                            label: Some(plugin_name.clone()),
                            description: desc.clone(),
                            parameters: params,
                        });
                    }
                }

                // 埋点：把"这个插件为什么不会生效"如实写进诊断字段。
                //
                // 以前这里恒为 `Vec::new()`，于是插件卡永远显示不出任何问题。P0 当时还补过一条
                // 「脚本执行面尚未接线，工具不会被模型调用」——**P1 已经把它接上了**
                // （`ScriptHost::load` 会真的执行一遍插件，贡献进装配），
                // 那条诊断等于对着已经生效的插件喊"不生效"，已删除。
                let mut diagnostics: Vec<PluginDiagnostic> = Vec::new();
                if !enabled {
                    diagnostics.push(PluginDiagnostic {
                        plugin_id: id.clone(),
                        level: "warn".to_string(),
                        message: "插件已停用：它的贡献不会参与装配".to_string(),
                        hint: None,
                    });
                }
                if !tools.is_empty() {
                    // 本页（`plugin.list`）是**管理面的清单**，为了不执行代码就列出插件，
                    // 工具名是从源码文本里猜的；真正生效的贡献由 `ScriptHost::load`
                    // 执行插件得到，两者可能不同（名字在运行时拼出来、`api.registerTool`
                    // 写在工厂函数里……）。把口径说清，别让"列出来了"被当成"已经在服务模型"。
                    diagnostics.push(PluginDiagnostic {
                        plugin_id: id.clone(),
                        level: "info".to_string(),
                        message: format!(
                            "本页工具清单由源码文本推断（{} 条）；实际生效的贡献以脚本执行结果为准，两者可能不同",
                            tools.len()
                        ),
                        hint: None,
                    });
                }

                let manifest = PluginManifest {
                    id: id.clone(),
                    name: plugin_name.clone(),
                    description: desc.clone(),
                    version: Some("1.0.0".to_string()),
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
                    status: "ready".to_string(),
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
                    status: "ready".to_string(),
                    version: Some("1.0.0".to_string()),
                    diagnostics,
                    tools,
                    skills: Vec::new(),
                    prompts: Vec::new(),
                    is_package: Some(is_package),
                    error: None,
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
        let dir = if scope == "workspace" && workspace.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            Self::agent_extensions_dir(workspace.unwrap())
        } else {
            self.app_home().join("agent-coding").join("extensions")
        };

        fs::create_dir_all(&dir)?;
        let target = dir.join(format!("{}.ts", name));

        let default_code = format!(
            r#"/**
 * 自定义插件：{}
 */
export default {{
  name: '{}',
  description: '自定义扩展插件',
  tools: [
    {{
      name: '{}_tool',
      description: '自定义工具示例',
      parameters: {{
        type: 'object',
        properties: {{
          query: {{ type: 'string', description: '查询内容' }}
        }},
        required: ['query']
      }},
      async execute(args) {{
        return {{ output: `已执行: ${{args.query}}`, ok: true }};
      }}
    }}
  ]
}};
"#,
            name, name, name
        );

        fs::write(&target, code.unwrap_or(&default_code))?;
        Ok(target.to_string_lossy().to_string())
    }

    /// 删除插件文件
    pub fn delete_plugin(&self, file_path: &str, _workspace: Option<&str>) -> Result<bool> {
        let p = PathBuf::from(file_path);
        if p.exists() {
            if p.is_file() {
                fs::remove_file(&p)?;
            } else if p.is_dir() {
                fs::remove_dir_all(&p)?;
            }
            return Ok(true);
        }
        Ok(false)
    }
}
