//! Agent Skills 规范（`docs/SKILL_SPEC.md`）的**解析、校验与目录发现**。
//!
//! # 为什么单独一个模块
//!
//! 规范是外部契约（`SKILL.md` 的 frontmatter 格式），而项目里有**三处**要读它：
//! 插件技能装配、`skill.create`、以及诊断/自检。各处手写一套 `if name.len() > 64`
//! 的结果必然是"哪里都不完全对"——所以只在这里实现一次，别处只调用。
//!
//! # 刻意不引入 YAML 依赖
//!
//! 规范用到的 YAML 是**受限子集**：顶层标量键 + 一层 `metadata:` 嵌套 map。
//! 为此拉一个完整 YAML 解析器（锚点、多行块、类型推断）代价太大，而
//! "解析不了就报错"远比"解析成别的意思"安全：不知道怎么写的时候，报错是**可修正**的，
//! 静默解析错则是**不可发现**的。所以这里手写，并且只认 `key: value` 与
//! `metadata:` 下两空格缩进的 kv；其它形态一律报错并指出行号。
//!
//! # 技能是"目录 + SKILL.md"，不是"一个 .md 文件"
//!
//! 这是本模块存在的**行为性**理由：2026-10-19 之前插件技能是脚本里
//! `registerSkill({ name, description, body })` 声明的，与磁盘无关；现在统一到规范的
//! 目录形态（`skills/<name>/SKILL.md`），好处是 `references/`、`scripts/`、`assets/`
//! 这些配套目录有了**确定的相对根**（技能目录），而不是散在包里靠 path 猜。

use std::collections::BTreeMap;
use std::path::Path;

use crate::plugins::PluginSkillDeclaration;

/// 规范上限：`name` 最多 64 个字符。
pub const MAX_NAME_CHARS: usize = 64;
/// 规范上限：`description` 最多 1024 个字符。
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
/// 规范上限：`compatibility` 最多 500 个字符。
pub const MAX_COMPATIBILITY_CHARS: usize = 500;

/// 一个已通过校验的技能包。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    /// `metadata` 下的任意键值（规范要求是**字符串到字符串**的映射）。
    pub metadata: BTreeMap<String, String>,
    /// `allowed-tools`（空格分隔，实验性）。本项目只**接受**不实现，
    /// 留着是为了让按规范写的技能不被判为"写了非法字段"。
    pub allowed_tools: Vec<String>,
    /// Frontmatter 之后的 Markdown 正文（**不含** frontmatter）。
    pub body: String,
}

/// 校验 `name` 是否符合规范。
///
/// 规则逐条对齐 `docs/SKILL_SPEC.md`：1-64 字符、只允许 `a-z` / `0-9` / `-`、
/// 不能以连字符开头或结尾、不能出现连续连字符、必须与父目录名一致（后者见
/// [`load_skill_package`]）。
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("`name` 不能为空".to_string());
    }
    let len = n.chars().count();
    if len > MAX_NAME_CHARS {
        return Err(format!("`name` 过长（{len} 字符，上限 {MAX_NAME_CHARS}）：{n}"));
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "`name` 只能包含小写字母、数字与连字符（不允许大写/空格/中文）：{n}"
        ));
    }
    if n.starts_with('-') || n.ends_with('-') {
        return Err(format!("`name` 不能以连字符开头或结尾：{n}"));
    }
    if n.contains("--") {
        return Err(format!("`name` 不能包含连续连字符：{n}"));
    }
    Ok(())
}

/// 解析一份 `SKILL.md`（含 frontmatter 与正文）。
///
/// 失败返回**可直接给作者看的**原因（带行号），因为这条错误会原样进插件诊断面板。
pub fn parse_skill_md(raw: &str) -> Result<SkillManifest, String> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = raw.lines().enumerate();

    let first = lines
        .next()
        .map(|(_, line)| line.trim())
        .unwrap_or_default();
    if first != "---" {
        return Err(
            "缺少 YAML Frontmatter：SKILL.md 必须以 `---` 开头，第二行起写 `name` / `description`"
                .to_string(),
        );
    }

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut license: Option<String> = None;
    let mut compatibility: Option<String> = None;
    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    let mut allowed_tools: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut in_metadata = false;
    let mut body_line = None;

    for (idx, line) in lines {
        let lineno = idx + 1; // 1-based，报错用
        if line.trim() == "---" {
            body_line = Some(lineno);
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        // 缩进行只允许出现在 `metadata:` 下（规范里没有别的嵌套块）
        if line.starts_with(' ') || line.starts_with('\t') {
            if !in_metadata {
                return Err(format!(
                    "第 {lineno} 行有缩进，但不在任何块里（本解析器只支持 `metadata:` 下的一层键值）"
                ));
            }
            let (k, v) = split_kv(line.trim(), lineno)?;
            metadata.insert(k, v);
            continue;
        }

        in_metadata = false;
        let (key, value) = split_kv(line, lineno)?;
        match key.as_str() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            "license" => license = Some(value),
            "compatibility" => compatibility = Some(value),
            "allowed-tools" => {
                allowed_tools = value.split_whitespace().map(str::to_string).collect()
            }
            "metadata" => {
                if !value.is_empty() {
                    return Err(format!(
                        "第 {lineno} 行：`metadata` 必须是嵌套映射（下一行起两空格缩进写 `键: 值`），不能写成行内值"
                    ));
                }
                in_metadata = true;
            }
            other => unknown.push(format!("{other}（第 {lineno} 行）")),
        }
    }

    if !unknown.is_empty() {
        return Err(format!(
            "frontmatter 里有规范未定义的字段：{}。规范只认 name / description / license / compatibility / metadata / allowed-tools",
            unknown.join("、")
        ));
    }

    let body_line = body_line.ok_or_else(|| {
        "frontmatter 没有闭合：文件里缺少第二个 `---` 行".to_string()
    })?;
    let body = raw
        .lines()
        .skip(body_line)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    let name = name.ok_or_else(|| "frontmatter 缺少 `name`".to_string())?;
    validate_skill_name(&name)?;

    let description = description.ok_or_else(|| "frontmatter 缺少 `description`".to_string())?;
    let desc = description.trim();
    if desc.is_empty() {
        return Err("`description` 不能为空（写清这个技能做什么、什么时候用）".to_string());
    }
    let desc_len = desc.chars().count();
    if desc_len > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "`description` 过长（{desc_len} 字符，上限 {MAX_DESCRIPTION_CHARS}）"
        ));
    }

    if let Some(c) = &compatibility {
        let c = c.trim();
        if c.is_empty() {
            return Err("`compatibility` 写了但不能为空".to_string());
        }
        let len = c.chars().count();
        if len > MAX_COMPATIBILITY_CHARS {
            return Err(format!(
                "`compatibility` 过长（{len} 字符，上限 {MAX_COMPATIBILITY_CHARS}）"
            ));
        }
    }
    if let Some(l) = &license {
        if l.trim().is_empty() {
            return Err("`license` 写了但不能为空".to_string());
        }
    }
    if body.is_empty() {
        return Err(
            "正文为空：frontmatter 之后要有技能指令（步骤 / 示例 / 边缘情况），否则它进提示词只是白占字节"
                .to_string(),
        );
    }

    Ok(SkillManifest {
        name,
        description: desc.to_string(),
        license,
        compatibility,
        metadata,
        allowed_tools,
        body,
    })
}

/// 解析一行 `key: value`（值去首尾引号）。
fn split_kv(line: &str, lineno: usize) -> Result<(String, String), String> {
    let (key, value) = line
        .split_once(':')
        .ok_or_else(|| format!("第 {lineno} 行不是 `键: 值` 形态：{line}"))?;
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return Err(format!("第 {lineno} 行的键名非法：{line}"));
    }
    let value = value.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    Ok((key.to_string(), value.to_string()))
}

/// 读一个**技能目录**（`<dir>/SKILL.md`）并校验。
///
/// 比 [`parse_skill_md`] 多一条规范硬约束：`name` 必须与**父目录名**一致。
/// 这条不能只靠作者自觉——它决定了 `references/` 这类相对引用的根，写错了技能
/// 仍然能"加载成功"，但所有相对路径都会指向错误的地方。
pub fn load_skill_package(skill_md: &Path) -> Result<SkillManifest, String> {
    let file_name = skill_md
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if file_name != "SKILL.md" {
        return Err(format!(
            "技能入口必须叫 `SKILL.md`（当前是 `{file_name}`）"
        ));
    }
    let dir_name = skill_md
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let raw = std::fs::read_to_string(skill_md)
        .map_err(|e| format!("读不到 {}：{e}", skill_md.display()))?;
    let manifest = parse_skill_md(&raw)?;
    if manifest.name != dir_name {
        return Err(format!(
            "`name`（{}）必须与技能目录名（{}）一致",
            manifest.name, dir_name
        ));
    }
    Ok(manifest)
}

/// 扫描一个插件包里的技能目录，返回（可用技能, 问题清单）。
///
/// 约定：`<plugin_root>/skills/<name>/SKILL.md`。
///
/// # 为什么旧格式**明确报错**而不是兼容
///
/// 2026-10-19 之前插件技能是 `skills/<name>.md` 裸文件或脚本 `registerSkill` 内联 body。
/// 现在按规范统一到目录形态；旧的散文件如果静默忽略，作者会看到"技能明明在磁盘上，
/// 界面上却没有"——所以这里把它当成**要报出来的问题**（`skill_spec_violation:*`）。
pub fn discover_plugin_skills(plugin_root: &Path) -> (Vec<PluginSkillDeclaration>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();

    let skills_dir = plugin_root.join("skills");
    if !skills_dir.is_dir() {
        return (found, problems);
    }
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(e) => {
            problems.push(format!("读不到 skills/ 目录：{e}"));
            return (found, problems);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        if path.is_file() {
            // 旧格式：`skills/<name>.md`
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                problems.push(format!(
                    "`skills/{name}` 是旧格式（单个 .md）：技能必须是目录 + `SKILL.md`，\
                     例如 `skills/{}/SKILL.md`（见 docs/SKILL_SPEC.md）",
                    name.trim_end_matches(".md")
                ));
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            problems.push(format!(
                "技能目录 `skills/{name}/` 缺少 `SKILL.md`（大小写敏感）"
            ));
            continue;
        }
        match load_skill_package(&skill_md) {
            Ok(m) => found.push(PluginSkillDeclaration {
                name: m.name,
                description: m.description,
                path: Some(skill_md.to_string_lossy().to_string()),
                body: m.body,
                // 规范里没有这两个字段（本项目自有扩展）：目录技能不给，交给上层默认。
                when_to_use: None,
                user_invocable: Some(true),
            }),
            Err(e) => problems.push(format!("技能 `skills/{name}/` 不符合 SKILL_SPEC：{e}")),
        }
    }

    // 目录遍历顺序不稳定：按名字排序，保证装配结果与诊断顺序可复现。
    found.sort_by(|a, b| a.name.cmp(&b.name));
    problems.sort();
    (found, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_md(name: &str, desc: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n")
    }

    #[test]
    fn test_parses_a_minimal_valid_skill() {
        let m = parse_skill_md(&ok_md("pdf-processing", "提取 PDF 文本。Use when …", "## 步骤\n1. 打开")).unwrap();
        assert_eq!(m.name, "pdf-processing");
        assert_eq!(m.description, "提取 PDF 文本。Use when …");
        assert!(m.body.contains("## 步骤"), "正文不含 frontmatter：{}", m.body);
        assert!(m.license.is_none() && m.compatibility.is_none());
    }

    #[test]
    fn test_parses_optional_fields_including_nested_metadata() {
        let raw = "---\nname: pdf\ndescription: 处理 PDF\nlicense: Apache-2.0\ncompatibility: Requires git and docker\nmetadata:\n  author: me\n  version: \"1.0\"\nallowed-tools: Bash(git:*) Read\n---\n正文\n";
        let m = parse_skill_md(raw).unwrap();
        assert_eq!(m.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(m.compatibility.as_deref(), Some("Requires git and docker"));
        assert_eq!(m.metadata.get("author").map(String::as_str), Some("me"));
        assert_eq!(m.metadata.get("version").map(String::as_str), Some("1.0"), "引号要去掉");
        assert_eq!(m.allowed_tools, vec!["Bash(git:*)".to_string(), "Read".to_string()]);
    }

    #[test]
    fn test_rejects_invalid_names() {
        for bad in ["PDF-Processing", "-pdf", "pdf-", "pdf--processing", "pdf processing", "技能"] {
            let err = parse_skill_md(&ok_md(bad, "说明", "正文")).unwrap_err();
            assert!(err.contains("name") || err.contains("`name`"), "`{bad}` 应被拒：{err}");
        }
        let long = "a".repeat(65);
        assert!(parse_skill_md(&ok_md(&long, "说明", "正文")).is_err(), "超过 64 字符要拒");
    }

    #[test]
    fn test_rejects_bad_frontmatter_shapes() {
        // 没有 frontmatter
        assert!(parse_skill_md("正文而已").unwrap_err().contains("Frontmatter"));
        // 没闭合
        assert!(parse_skill_md("---\nname: x\n").unwrap_err().contains("闭合"));
        // 缺 description
        assert!(parse_skill_md("---\nname: x\n---\n正文").unwrap_err().contains("description"));
        // description 超长
        let long = "描".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert!(parse_skill_md(&ok_md("x", &long, "正文")).unwrap_err().contains("过长"));
        // 未知字段要报出来（不静默）
        let err = parse_skill_md("---\nname: x\ndescription: d\nwhen_to_use: 随便\n---\n正文").unwrap_err();
        assert!(err.contains("未定义"), "{err}");
        // 空正文
        assert!(parse_skill_md("---\nname: x\ndescription: d\n---\n   ").unwrap_err().contains("正文"));
    }

    #[test]
    fn test_name_must_match_directory() {
        let dir = std::env::temp_dir().join(format!("a_da_skill_spec_{}", std::process::id()));
        let skill_dir = dir.join("actual-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let md = skill_dir.join("SKILL.md");
        std::fs::write(&md, ok_md("other-name", "说明", "正文")).unwrap();
        let err = load_skill_package(&md).unwrap_err();
        assert!(err.contains("必须与技能目录名"), "{err}");

        std::fs::write(&md, ok_md("actual-name", "说明", "正文")).unwrap();
        assert!(load_skill_package(&md).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discovers_directory_skills_and_reports_old_loose_files() {
        let root = std::env::temp_dir().join(format!("a_da_skill_disc_{}", std::process::id()));
        let skills = root.join("skills");
        std::fs::create_dir_all(skills.join("good-one")).unwrap();
        std::fs::write(skills.join("good-one").join("SKILL.md"), ok_md("good-one", "说明", "正文")).unwrap();
        std::fs::create_dir_all(skills.join("no-entry")).unwrap();
        std::fs::write(skills.join("legacy.md"), "旧格式").unwrap();

        let (found, problems) = discover_plugin_skills(&root);
        assert_eq!(found.len(), 1, "只认目录 + SKILL.md：{problems:?}");
        assert_eq!(found[0].name, "good-one");
        assert!(found[0].path.as_deref().unwrap_or("").ends_with("SKILL.md"));
        assert!(
            problems.iter().any(|p| p.contains("旧格式") && p.contains("legacy.md")),
            "旧的裸 .md 要明确报出来：{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("缺少 `SKILL.md`")),
            "缺入口的目录要报：{problems:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
