use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
    pub base_directory: String,
    pub scope: String, // "builtin" | "workspace" | "global" | "plugin"
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_file_skill: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSkillFile {
    pub file_path: std::path::PathBuf,
    pub base_dir: std::path::PathBuf,
    pub is_file_skill: bool,
}
