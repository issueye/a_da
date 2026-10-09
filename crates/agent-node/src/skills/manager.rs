use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::builtins::get_builtin_skills;
use super::types::{DiscoveredSkillFile, SkillSummary};
use crate::session::get_app_home;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsState {
    #[serde(rename = "enabledState", default)]
    pub enabled_state: HashMap<String, bool>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillManager;

impl SkillManager {
    pub fn new() -> Self {
        Self
    }

    /// 技能状态文件路径：~/.a-da/skills_state.json
    pub fn state_file_path(&self) -> PathBuf {
        get_app_home().join("skills_state.json")
    }

    /// 读取持久化启停状态
    pub fn load_state(&self) -> SkillsState {
        let path = self.state_file_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str::<SkillsState>(&content) {
                    return state;
                }
            }
        }
        SkillsState::default()
    }

    /// 保存持久化启停状态
    pub fn save_state(&self, state: &SkillsState) -> Result<()> {
        let path = self.state_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(state)?;
        let mut file = File::create(&path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    /// 递归发现目录下的所有技能文件
    fn find_skill_files(&self, dir: &Path, depth: usize) -> Vec<DiscoveredSkillFile> {
        if depth > 4 || !dir.exists() {
            return Vec::new();
        }

        let mut results = Vec::new();
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut sub_dirs = Vec::new();
        let mut direct_skill_md: Option<PathBuf> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.starts_with('.') || file_name == "node_modules" || file_name == "target" {
                continue;
            }

            if path.is_file() {
                if file_name.eq_ignore_ascii_case("SKILL.md") {
                    direct_skill_md = Some(path);
                    break;
                } else if file_name.ends_with(".md") {
                    results.push(DiscoveredSkillFile {
                        file_path: path,
                        base_dir: dir.to_path_buf(),
                        is_file_skill: true,
                    });
                }
            } else if path.is_dir() {
                sub_dirs.push(path);
            }
        }

        // 如果当前目录直接包含 SKILL.md，则视为目录技能根，不向下深搜
        if let Some(skill_md) = direct_skill_md {
            return vec![DiscoveredSkillFile {
                file_path: skill_md,
                base_dir: dir.to_path_buf(),
                is_file_skill: false,
            }];
        }

        // 否则递归子目录
        for sub in sub_dirs {
            results.extend(self.find_skill_files(&sub, depth + 1));
        }

        results
    }

    /// 解析技能 Markdown 正文与 Frontmatter 元数据
    fn parse_skill_markdown(&self, raw: &str, file_path: &Path) -> (String, String, Option<String>, Option<bool>, String) {
        let mut name = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill").to_string();
        if name.eq_ignore_ascii_case("SKILL") {
            if let Some(parent) = file_path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()) {
                name = parent.to_string();
            }
        }
        let mut description = String::new();
        let mut when_to_use: Option<String> = None;
        let mut user_invocable: Option<bool> = Some(true);
        let mut body = raw.trim().to_string();

        if raw.starts_with("---") {
            if let Some(end_idx) = raw[3..].find("---") {
                let yaml_part = &raw[3..3 + end_idx];
                body = raw[3 + end_idx + 3..].trim().to_string();

                for line in yaml_part.lines() {
                    if let Some(colon_idx) = line.find(':') {
                        let key = line[..colon_idx].trim();
                        let val = line[colon_idx + 1..].trim().trim_matches(|c| c == '\'' || c == '"');
                        match key {
                            "name" => {
                                if !val.is_empty() {
                                    name = val.to_string();
                                }
                            }
                            "description" => {
                                description = val.to_string();
                            }
                            "whenToUse" | "when_to_use" => {
                                when_to_use = Some(val.to_string());
                            }
                            "userInvocable" | "user_invocable" => {
                                user_invocable = Some(val.eq_ignore_ascii_case("true"));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        (name, description, when_to_use, user_invocable, body)
    }

    /// 扫描并汇集工作区、全局与系统内置的所有技能
    pub fn scan_skills(&self, workspace: Option<&str>) -> Vec<SkillSummary> {
        let state = self.load_state();
        let mut skills = Vec::new();
        let mut seen_names = HashSet::new();

        let mut roots: Vec<(PathBuf, &'static str)> = Vec::new();

        // 1. 工作区专属技能目录：<workspace>/.ada/skills 与 <workspace>/.agents/skills
        if let Some(ws) = workspace {
            let ws_path = PathBuf::from(ws);
            if !ws.trim().is_empty() {
                roots.push((ws_path.join(".ada").join("skills"), "workspace"));
                roots.push((ws_path.join(".agents").join("skills"), "workspace"));
            }
        }

        // 2. 用户全局技能目录：~/.a-da/skills 与 ~/.agents/skills
        let app_home = get_app_home();
        roots.push((app_home.join("skills"), "global"));
        if let Some(parent) = app_home.parent() {
            roots.push((parent.join(".agents").join("skills"), "global"));
        }

        // 3. 扫描文件系统技能
        for (root_dir, scope) in roots {
            if !root_dir.exists() {
                continue;
            }
            let files = self.find_skill_files(&root_dir, 0);
            for item in files {
                if let Ok(raw) = fs::read_to_string(&item.file_path) {
                    let (name, desc, when, invocable, body) = self.parse_skill_markdown(&raw, &item.file_path);
                    let dedupe_key = name.to_lowercase();
                    if seen_names.contains(&dedupe_key) {
                        continue;
                    }
                    seen_names.insert(dedupe_key);

                    let id = format!("{}:{}", scope, name);
                    let enabled = state.enabled_state.get(&id).copied().unwrap_or(true);

                    skills.push(SkillSummary {
                        id,
                        name,
                        description: desc,
                        body,
                        path: item.file_path.to_string_lossy().to_string(),
                        base_directory: item.base_dir.to_string_lossy().to_string(),
                        scope: scope.to_string(),
                        enabled,
                        plugin_name: None,
                        plugin_id: None,
                        is_file_skill: Some(item.is_file_skill),
                        user_invocable: invocable,
                        when_to_use: when,
                    });
                }
            }
        }

        // 4. 注入系统内置 5 大预设技能（若未被工作区或全局同名技能覆盖）
        let builtin_skills = get_builtin_skills();
        for mut builtin in builtin_skills {
            let dedupe_key = builtin.name.to_lowercase();
            if seen_names.contains(&dedupe_key) {
                continue;
            }
            seen_names.insert(dedupe_key);

            let enabled = state.enabled_state.get(&builtin.id).copied().unwrap_or(true);
            builtin.enabled = enabled;
            skills.push(builtin);
        }

        skills
    }

    /// 切换技能启停状态
    pub fn toggle_skill(&self, id: &str, enabled: bool) -> Result<()> {
        let mut state = self.load_state();
        state.enabled_state.insert(id.to_string(), enabled);
        self.save_state(&state)
    }

    /// 创建自定义技能模板
    pub fn create_skill_template(
        &self,
        workspace: Option<&str>,
        scope: &str,
        name: &str,
        description: &str,
        body: Option<&str>,
    ) -> Result<String> {
        let dir = if scope == "workspace" && workspace.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            PathBuf::from(workspace.unwrap()).join(".ada").join("skills").join(name)
        } else {
            get_app_home().join("skills").join(name)
        };

        fs::create_dir_all(&dir)?;
        let target = dir.join("SKILL.md");

        let default_body = format!(
            "---\nname: {}\ndescription: {}\nwhenToUse: 当...\nuserInvocable: true\n---\n\n# {}\n\n{}",
            name,
            description,
            name,
            body.unwrap_or("在此处编写技能规范正文...")
        );

        fs::write(&target, default_body)?;
        Ok(target.to_string_lossy().to_string())
    }

    /// 删除技能
    pub fn delete_skill(&self, id: &str, workspace: Option<&str>) -> Result<bool> {
        let skills = self.scan_skills(workspace);
        if let Some(target) = skills.iter().find(|s| s.id == id) {
            if target.scope != "builtin" && !target.path.is_empty() {
                let p = PathBuf::from(&target.path);
                if p.exists() {
                    let _ = fs::remove_file(&p);
                    if let Some(parent) = p.parent() {
                        let _ = fs::remove_dir(parent); // 如果是空目录顺便清除
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
