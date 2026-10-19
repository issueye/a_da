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

/// 技能管理器。
///
/// 🔴 带 `home` 的理由同 [`crate::plugins::PluginManager`]：早先纯 unit struct 直接读
/// `get_app_home()`，跑单测就会改写用户真实的 `~/.a-da/skills_state.json`。
#[derive(Debug, Clone, Default)]
pub struct SkillManager {
    /// `None` = 进程默认应用目录（生产）；`Some` = 注入目录（测试 / 嵌入式宿主）。
    home: Option<PathBuf>,
}

impl SkillManager {
    pub fn new() -> Self {
        Self { home: None }
    }

    /// 注入应用目录（测试 / 嵌入式宿主用；生产路径走 [`SkillManager::new`]）。
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }

    fn app_home(&self) -> PathBuf {
        match &self.home {
            Some(h) => h.clone(),
            None => get_app_home(),
        }
    }

    /// 技能状态文件路径：~/.a-da/skills_state.json
    pub fn state_file_path(&self) -> PathBuf {
        self.app_home().join("skills_state.json")
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
    /// 解析技能 Markdown 的 frontmatter 与正文。
    ///
    /// 返回的第 6 项是**认不出的 frontmatter 键**（P2·SP-45）：以前是静默忽略，
    /// 于是作者写 `when_to_use:` 拼错一个字母、技能就"看起来装上了却不生效"。
    fn parse_skill_markdown(
        &self,
        raw: &str,
        file_path: &Path,
    ) -> (String, String, Option<String>, Option<bool>, String, Vec<String>) {
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
        let mut unknown_keys: Vec<String> = Vec::new();

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
                            other => {
                                let other = other.trim();
                                if !other.is_empty() {
                                    unknown_keys.push(other.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        (
            name,
            description,
            when_to_use,
            user_invocable,
            body,
            unknown_keys,
        )
    }

    /// 旧技能目录**不再扫描**，但存在时要提醒迁移（每个进程只提示一次）。
    ///
    /// 为什么不是静默丢弃：用户看到"技能明明在 `.ada/skills/` 里"，界面却一条都没有，
    /// 只会怀疑功能坏了。一条带**目标路径**的告警能让他 10 秒内完成迁移。
    fn warn_about_legacy_skill_dirs(&self, workspace: Option<&str>) {
        use std::sync::OnceLock;
        static WARNED: OnceLock<()> = OnceLock::new();
        if WARNED.get().is_some() {
            return;
        }

        let mut legacy: Vec<PathBuf> = Vec::new();
        if let Some(ws) = workspace.filter(|s| !s.trim().is_empty()) {
            legacy.push(PathBuf::from(ws).join(".ada").join("skills"));
            legacy.push(PathBuf::from(ws).join(".agents").join("skills"));
        }
        let home = self.app_home();
        legacy.push(home.join("skills"));
        if let Some(parent) = home.parent() {
            legacy.push(parent.join(".agents").join("skills"));
        }

        let hits: Vec<String> = legacy
            .iter()
            .filter(|p| p.is_dir() && dir_contains_markdown(p))
            .map(|p| p.display().to_string())
            .collect();
        if hits.is_empty() {
            return;
        }
        WARNED.set(()).ok();
        tracing::warn!(
            "[skills] 这些旧技能目录已不再扫描：{}。请把技能（含 SKILL.md 的目录）移到 \
             `<ws>/.ada/agent-coding/skills/` 或 `~/.a-da/agent-coding/skills/`",
            hits.join("、")
        );
    }

    /// 扫描并汇集工作区、全局与系统内置的所有技能
    pub fn scan_skills(&self, workspace: Option<&str>) -> Vec<SkillSummary> {
        self.scan_skills_with(workspace, &[])
    }

    /// 同 [`Self::scan_skills`]，但额外带上**插件声明的技能**（P2 · SP-45）。
    ///
    /// # 为什么插件技能是"注入"而不是自己去读
    ///
    /// 技能模块不认识插件（那要执行 JS 沙箱，属于插件装载层）。所以插件技能由**组合根**
    /// 在装配期收好后在这里合并——技能三来源就此齐了：**目录** / **插件** / **内置**。
    ///
    /// # 同名谁胜出（口径写在这里，别处不再判一遍）
    ///
    /// 顺序是 `目录 → 插件 → 内置`，先到先得（沿用原有的 `seen_names` 去重）：
    /// - 用户自己写在 `SKILL.md` 里的技能**胜过**第三方插件带来的同名技能——
    ///   用户显式写下的那份是最强意图，插件不该顶掉它（与工具口径**刻意不同**：
    ///   工具名是注册表先占的，插件只能"重实现"）；
    /// - 插件技能**胜过**内置技能：插件是贴这个项目写的，内置是通用兜底。
    pub fn scan_skills_with(
        &self,
        workspace: Option<&str>,
        plugin_skills: &[SkillSummary],
    ) -> Vec<SkillSummary> {
        let state = self.load_state();
        let mut skills = Vec::new();
        let mut seen_names = HashSet::new();

        let mut roots: Vec<(PathBuf, &'static str)> = Vec::new();

        // 1. 工作区技能目录：**只有一处** `<ws>/.ada/agent-coding/skills`
        //
        // 2026-10-19：此前还扫 `.ada/skills` 与 `.agents/skills`，同一台机器上技能散在
        // 四个根里（工作区两个 + 全局两个），"我把技能放哪了"成了常见问题。
        // 现在统一到 `agent-coding/skills` 一处：与插件技能（`<plugin>/skills/<name>/SKILL.md`）
        // 的目录名一致，界面上「新建技能」也落在这里——**位置唯一**，别再猜。
        if let Some(ws) = workspace {
            let ws_path = PathBuf::from(ws);
            if !ws.trim().is_empty() {
                roots.push((ws_path.join(".ada").join("agent-coding").join("skills"), "workspace"));
            }
        }

        // 2. 用户全局技能目录：**只有一处** `~/.a-da/agent-coding/skills`
        let app_home = self.app_home();
        roots.push((app_home.join("agent-coding").join("skills"), "global"));

        // 旧根只用来**提示迁移**（不再扫描）：静默忽略会让用户对着"技能明明在磁盘上"发呆。
        self.warn_about_legacy_skill_dirs(workspace);

        // 3. 扫描文件系统技能
        for (root_dir, scope) in roots {
            if !root_dir.exists() {
                continue;
            }
            let files = self.find_skill_files(&root_dir, 0);
            for item in files {
                if let Ok(raw) = fs::read_to_string(&item.file_path) {
                    let (name, desc, when, invocable, body, unknown_keys) =
                        self.parse_skill_markdown(&raw, &item.file_path);
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
                        unknown_frontmatter_keys: unknown_keys,
                    });
                }
            }
        }

        // 3.5 插件声明的技能（P2·SP-45）：排在目录之后、内置之前。
        //
        // 启停状态照旧走 `skills_state.json`（id 形如 `plugin:<插件id>:<技能名>`），
        // 所以"停用某个插件技能"不需要新机制。
        for plugin_skill in plugin_skills {
            let dedupe_key = plugin_skill.name.to_lowercase();
            if seen_names.contains(&dedupe_key) {
                continue;
            }
            seen_names.insert(dedupe_key);

            let mut s = plugin_skill.clone();
            s.enabled = state.enabled_state.get(&s.id).copied().unwrap_or(true);
            skills.push(s);
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
        // 落点与扫描根**必须同一处**（2026-10-19 统一）：
        //   工作区 `<ws>/.ada/agent-coding/skills/<name>/SKILL.md`
        //   全局   `~/.a-da/agent-coding/skills/<name>/SKILL.md`
        // 早先写的是 `.ada/skills` 与 `~/.a-da/skills`，与新的扫描根分叉过一次。
        let dir = if scope == "workspace" && workspace.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            PathBuf::from(workspace.unwrap())
                .join(".ada")
                .join("agent-coding")
                .join("skills")
                .join(name)
        } else {
            self.app_home()
                .join("agent-coding")
                .join("skills")
                .join(name)
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

/// 目录里（两层内）是否存在 `.md` 文件——用来判断"旧目录里是不是真放了技能"。
fn dir_contains_markdown(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: usize) -> bool {
        if depth > 2 {
            return false;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                return true;
            }
            if path.is_dir() && walk(&path, depth + 1) {
                return true;
            }
        }
        false
    }
    walk(dir, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时"应用目录 + 工作区"一对。
    ///
    /// 🔴 应用目录**必须注入**（`with_home`）：`SkillManager::new()` 会指向用户真实的
    /// `~/.a-da`，测试写它就会污染真实配置（AGENTS.md 的三条铁律）。
    fn temp_env(tag: &str) -> (PathBuf, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "a_da_skills_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        let home = base.join("home");
        let ws = base.join("ws");
        fs::create_dir_all(&home).expect("建临时应用目录失败");
        fs::create_dir_all(ws.join(".ada").join("agent-coding").join("skills")).expect("建临时工作区失败");
        (home, ws)
    }

    fn write_workspace_skill(ws: &Path, dir: &str, content: &str) {
        let dir = ws.join(".ada").join("agent-coding").join("skills").join(dir);
        fs::create_dir_all(&dir).expect("建技能目录失败");
        fs::write(dir.join("SKILL.md"), content).expect("写技能失败");
    }

    /// SP-45：frontmatter 里认不出的键**必须被如实报出来**（以前是静默忽略）。
    ///
    /// 静默的后果很具体：作者把 `whenToUse` 拼错一个字母，技能"看起来装上了却不生效"，
    /// 而所有线索都不指向键名——他只会去反复检查正文。
    #[test]
    fn test_unknown_frontmatter_keys_are_reported() {
        let (home, ws) = temp_env("ufk");
        write_workspace_skill(
            &ws,
            "mine",
            "---\nname: 我的规范\ndescription: 说明\nwhen_to_use_typo: 写错了\n---\n正文\n",
        );

        let mgr = SkillManager::with_home(&home);
        let skills = mgr.scan_skills(Some(ws.to_str().unwrap()));
        let found = skills
            .iter()
            .find(|s| s.name == "我的规范")
            .expect("技能必须被扫到");
        assert_eq!(
            found.unknown_frontmatter_keys,
            vec!["when_to_use_typo".to_string()],
            "认不出的键要报出来，而不是静默吃掉"
        );
        // 认识的键照旧生效（不因为多了个未知键就整条作废）
        assert_eq!(found.description, "说明");
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    /// 认得出的键**不该**被误报（否则这条诊断会变成噪音，没人再看它）。
    #[test]
    fn test_known_frontmatter_keys_are_not_reported() {
        let (home, ws) = temp_env("kfk");
        write_workspace_skill(
            &ws,
            "ok",
            "---\nname: 正常技能\ndescription: 说明\nwhenToUse: 当 X 时\nuserInvocable: false\n---\n正文\n",
        );

        let mgr = SkillManager::with_home(&home);
        let skills = mgr.scan_skills(Some(ws.to_str().unwrap()));
        let found = skills.iter().find(|s| s.name == "正常技能").expect("能扫到");
        assert!(
            found.unknown_frontmatter_keys.is_empty(),
            "四个已知键都不该被报：{:?}",
            found.unknown_frontmatter_keys
        );
        assert_eq!(found.when_to_use.as_deref(), Some("当 X 时"));
        assert_eq!(found.user_invocable, Some(false));
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    /// SP-45 的核心：插件技能进**同一个扫描结果**，而且带得住归因
    /// （`plugin_id` / `plugin_name` 到这一条之前恒为空）。
    #[test]
    fn test_plugin_skills_join_the_scan_with_attribution() {
        let (home, _ws) = temp_env("pskills");
        let mgr = SkillManager::with_home(&home);
        let extra = vec![SkillSummary {
            id: "plugin:demo:deploy-runbook".to_string(),
            name: "deploy-runbook".to_string(),
            description: "部署手册".to_string(),
            body: "步骤一……".to_string(),
            path: String::new(),
            base_directory: String::new(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some("Demo 插件".to_string()),
            plugin_id: Some("demo".to_string()),
            is_file_skill: Some(false),
            user_invocable: Some(true),
            when_to_use: None,
            unknown_frontmatter_keys: Vec::new(),
        }];

        let skills = mgr.scan_skills_with(None, &extra);
        let found = skills
            .iter()
            .find(|s| s.name == "deploy-runbook")
            .expect("插件技能必须出现在扫描结果里");
        assert_eq!(found.scope, "plugin");
        assert_eq!(found.plugin_id.as_deref(), Some("demo"));
        assert_eq!(found.plugin_name.as_deref(), Some("Demo 插件"));
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    /// 同名顺序：**用户自己写的目录技能 > 插件技能 > 内置**（口径写在 `scan_skills_with`）。
    #[test]
    fn test_directory_skill_wins_over_plugin_skill_of_same_name() {
        let (home, ws) = temp_env("dedupe");
        write_workspace_skill(
            &ws,
            "dup",
            "---\nname: 同名技能\ndescription: 用户自己写的\n---\n我的正文\n",
        );

        let mgr = SkillManager::with_home(&home);
        let extra = vec![SkillSummary {
            id: "plugin:demo:同名技能".to_string(),
            name: "同名技能".to_string(),
            description: "插件带来的".to_string(),
            body: "插件正文".to_string(),
            path: String::new(),
            base_directory: String::new(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some("Demo".to_string()),
            plugin_id: Some("demo".to_string()),
            is_file_skill: Some(false),
            user_invocable: Some(true),
            when_to_use: None,
            unknown_frontmatter_keys: Vec::new(),
        }];

        let skills = mgr.scan_skills_with(Some(ws.to_str().unwrap()), &extra);
        let hits: Vec<&SkillSummary> = skills.iter().filter(|s| s.name == "同名技能").collect();
        assert_eq!(hits.len(), 1, "同名只该留一条：{hits:?}");
        assert_eq!(hits[0].scope, "workspace", "用户自己写的那份必须胜出");
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    /// 插件技能的启停照旧走 `skills_state.json`：不需要为它发明新机制。
    #[test]
    fn test_plugin_skill_can_be_disabled_through_the_state_file() {
        let (home, _ws) = temp_env("pskills_off");
        let mgr = SkillManager::with_home(&home);
        let mut state = SkillsState::default();
        state
            .enabled_state
            .insert("plugin:demo:deploy-runbook".to_string(), false);
        mgr.save_state(&state).expect("写状态失败");

        let extra = vec![SkillSummary {
            id: "plugin:demo:deploy-runbook".to_string(),
            name: "deploy-runbook".to_string(),
            description: "部署手册".to_string(),
            body: "步骤一……".to_string(),
            path: String::new(),
            base_directory: String::new(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some("Demo 插件".to_string()),
            plugin_id: Some("demo".to_string()),
            is_file_skill: Some(false),
            user_invocable: Some(true),
            when_to_use: None,
            unknown_frontmatter_keys: Vec::new(),
        }];

        let skills = mgr.scan_skills_with(None, &extra);
        let found = skills.iter().find(|s| s.name == "deploy-runbook").expect("还在列表里");
        assert!(!found.enabled, "停用状态必须被读回来（否则界面点了没用）");
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }
}
