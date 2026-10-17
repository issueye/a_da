use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    Builtin,
    Workspace,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

/// 插件声明的一段**提示词片段**（P2 · SP-44）。
///
/// 这是插件"给模型加话"的唯一入口：它进 `PromptPlan` 时带 `plugin:<id>` 来源，
/// 于是埋点能回答"这段提示词是谁加的"——而不是只看到一个总数莫名膨胀。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPromptDeclaration {
    /// 片段标识（同一插件内唯一即可）
    pub id: String,
    pub content: String,
}

/// 插件声明的一个**技能**（P2 · SP-45）。
///
/// 插件有两种写法，两条路都通到这里：功能式 `api.registerSkill({...})`，
/// 以及对象式 `export default { skills: [...] }`。收下之后它就是技能三来源之一
/// （目录 / 插件 / 内置，见 `SkillManager::scan_skills_with`），
/// 并带上 `plugin:<id>` 来源——于是"这条技能是谁带来的"在提示词字节报表里看得见。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSkillDeclaration {
    /// 技能名（同一个插件内唯一；合并时同名先到先得，见 `SkillManager`）
    pub name: String,
    /// Optional source file path, resolved by the host inside the plugin/workspace roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub description: String,
    /// 正文。**空正文的技能不装**：它进提示词只会白占字节，而"技能"的意义就是那段规范。
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,
}

impl PluginSkillDeclaration {
    /// 转成扫描结果里的一条技能（`plugin_id` / `plugin_name` 由此**第一次**真正被填上）。
    ///
    /// `scope = "plugin"`：界面据此把插件技能与用户自己的文件技能分开显示
    /// （`skill.list` 的两个既有分支正是按 `scope == "builtin"` 分流的）。
    pub fn to_summary(&self, plugin_id: &str, plugin_name: &str) -> crate::skills::SkillSummary {
        crate::skills::SkillSummary {
            id: format!("plugin:{plugin_id}:{}", self.name),
            name: self.name.clone(),
            description: self.description.clone(),
            body: self.body.clone(),
            // 插件技能没有独立的文件路径：正文来自插件模块的声明，不是磁盘上的 .md
            path: self.path.clone().unwrap_or_default(),
            base_directory: self
                .path
                .as_deref()
                .and_then(|path| std::path::Path::new(path).parent())
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some(plugin_name.to_string()),
            plugin_id: Some(plugin_id.to_string()),
            is_file_skill: Some(false),
            user_invocable: self.user_invocable.or(Some(true)),
            when_to_use: self.when_to_use.clone(),
            unknown_frontmatter_keys: Vec::new(),
        }
    }
}

/// 插件声明的一条**钩子注册**（P3a · SP-50）。
///
/// 声明面是 `api.registerHook({ point, actions, order, handler })`（对象形态
/// `export default { hooks: [...] }` 也行）。**声明即授权**：`actions` 里没有的动作，
/// 运行期即使插件返回了也不生效（并记理由）——否则"我声明了 observe、实际却拦下了"就没法解释。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHookDeclaration {
    pub point: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolInfo {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    pub is_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnostic {
    pub plugin_id: String,
    pub level: String, // "info" | "warn" | "error"
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPluginContributions {
    #[serde(default)]
    pub tools: Vec<PluginToolDeclaration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    #[serde(default)]
    pub contributions: LoadedPluginContributions,
    pub declarative: bool,
    pub status: String, // "ready" | "not-ready" | "broken" | "conflict"
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    pub plugin: LoadedPlugin,
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub file_path: String,
    pub scope: String, // "builtin" | "workspace" | "global"
    pub enabled: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub tools: Vec<PluginToolInfo>,
    pub skills: Vec<crate::skills::SkillSummary>,
    pub prompts: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_package: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub size_bytes: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilities {
    pub allow_system_prompt_replace: bool,
    pub allow_text_rewrite: bool,
    pub allow_thread_delete_block: bool,
    pub allow_compaction_replace: bool,
    pub allow_plan_mode_hooks: bool,
    pub allow_third_party_hooks: bool,
    pub allow_builtin_shadow: bool,
    pub hook_timeout_ms: u64,
}

impl Default for PluginCapabilities {
    fn default() -> Self {
        Self {
            allow_system_prompt_replace: true,
            allow_text_rewrite: true,
            allow_thread_delete_block: true,
            allow_compaction_replace: true,
            allow_plan_mode_hooks: true,
            allow_third_party_hooks: true,
            allow_builtin_shadow: true,
            hook_timeout_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPluginCapabilitiesDto {
    pub capabilities: PluginCapabilities,
    pub invalid: Vec<String>,
    pub overrides: HashMap<String, serde_json::Value>,
}
