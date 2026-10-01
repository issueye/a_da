use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::builtins::builtin_subagents;
use super::types::{SubagentMode, SubagentProfile, SubagentScope};
use crate::session::get_app_home;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentsState {
    #[serde(default)]
    builtin_enabled: HashMap<String, bool>,
}

#[derive(Debug)]
pub struct SubagentManager {
    cached_profiles: RwLock<HashMap<String, Vec<SubagentProfile>>>,
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            cached_profiles: RwLock::new(HashMap::new()),
        }
    }

    fn state_file_path() -> PathBuf {
        get_app_home().join("subagents_state.json")
    }

    fn load_state(&self) -> SubagentsState {
        let path = Self::state_file_path();
        if !path.exists() {
            return SubagentsState::default();
        }
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => SubagentsState::default(),
        }
    }

    fn save_state(&self, state: &SubagentsState) -> Result<()> {
        let path = Self::state_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(state)?;
        fs::write(path, data)?;
        Ok(())
    }

    pub fn global_dir() -> PathBuf {
        get_app_home().join("subagents")
    }

    pub fn workspace_dir(workspace: &Path) -> PathBuf {
        workspace.join(".ada").join("subagents")
    }

    fn parse_json_subagent(
        &self,
        raw: &str,
        default_id: &str,
        scope: SubagentScope,
    ) -> Option<SubagentProfile> {
        let val: serde_json::Value = serde_json::from_str(raw).ok()?;
        let clean_id = val
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(default_id)
            .trim_start_matches("workspace_")
            .trim_start_matches("global_");

        let id = format!("{}_{}", format!("{:?}", scope).to_lowercase(), clean_id);
        let name = val
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(default_id)
            .to_string();
        let description = val
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let system_prompt = val
            .get("systemPrompt")
            .or_else(|| val.get("system_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let allowed_tools = val
            .get("allowedTools")
            .or_else(|| val.get("allowed_tools"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(|str_v| str_v.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "list_files".to_string(),
                    "read_file".to_string(),
                    "search_files".to_string(),
                ]
            });

        let disallowed_tools = val
            .get("disallowedTools")
            .or_else(|| val.get("disallowed_tools"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(|str_v| str_v.to_string()))
                    .collect()
            });

        let mode = match val.get("mode").and_then(|v| v.as_str()) {
            Some("readwrite") => SubagentMode::Readwrite,
            _ => SubagentMode::Readonly,
        };

        let color = val
            .get("color")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let background = val.get("background").and_then(|v| v.as_bool());
        let max_steps = val
            .get("maxSteps")
            .or_else(|| val.get("max_steps"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let enabled = val
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let icon = val
            .get("icon")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                Some(
                    if mode == SubagentMode::Readwrite {
                        "bug"
                    } else {
                        "search"
                    }
                    .to_string(),
                )
            });

        let updated_at = val
            .get("updatedAt")
            .or_else(|| val.get("updated_at"))
            .and_then(|v| v.as_u64())
            .or_else(|| {
                Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                )
            });

        Some(SubagentProfile {
            id,
            name,
            description,
            system_prompt,
            allowed_tools,
            disallowed_tools,
            mode,
            color,
            background,
            max_steps,
            model_override: None,
            gate: None,
            enabled,
            scope,
            icon,
            updated_at,
        })
    }

    fn parse_markdown_subagent(
        &self,
        raw: &str,
        default_id: &str,
        scope: SubagentScope,
    ) -> SubagentProfile {
        let mut name = default_id.to_string();
        let mut description = String::new();
        let mut mode = SubagentMode::Readonly;
        let mut allowed_tools = vec![
            "list_files".to_string(),
            "read_file".to_string(),
            "search_files".to_string(),
        ];
        let mut disallowed_tools: Option<Vec<String>> = None;
        let mut color = None;
        let mut background = None;
        let mut enabled = true;
        let mut max_steps = None;
        let mut icon = Some("search".to_string());
        let mut system_prompt = raw.trim().to_string();

        if let Some(stripped) = raw.strip_prefix("---") {
            if let Some(end_idx) = stripped.find("\n---") {
                let frontmatter = &stripped[..end_idx];
                let body = stripped[end_idx + 4..].trim();
                system_prompt = body.to_string();

                for line in frontmatter.lines() {
                    let trimmed = line.trim();
                    if let Some(idx) = trimmed.find(':') {
                        let key = trimmed[..idx].trim();
                        let val = trimmed[idx + 1..]
                            .trim()
                            .trim_matches('\'')
                            .trim_matches('"');
                        match key {
                            "name" => name = val.to_string(),
                            "description" => description = val.to_string(),
                            "mode" => {
                                mode = if val == "readwrite" {
                                    SubagentMode::Readwrite
                                } else {
                                    SubagentMode::Readonly
                                }
                            }
                            "enabled" => enabled = val != "false",
                            "maxSteps" | "max_steps" => max_steps = val.parse::<usize>().ok(),
                            "icon" => icon = Some(val.to_string()),
                            "color" => color = Some(val.to_string()),
                            "background" => background = Some(val == "true"),
                            "tools" | "allowedTools" | "allowed_tools" => {
                                allowed_tools = val
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                            }
                            "disallowedTools" | "disallowed_tools" => {
                                disallowed_tools = Some(
                                    val.split(',')
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty())
                                        .collect(),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let clean_id = default_id
            .trim_start_matches("workspace_")
            .trim_start_matches("global_");
        let id = format!("{}_{}", format!("{:?}", scope).to_lowercase(), clean_id);

        SubagentProfile {
            id,
            name,
            description,
            system_prompt,
            allowed_tools,
            disallowed_tools,
            mode,
            color,
            background,
            max_steps,
            model_override: None,
            gate: None,
            enabled,
            scope,
            icon,
            updated_at: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
        }
    }

    fn scan_directory(&self, dir: &Path, scope: SubagentScope) -> Vec<SubagentProfile> {
        let mut results = Vec::new();
        if !dir.exists() || !dir.is_dir() {
            return results;
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if stem.is_empty() {
                    continue;
                }

                if let Ok(content) = fs::read_to_string(&path) {
                    if ext == "json" {
                        if let Some(profile) = self.parse_json_subagent(&content, &stem, scope) {
                            results.push(profile);
                        }
                    } else if ext == "md" {
                        results.push(self.parse_markdown_subagent(&content, &stem, scope));
                    }
                }
            }
        }

        results
    }

    pub fn list_profiles(&self, workspace: Option<&Path>) -> Vec<SubagentProfile> {
        let state = self.load_state();
        let mut list = Vec::new();

        // 1. 内置子智能体并叠加启停状态
        for mut builtin in builtin_subagents() {
            if let Some(&enabled) = state.builtin_enabled.get(&builtin.id) {
                builtin.enabled = enabled;
            }
            list.push(builtin);
        }

        // 2. 全局自定义子智能体 (~/.a-da/subagents/)
        let global_dir = Self::global_dir();
        list.extend(self.scan_directory(&global_dir, SubagentScope::Global));

        // 3. 工作区自定义子智能体 (<workspace>/.ada/subagents/)
        if let Some(ws) = workspace {
            let ws_dir = Self::workspace_dir(ws);
            list.extend(self.scan_directory(&ws_dir, SubagentScope::Workspace));
        }

        // 更新缓存
        let key = workspace
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "__global__".to_string());
        if let Ok(mut cache) = self.cached_profiles.write() {
            cache.insert(key, list.clone());
        }

        list
    }

    pub fn get_profile(&self, id: &str, workspace: Option<&Path>) -> Option<SubagentProfile> {
        let list = self.list_profiles(workspace);
        list.into_iter().find(|p| p.id == id)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut state = self.load_state();
        state.builtin_enabled.insert(id.to_string(), enabled);
        self.save_state(&state)
    }

    pub fn delete_profile(&self, id: &str, workspace: Option<&Path>) -> Result<bool> {
        // 内置智能体不可物理删除，只能禁用
        if matches!(id, "general_purpose" | "researcher" | "code_reviewer" | "tester") {
            self.set_enabled(id, false)?;
            return Ok(true);
        }

        // 尝试在全局目录找
        let global_dir = Self::global_dir();
        for ext in &["json", "md"] {
            let p = global_dir.join(format!("{}.{}", id, ext));
            if p.exists() {
                let _ = fs::remove_file(p);
                return Ok(true);
            }
        }

        // 尝试在工作区目录找
        if let Some(ws) = workspace {
            let ws_dir = Self::workspace_dir(ws);
            for ext in &["json", "md"] {
                let p = ws_dir.join(format!("{}.{}", id, ext));
                if p.exists() {
                    let _ = fs::remove_file(p);
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}
