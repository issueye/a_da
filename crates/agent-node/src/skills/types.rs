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
    /// frontmatter 里**认不出的键**（P2 · SP-45）。
    ///
    /// 非空 = 这个技能文件写错了键名（`whenToUse` 拼错一个字母就会走到这里）。
    /// 以前是静默忽略，于是"技能写好了却不生效"没有任何线索；现在由 `skill.list`
    /// 产出 `skill.invalid` 诊断报给界面。空数组不序列化（老客户端不必知道这个新键）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_frontmatter_keys: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSkillFile {
    pub file_path: std::path::PathBuf,
    pub base_dir: std::path::PathBuf,
    pub is_file_skill: bool,
}
