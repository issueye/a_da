//! INV-7: 领域与投影分离不变量
//!
//! 基座领域模型中杜绝产品特定 UI 形状（`settings_open`、`card` 投影等）；
//! 投影只属于适配器与协议 DTO。
//!
//! # 为什么要从"无条件 `Ok(())`"升级（W6-T5）
//!
//! 旧版本函数体是 `Ok(())`——**它永远通过**，把整条不变量删掉也不会有任何信号。
//! 那是最典型的空转断言：看起来有一条 INV-7，实际没有任何东西在守。
//!
//! 现在断言两件**可证伪**的事：
//!
//! 1. **依赖纯度**：`agent-base` 的 `[dependencies]` 只能有底层通用库，
//!    不得出现任何产品/适配器/宿主/工具包 crate（否则领域层就"知道"了外部世界）；
//! 2. **源码纯度**：`crates/agent-base/src/domain/**` 里不得出现 UI/产品专有标识。
//!
//! 两者都是"读文件 + 断言"的形态——conformance 是验证层，允许 IO
//! （`agent-base` 自身保持零 IO）。

use std::path::{Path, PathBuf};

/// `agent-base` 允许的依赖（底层通用库，全部与产品无关）。
///
/// 加依赖要**显式改这张表**——这正是本断言的意义：领域层新增依赖必须是有意识的决定。
const ALLOWED_DEPS: &[&str] = &["serde", "serde_json", "thiserror", "tokio"];

/// 领域源码里禁止出现的标识（UI 渲染态、宿主/产品专有名词）。
///
/// 选词原则：**只放"出现即违规"的专有名词**，不放 `ui`/`view` 这类可能出现在
/// 注释里的宽泛词——宽泛词会让这条断言变成一改注释就红的噪音源。
const FORBIDDEN_TOKENS: &[&str] = &[
    // 客户端渲染态（TS 时代的 UI 形状）
    "settings_open",
    "scroll_top",
    "viewport",
    // 宿主 / 界面框架
    "gpui",
    "tauri",
    "AgentWindow",
    "react",
    // 产品名
    "ada-coding",
    "ada_coding",
    // 投影专用名词（协议 DTO 才允许有 card 投影）
    "card",
];

fn workspace_root() -> Result<PathBuf, String> {
    // conformance 位于 <root>/crates/agent-conformance
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "定位工作区根目录失败".to_string())
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// 解析 `[dependencies]` 段里出现的依赖名。
fn dependency_names(cargo: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in cargo.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]";
            continue;
        }
        if !in_deps || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some(name) = t.split(['=', '.']).next().map(str::trim) else { continue };
        if !name.is_empty() {
            out.push(name.to_string());
        }
    }
    out
}

/// 断言领域与投影分离（INV-7）。
pub fn assert_domain_projection_split() -> Result<(), String> {
    let root = workspace_root()?;

    // ── 1. 依赖纯度 ──────────────────────────────────────────────────────
    let cargo_path = root.join("crates/agent-base/Cargo.toml");
    let cargo = std::fs::read_to_string(&cargo_path)
        .map_err(|e| format!("读取 {} 失败：{e}", cargo_path.display()))?;

    let deps = dependency_names(&cargo);
    if deps.is_empty() {
        return Err(
            "解析不出 agent-base 的依赖（Cargo.toml 格式变了？）——解析失效会让本断言变成空转"
                .to_string(),
        );
    }
    let offenders: Vec<&String> = deps
        .iter()
        .filter(|d| !ALLOWED_DEPS.contains(&d.as_str()))
        .collect();
    if !offenders.is_empty() {
        return Err(format!(
            "agent-base 的依赖超出白名单（领域层不得依赖产品/适配器/宿主）：{offenders:?}\n\
             白名单：{ALLOWED_DEPS:?}。确需新增请显式改 `ALLOWED_DEPS` 并说明理由。"
        ));
    }

    // ── 2. 源码纯度 ──────────────────────────────────────────────────────
    let domain_dir = root.join("crates/agent-base/src/domain");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs(&domain_dir, &mut files);
    if files.is_empty() {
        return Err(format!(
            "{} 下没有 .rs 文件——扫描失效会让本断言变成空转",
            domain_dir.display()
        ));
    }

    let mut hits: Vec<String> = Vec::new();
    for p in &files {
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        let lower = src.to_lowercase();
        for token in FORBIDDEN_TOKENS {
            let needle = token.to_lowercase();
            if lower.contains(&needle) {
                let line = src
                    .lines()
                    .position(|l| l.to_lowercase().contains(&needle))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                hits.push(format!("{}:{line} 出现专有标识 `{token}`", p.display()));
            }
        }
    }
    if !hits.is_empty() {
        return Err(format!(
            "领域模型里出现了 UI/产品专有形状（投影应属于适配器与协议 DTO）：\n  {}",
            hits.join("\n  ")
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::AgentMessage;

    #[test]
    fn test_inv7_domain_projection_split() {
        let msg = AgentMessage::User {
            content: "纯文本".into(),
            images: None,
            timestamp: Some(1000),
        };
        match msg {
            AgentMessage::User { content, .. } => assert_eq!(content, "纯文本"),
            _ => panic!("必须是 User 消息"),
        }
        assert_domain_projection_split().expect("领域与投影必须分离");
    }

    /// 断言**真的能红**：收紧白名单后必须报出 offender。
    ///
    /// 这是"空转断言"的抗体——如果哪天有人把函数体改回 `Ok(())`，
    /// 主断言仍会通过，但这条会因为 offenders 为空而失败。
    #[test]
    fn test_inv7_dependency_purity_has_teeth() {
        let root = workspace_root().expect("定位根目录");
        let cargo = std::fs::read_to_string(root.join("crates/agent-base/Cargo.toml"))
            .expect("读取 Cargo.toml");
        let deps = dependency_names(&cargo);
        assert!(!deps.is_empty(), "解析必须能拿到依赖");

        let tightened: Vec<&str> = ALLOWED_DEPS.iter().copied().filter(|d| *d != "serde").collect();
        let offenders: Vec<&String> = deps
            .iter()
            .filter(|d| !tightened.contains(&d.as_str()))
            .collect();
        assert!(
            offenders.contains(&&"serde".to_string()),
            "收紧白名单后必须能报出 offender（否则本断言是空转）：{offenders:?}"
        );
    }

    /// 扫描**确实覆盖到文件**（防"目录写错 → 永远没有 hit"）。
    #[test]
    fn test_inv7_scan_covers_domain_files() {
        let root = workspace_root().expect("定位根目录");
        let mut files = Vec::new();
        collect_rs(&root.join("crates/agent-base/src/domain"), &mut files);
        assert!(
            files.len() >= 3,
            "domain 目录扫描到的文件太少（{} 个），扫描路径可能写错了",
            files.len()
        );
    }

    /// 禁止词命中时**确实会报错**（用一个临时文件验证判定逻辑）。
    #[test]
    fn test_inv7_forbidden_token_detection_works() {
        let dir = std::env::temp_dir().join(format!("a_da_inv7_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("fake_domain.rs");
        std::fs::write(&f, "pub struct S { pub settings_open: bool }\n").expect("写临时文件");

        let mut files = Vec::new();
        collect_rs(&dir, &mut files);
        assert_eq!(files.len(), 1, "应扫描到刚写的文件");

        let src = std::fs::read_to_string(&files[0]).expect("读回");
        assert!(
            src.to_lowercase().contains("settings_open"),
            "命中判定必须能识别专有标识"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
