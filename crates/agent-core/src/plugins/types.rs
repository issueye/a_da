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
