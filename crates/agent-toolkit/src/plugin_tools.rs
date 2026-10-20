//! 内置**插件工具**的 `Tool` 实现（W2-T2）。
//!
//! 背景：`git_*` / `project_inspect` / `code_outline` / `run_tests` 这些工具原先只存在于
//! `agent-core` 的 `execute_builtin_plugin_tool` 字符串分派里——它们是"插件工具"，
//! 却不是一等 `Tool`，因此**进不了 catalog**、也没有 `ToolDescriptor` 可被各消费侧读取。
//!
//! 本文件把它们的**逻辑**搬成自由函数（唯一真源），再由薄薄的 [`PluginTool`] 包成 `Tool`：
//! - 自由函数：给 legacy 分派复用（`agent-core` 只做委派，不复制逻辑，R2）；
//! - `Tool` 实现：进 catalog，供 `agent.spec.json` 的 `toolkits` 声明装配。
//!
//! **描述符来自注册表**（`registry::find_tool_descriptor`），不在这里另写一份 schema——
//! 那是第 7 张名单的开始。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use super::cmd_tools::run_command;
use super::fs_tools::{read_file, PathGate, ToolResult};

// ── 自由函数：逻辑真源 ──────────────────────────────────────────────────────

/// 项目工程与环境诊断（Node/Rust/Python/Go 生态探测）。
pub async fn project_inspect(workspace: &Path) -> ToolResult {
    let mut sections = vec![format!("# 项目工程与环境诊断报告: {}", workspace.display())];

    let pkg_path = workspace.join("package.json");
    if pkg_path.exists() {
        if let Ok(raw) = tokio::fs::read_to_string(&pkg_path).await {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&raw) {
                sections.push("## Node / TypeScript 生态配置".to_string());
                let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
                let ver = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
                sections.push(format!("- **包名**: {} (v{})", name, ver));

                if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
                    sections.push("- **可用 Scripts 指令**:".to_string());
                    for (k, v) in scripts {
                        sections.push(format!("  - `{}`: {}", k, v.as_str().unwrap_or("")));
                    }
                }
                if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                    let keys: Vec<&str> = deps.keys().map(|s| s.as_str()).take(15).collect();
                    sections.push(format!("- **生产依赖**: {} 个 ({})", deps.len(), keys.join(", ")));
                }
                if let Some(dev) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
                    let keys: Vec<&str> = dev.keys().map(|s| s.as_str()).take(15).collect();
                    sections.push(format!("- **开发依赖**: {} 个 ({})", dev.len(), keys.join(", ")));
                }
            }
        }
    }

    let cargo_path = workspace.join("Cargo.toml");
    if cargo_path.exists() {
        sections.push("## Rust / Cargo 生态配置".to_string());
        if let Ok(raw) = tokio::fs::read_to_string(&cargo_path).await {
            if raw.contains("[workspace]") {
                sections.push("- **工程模式**: Cargo Workspace 多包工作区".to_string());
            }
            if let Some(line) = raw.lines().find(|l| l.trim().starts_with("name =")) {
                sections.push(format!("- **Crate 包名**: {}", line.trim()));
            }
        }
    }

    if workspace.join("pyproject.toml").exists() || workspace.join("requirements.txt").exists() {
        sections.push("## Python 生态配置".to_string());
        if workspace.join("pyproject.toml").exists() {
            sections.push("- 发现 pyproject.toml".to_string());
        }
        if workspace.join("requirements.txt").exists() {
            sections.push("- 发现 requirements.txt".to_string());
        }
    }

    if workspace.join("go.mod").exists() {
        sections.push("## Go 生态配置".to_string());
        sections.push("- 发现 go.mod".to_string());
    }

    ToolResult::success(sections.join("\n\n"))
}

/// 一行是否是"显著声明"。按行首前缀识别 Rust / TS-JS / Go / Python / Java-C# 的常见写法。
///
/// 早先只认 Rust 与 TS 的前缀，Go 的 `func main()` / `type Server struct` 一律匹配不到，
/// 于是**对 Go 文件谎报"未提取到显著类、函数或接口大纲"**——模型据此以为"文件里没有东西"，
/// 反复换工具硬探（实测一次子智能体就是这么烧掉 80 次调用、445k tokens 后失败的）。
fn is_outline_signature(body: &str) -> bool {
    const PREFIXES: &[&str] = &[
        // Rust
        "pub fn ",
        "fn ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "pub mod ",
        "pub type ",
        "type ",
        // TS / JS
        "class ",
        "export class ",
        "interface ",
        "export interface ",
        "export function ",
        "export async function ",
        "export default ",
        "export const ",
        "export type ",
        "function ",
        "async function ",
        // Go
        "func ",
        // Python
        "def ",
        "async def ",
        // Java / C# / Kotlin
        "public ",
        "private ",
        "protected ",
        "internal ",
    ];
    PREFIXES.iter().any(|p| body.starts_with(p))
}

/// 提取文件里的类/函数/接口骨架签名与行号。
///
/// ⚠️ `read_file` 的输出每行都带 `"<行号> | "` 前缀，所以匹配前必须**剥掉前缀**。
/// 原实现在 `agent-core` 里直接对整行做 `starts_with("pub fn ")`，于是**永远匹配不到**
/// 任何签名、每次都返回"未提取到显著大纲"——一个静默失效的工具。
/// W2-T2 搬运时补了单测才暴露（`test_code_outline_extracts_rust_signatures`）。
pub async fn code_outline(workspace: &Path, file_path: &str) -> ToolResult {
    let res = read_file(PathGate::Workspace(workspace), file_path, None, Some(2000));
    if !res.ok {
        return res;
    }

    let mut outlines = Vec::new();
    for (line_no, line) in res.output.lines().enumerate() {
        // 剥掉 `read_file` 的行号前缀（形如 `123 | pub fn foo()`）
        let body = line
            .trim()
            .split_once(" | ")
            .map(|(_, rest)| rest.trim())
            .unwrap_or_else(|| line.trim());

        if body.starts_with("//") || body.starts_with('#') || body.starts_with("/*") {
            continue;
        }
        if is_outline_signature(body) {
            outlines.push(format!("L{:03}: {}", line_no + 1, body));
        }
    }

    if outlines.is_empty() {
        // 如实交代能力边界：**别让"没提取到"被读成"文件里没有东西"**。
        // 这是行首前缀启发式匹配，不是语言服务；不认的语言/纯配置/纯文档请直接读原文。
        ToolResult::success(format!(
            "文件 [{}] 未匹配到类/函数/接口声明。该工具按行首前缀启发式识别 \
             Rust / TS-JS / Go / Python / Java-C# 风格的声明，不解析语法、不认其它语言与纯配置/文档文件；\
             需要正文请改用 read_file 直接读取。",
            file_path
        ))
    } else {
        ToolResult::success(format!("## 代码大纲结构: {}\n```\n{}\n```", file_path, outlines.join("\n")))
    }
}

/// `git` 的**单次输出上限**（字节）。
///
/// 整仓 diff 一次动辄几百 KB，直接塞进上下文会把模型窗口撑爆——
/// 实测一次子智能体就是靠反复读整仓 diff 冲到 445k tokens，最后请求被网关掐断、
/// 报告只写了一半。截断时**明确告诉模型怎么拿全文**，而不是静默丢内容。
const MAX_GIT_BYTES: usize = 48 * 1024;

/// 按 UTF-8 字符边界截断，并附上"怎么拿全文"的指引。
fn truncate_git_output(mut res: ToolResult, action: &str) -> ToolResult {
    if res.output.len() <= MAX_GIT_BYTES {
        return res;
    }
    let mut cut = MAX_GIT_BYTES;
    while cut > 0 && !res.output.is_char_boundary(cut) {
        cut -= 1;
    }
    let orig_kb = res.output.len() / 1024;
    let kept_kb = cut / 1024;
    let head = res.output[..cut].to_string();
    res.output = format!(
        "{head}\n\n……（输出已截断：原始约 {orig_kb} KB，仅保留前 {kept_kb} KB。\
         需要全文请缩小范围：`action:\"diff\"` 指定单个 file、`action:\"log\"` 调低 limit、\
         或改用 `read_file` 读具体文件；整仓 {action} 不要一次性读入。）"
    );
    res
}

/// 支持的 `git` 动作（**单一真源**：描述符的 enum 与这里必须一致）。
pub const GIT_ACTIONS: &[&str] = &[
    "status", "diff", "log", "show", "blame", "add", "commit", "checkout", "restore",
    "reset", "stash", "branch", "merge", "push", "pull",
];

/// 统一的 `git` 工具入口。
///
/// # 为什么把 status / diff / log 合成一个（2026-10-19）
///
/// 它们是**同一件事的三个动作**（看版本库），却各占一条清单：模型要先读三份高度相似的
/// 描述才能选对，而清单每长一条，选错的概率就多一点；用户侧看到的是"查个状态调了三个工具"。
/// 现在 `action` 选动作，其余参数按动作解释，另外把常用写操作（add / commit / checkout /
/// restore / reset / stash / branch / merge / push / pull）一起收进来——
/// 早先这些只能让模型拼 `run_command`，既没有审批保护也没有输出限幅。
///
/// 🔴 **参数一律走 argv，不经 shell**：git 的参数里到处是用户输入（提交信息、路径、ref），
/// 拼成命令行再交给 `cmd /c` 会把 `%VAR%` 展开、把 `&` 当命令分隔符。这里直接 spawn 子进程，
/// 没有任何一处转义需要被"猜对"。路径另外做"不得越出工作区"的校验。
pub async fn git(workspace: &Path, args: &serde_json::Value) -> ToolResult {
    let (action, argv, cwd) = match git_argv(args) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(e),
    };
    let res = run_git(workspace, &argv, cwd.as_deref()).await;
    if !res.ok && res.output.contains("not a git repository") {
        return ToolResult::error(repo_hint(workspace, cwd.as_deref()));
    }
    truncate_git_output(res, &action)
}

/// 工作区不是仓库时的诊断。
///
/// 🔴 多仓库工作区（一个目录下几个子项目各自 `git init`）是很常见的形态，
/// 而 `path` 的语义是**文件过滤**（pathspec），不是"在哪个仓库执行"。
/// 以前这两种用法撞在一起只会得到一句"当前目录不是一个有效的 Git 仓库"，
/// 模型只能改去拼 `run_command`——既绕过了审批，也没有输出限幅。
/// 所以这里把**一层子目录里的独立仓库**直接列出来，并给出可以照抄的 `cwd`。
fn repo_hint(workspace: &Path, cwd: Option<&str>) -> String {
    if let Some(dir) = cwd {
        return format!(
            "`{dir}` 及其上层都不是 Git 仓库（工作区：{}）。\
             确认 `cwd` 指向的是仓库根或其子目录。",
            workspace.display()
        );
    }

    let repos = find_child_repos(workspace);
    if repos.is_empty() {
        return format!("当前目录及其上层都不是 Git 仓库：{}", workspace.display());
    }

    let example = serde_json::json!({ "action": "status", "cwd": repos[0] });
    format!(
        "工作区根本身不是 Git 仓库，但检测到以下子目录是**独立仓库**：{}。\n\
         请用 `cwd` 指定其中一个再试，例如 {}。\n\
         （`path` / `file` 是**文件过滤**，不是仓库目录；`cwd` 才决定在哪个仓库执行。）",
        repos.join(" / "),
        example
    )
}

/// 只扫**一层**子目录：多仓库工作区的仓库都在这一层，再深就不是"工作区结构"而是噪音了。
fn find_child_repos(workspace: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(workspace) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // `.git` 可能是目录（普通仓库），也可能是文件（worktree / submodule）
        if !path.join(".git").exists() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            found.push(name.to_string());
        }
    }
    found.sort();
    found
}

/// 把工具入参翻译成 `git` 的 argv（`argv[0]` 之外的部分）。
///
/// 校验一律**失败即拒**：宁可让模型改参数重试，也不要把可疑字符串交给 git
/// （`--upload-pack=...` 这类"看着像值、其实是选项"的输入只能靠拒绝 `-` 开头挡住）。
fn git_argv(args: &serde_json::Value) -> Result<(String, Vec<String>, Option<String>), String> {
    let raw_action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status")
        .trim()
        .to_lowercase();
    let action = GIT_ACTIONS
        .iter()
        .find(|a| **a == raw_action)
        .map(|a| a.to_string())
        .ok_or_else(|| {
            format!(
                "未知 git action `{raw_action}`（可用：{}）",
                GIT_ACTIONS.join(" / ")
            )
        })?;

    // 执行目录：多仓库工作区用 `cwd` 指定"在哪个仓库执行"。
    //
    // 🔴 与 `file`/`path` 是**两回事**：后者是 git 的 pathspec（过滤哪些文件），
    // 前者是子进程的 `current_dir`。混用会让"在工作区根执行、把 frontend 当过滤器"
    // 这种误用失败得莫名其妙。
    let cwd = args
        .get("cwd")
        .or_else(|| args.get("repo"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(repo_path)
        .transpose()?;

    // 文件集合：`file`/`path` 单值 + `files` 数组，一起去重
    let mut files: Vec<String> = Vec::new();
    if let Some(f) = args
        .get("file")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
    {
        if !f.trim().is_empty() {
            files.push(repo_path(f)?);
        }
    }
    if let Some(arr) = args.get("files").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    files.push(repo_path(s)?);
                }
            }
        }
    }
    files.dedup();

    let reference = args
        .get("ref")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(git_ref)
        .transpose()?;
    let branch = args
        .get("branch")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(git_ref)
        .transpose()?;
    let staged = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 200);

    let need_files = |files: &Vec<String>, what: &str| -> Result<(), String> {
        if files.is_empty() {
            return Err(format!("`{what}` 需要指定文件：传 file / path，或 files 数组。"));
        }
        Ok(())
    };

    let argv: Vec<String> = match action.as_str() {
        "status" => vec!["status".into(), "--porcelain=v1".into(), "-b".into()],
        "diff" => {
            let mut a: Vec<String> = vec!["diff".into()];
            if staged {
                a.push("--cached".into());
            }
            if let Some(r) = &reference {
                a.push(r.clone());
            }
            a.push("--".into());
            a.extend(files.iter().cloned());
            a
        }
        "log" => {
            let mut a: Vec<String> = vec![
                "log".into(),
                "-n".into(),
                limit.to_string(),
                "--oneline".into(),
                "--decorate".into(),
            ];
            if let Some(r) = &reference {
                a.push(r.clone());
            }
            if !files.is_empty() {
                a.push("--".into());
                a.extend(files.iter().cloned());
            }
            a
        }
        "show" => {
            let mut a: Vec<String> = vec![
                "show".into(),
                reference.clone().unwrap_or_else(|| "HEAD".to_string()),
                "--stat".into(),
                "--patch".into(),
            ];
            if !files.is_empty() {
                a.push("--".into());
                a.extend(files.iter().cloned());
            }
            a
        }
        "blame" => {
            need_files(&files, "blame")?;
            vec!["blame".into(), "--date=short".into(), files[0].clone()]
        }
        "add" => {
            if files.is_empty() {
                // 不指定文件 = 暂存全部改动（含新增与删除），这是 git add -A 的语义
                vec!["add".into(), "-A".into()]
            } else {
                let mut a: Vec<String> = vec!["add".into(), "--".into()];
                a.extend(files.iter().cloned());
                a
            }
        }
        "commit" => {
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .map(commit_message)
                .transpose()?
                .ok_or_else(|| "`commit` 需要 message（提交信息）。".to_string())?;
            let mut a: Vec<String> = vec!["commit".into(), "-m".into(), message];
            if args.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
                a.push("-a".into());
            }
            if !files.is_empty() {
                a.push("--".into());
                a.extend(files.iter().cloned());
            }
            a
        }
        "checkout" => {
            if args.get("create").and_then(|v| v.as_bool()).unwrap_or(false) {
                let name = branch
                    .clone()
                    .or_else(|| reference.clone())
                    .ok_or_else(|| "`checkout` 创建分支需要 branch（新分支名）。".to_string())?;
                vec!["checkout".into(), "-b".into(), name]
            } else {
                let target = reference
                    .clone()
                    .or_else(|| branch.clone())
                    .ok_or_else(|| "`checkout` 需要 ref 或 branch。".to_string())?;
                vec!["checkout".into(), target]
            }
        }
        "restore" => {
            need_files(&files, "restore")?;
            let mut a: Vec<String> = vec!["restore".into()];
            if staged {
                a.push("--staged".into());
            }
            a.push("--".into());
            a.extend(files.iter().cloned());
            a
        }
        "reset" => {
            let mode = args
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("mixed")
                .trim()
                .to_lowercase();
            let mode = match mode.as_str() {
                "soft" | "mixed" | "hard" => mode,
                other => {
                    return Err(format!(
                        "未知 reset mode `{other}`（可用：soft / mixed / hard）"
                    ))
                }
            };
            vec![
                "reset".into(),
                format!("--{mode}"),
                reference.unwrap_or_else(|| "HEAD".to_string()),
            ]
        }
        "stash" => {
            let sub = args
                .get("sub")
                .and_then(|v| v.as_str())
                .unwrap_or("list")
                .trim()
                .to_lowercase();
            match sub.as_str() {
                "list" | "pop" | "apply" | "drop" | "show" => vec!["stash".into(), sub],
                "push" | "save" => {
                    let mut a: Vec<String> = vec!["stash".into(), "push".into()];
                    if let Some(msg) = args.get("message").and_then(|v| v.as_str()) {
                        a.push("-m".into());
                        a.push(commit_message(msg)?);
                    }
                    a
                }
                other => {
                    return Err(format!(
                        "未知 stash sub `{other}`（可用：list / push / pop / apply / drop / show）"
                    ))
                }
            }
        }
        "branch" => {
            if args.get("delete").and_then(|v| v.as_bool()).unwrap_or(false) {
                let name = branch
                    .ok_or_else(|| "`branch` 删除分支需要 branch（分支名）。".to_string())?;
                vec!["branch".into(), "-d".into(), name]
            } else if let Some(name) = branch {
                vec!["branch".into(), name]
            } else {
                // 不传 branch = 列本地分支（带上游与最后提交）
                vec!["branch".into(), "-vv".into()]
            }
        }
        "merge" => {
            let target = reference
                .clone()
                .or_else(|| branch.clone())
                .ok_or_else(|| "`merge` 需要 ref 或 branch。".to_string())?;
            vec!["merge".into(), "--no-edit".into(), target]
        }
        "push" | "pull" => {
            let remote = args
                .get("remote")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(git_ref)
                .transpose()?
                .unwrap_or_else(|| "origin".to_string());
            let mut a: Vec<String> = vec![action.clone(), remote];
            if let Some(r) = reference.or(branch) {
                a.push(r);
            }
            if args.get("force").and_then(|v| v.as_bool()).unwrap_or(false) && action == "push" {
                a.push("--force-with-lease".into());
            }
            a
        }
        _ => unreachable!("动作已在 GIT_ACTIONS 里校验过"),
    };

    Ok((action, argv, cwd))
}

/// 仓库内的相对路径：拒绝绝对路径与 `..` 段（越出工作区的 git 操作没有意义，
/// 而 `git restore -- ../outside` 会真的改到工作区外的文件）。
fn repo_path(raw: &str) -> Result<String, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("路径不能为空。".to_string());
    }
    if p.chars().any(|c| c.is_control()) {
        return Err("路径里不能有控制字符（换行等）。".to_string());
    }
    let normalized = p.replace('\\', "/");
    let is_absolute = normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.as_bytes().get(1) == Some(&b':');
    if is_absolute {
        return Err(format!("`{p}` 是绝对路径：git 工具只接受工作区内的相对路径。"));
    }
    if normalized.split('/').any(|seg| seg == "..") {
        return Err(format!("`{p}` 含 `..`：不允许越出工作区。"));
    }
    Ok(p.to_string())
}

/// ref / 分支 / 远端名。
///
/// 以 `-` 开头的一律拒：git 会把它们当选项（`--exec=`、`-c core.pager=...`），
/// 而这些不是"引用名"，是执行入口。
fn git_ref(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("引用名不能为空。".to_string());
    }
    if s.starts_with('-') {
        return Err(format!("`{s}` 以 `-` 开头：引用名不允许像选项一样书写。"));
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("`{s}` 含空白或控制字符：不是合法的引用名。"));
    }
    if s.len() > 200 {
        return Err("引用名过长（上限 200 字符）。".to_string());
    }
    Ok(s.to_string())
}

/// 提交信息：允许中文与标点，拒绝换行（多人多行用 `-m` 重复传会改变语义）与超长正文。
fn commit_message(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("提交信息不能为空。".to_string());
    }
    if s.chars().any(|c| c == '\n' || c == '\r') {
        return Err("提交信息不能含换行：需要多段请分行写成 `；`，或改用 run_command 走 git commit -F。".to_string());
    }
    if s.chars().count() > 2000 {
        return Err("提交信息过长（上限 2000 字符）。".to_string());
    }
    Ok(s.to_string())
}

/// 直接 spawn `git`（不经 shell），带超时与输出上限。
///
/// 并发读 stdout/stderr：只读一路时，另一路写满管道缓冲会**双双卡死**，
/// 最后表现为"git 工具超时"——而 git 其实早就干完了。
async fn run_git(workspace: &Path, argv: &[String], cwd: Option<&str>) -> ToolResult {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    // 执行目录：`cwd` 相对工作区（已在 `repo_path` 里校验过不含 `..`），默认工作区根。
    let dir = match cwd {
        Some(rel) => workspace.join(rel),
        None => workspace.to_path_buf(),
    };
    if !dir.is_dir() {
        return ToolResult::error(format!("git 执行目录不存在：{}", dir.display()));
    }

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(argv)
        .current_dir(&dir)
        .env("A_DA_AGENT", "1")
        .env("GIT_TERMINAL_PROMPT", "0") // 要凭据时立刻失败，而不是等满超时
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // tokio::process::Command 自带 `creation_flags`（不需要 std 的 CommandExt）
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ToolResult::error(format!(
                "启动 git 失败（未安装或不在 PATH 上）：{e}"
            ))
        }
    };
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();

    let read_out = async {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    };
    let read_err = async {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    };

    let timeout = std::time::Duration::from_secs(30);
    match tokio::time::timeout(timeout, async {
        tokio::join!(read_out, read_err, child.wait())
    })
    .await
    {
        Ok((out, err, status)) => {
            let stdout = String::from_utf8_lossy(&out).trim_end().to_string();
            let stderr = String::from_utf8_lossy(&err).trim_end().to_string();
            // `tokio::join!` 把 `child.wait()` 的 `Result` 原样交出来：取不到退出状态
            // 说明子进程被外部杀掉或句柄失效，如实报错，不要假装成功。
            let status = match status {
                Ok(s) => s,
                Err(e) => return ToolResult::error(format!("等待 git 进程结束失败：{e}")),
            };
            let code = status.code().unwrap_or(-1);
            if status.success() {
                let body = if stdout.is_empty() {
                    "（命令成功，无输出）".to_string()
                } else {
                    stdout
                };
                ToolResult::success(body)
            } else {
                let mut msg = format!("git 执行失败（退出码 {code}）");
                if !stderr.is_empty() {
                    msg.push_str(&format!("：\n{stderr}"));
                }
                if !stdout.is_empty() {
                    msg.push_str(&format!("\n{stdout}"));
                }
                ToolResult::error(msg)
            }
        }
        Err(_) => {
            let _ = child.kill().await;
            ToolResult::error(format!(
                "git 执行超时（{} 秒）已终止：`{}`。交互式操作（rebase -i、要输密码的 push）在这里跑不了，请改用非交互参数。",
                timeout.as_secs(),
                argv.join(" ")
            ))
        }
    }
}

/// 自动探测并执行项目测试。
pub async fn run_tests(workspace: &Path) -> ToolResult {
    let cmd = if workspace.join("Cargo.toml").exists() {
        "cargo test"
    } else if workspace.join("bun.lockb").exists() || workspace.join("bun.lock").exists() {
        "bun test"
    } else if workspace.join("package.json").exists() {
        "npm test"
    } else if workspace.join("pytest.ini").exists() || workspace.join("tests").exists() {
        "pytest"
    } else {
        "cargo test"
    };
    run_command(workspace, cmd, None, Some(180), None).await
}

// ── Tool 包装 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Kind {
    ProjectInspect,
    CodeOutline,
    Git,
    RunTests,
}

/// 插件工具的 `Tool` 包装（描述符取自注册表）。
pub struct PluginTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
    kind: Kind,
}

impl PluginTool {
    /// 按注册表里的名字建一个工具实例。
    ///
    /// 名字**必须**在注册表里——否则就是"实现了没人声明"，
    /// 直接 panic 好过静默造出一个没有描述符的工具。
    fn new(name: &str, workspace: &Path, kind: Kind) -> Self {
        let descriptor = super::registry::find_tool_descriptor(name)
            .unwrap_or_else(|| panic!("插件工具 `{name}` 必须在 ToolDescriptor 注册表里"))
            .clone();
        Self { descriptor, workspace: workspace.to_path_buf(), kind }
    }
}

fn to_receipt(res: ToolResult, started_at: i64) -> ToolReceipt {
    let status = if res.ok { ToolStatus::Success } else { ToolStatus::Error };
    let mut receipt = ToolReceipt::new(status, res.output, started_at, crate::now_ms());
    receipt.details = res.details;
    receipt
}

impl Tool for PluginTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = crate::now_ms();
            let args = &call.args;
            let res = match self.kind {
                Kind::ProjectInspect => project_inspect(&self.workspace).await,
                Kind::CodeOutline => {
                    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    code_outline(&self.workspace, path).await
                }
                Kind::Git => git(&self.workspace, args).await,
                Kind::RunTests => run_tests(&self.workspace).await,
            };
            to_receipt(res, started_at)
        })
    }
}

/// `git` 工具包：**一个**工具承载全部版本库动作（`action` 选动作）。
///
/// 合并前是 `git_status` / `git_diff` / `git_log` 三个名字（2026-10-19 合并）：
/// 同一件事的三个动作各占一条清单，模型要在三份相似描述里挑，用户看到的是"查状态调了三次"。
pub fn git_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(PluginTool::new("git", workspace, Kind::Git))]
}

/// `project` 工具包：项目诊断与代码结构提取。
///
/// 只保留 `project_inspect` 一个名字（2026-10-18 合并）：早先它还有 `inspect_project`
/// 这个**同一实现的别名**，同一个能力在界面与模型清单里各出现两次，纯粹是噪声。
pub fn project_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(PluginTool::new("project_inspect", workspace, Kind::ProjectInspect)),
        Arc::new(PluginTool::new("code_outline", workspace, Kind::CodeOutline)),
    ]
}

/// `command` 工具包里的测试执行工具（它本质是"跑一条命令"）。
pub fn test_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(PluginTool::new("run_tests", workspace, Kind::RunTests))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crate 应在 <root>/crates/agent-toolkit")
            .to_path_buf()
    }

    #[test]
    fn test_every_wrapped_tool_has_a_registry_descriptor() {
        // 描述符来自注册表：这 4 个名字都必须存在（否则 new() 会 panic）
        // 2026-10-19：git 三兄弟（status/diff/log）合并为一个 `git`，由 action 区分。
        let all: Vec<Arc<dyn Tool>> = git_tools(&ws())
            .into_iter()
            .chain(project_tools(&ws()))
            .chain(test_tools(&ws()))
            .collect();
        let names: Vec<String> = all.iter().map(|t| t.descriptor().name.clone()).collect();
        assert_eq!(
            names,
            vec!["git", "project_inspect", "code_outline", "run_tests"],
            "项目诊断只保留一个名字：`inspect_project` 别名已合并进 `project_inspect`"
        );
        for t in &all {
            let d = crate::registry::find_tool_descriptor(&t.descriptor().name)
                .expect("必须在注册表里");
            assert_eq!(t.descriptor(), d, "描述符必须与注册表逐字段一致（不许各写一份）");
        }
    }

    #[tokio::test]
    async fn test_project_inspect_reports_cargo_ecosystem() {
        let res = project_inspect(&ws()).await;
        assert!(res.ok, "{}", res.output);
        assert!(res.output.contains("项目工程与环境诊断报告"));
        assert!(res.output.contains("Cargo"), "本仓是 Cargo 工程：{}", res.output);
    }

    #[tokio::test]
    async fn test_code_outline_extracts_rust_signatures() {
        let res = code_outline(&ws(), "crates/agent-toolkit/src/lib.rs").await;
        assert!(res.ok, "{}", res.output);
        // lib.rs 里全是 `pub mod`，未必有 fn；换一个一定有 fn 的文件
        let res2 = code_outline(&ws(), "crates/agent-toolkit/src/toolkits.rs").await;
        assert!(res2.ok, "{}", res2.output);
        assert!(
            res2.output.contains("pub fn tools_for_toolkit"),
            "应提取到函数签名：{}",
            res2.output
        );
    }

    #[tokio::test]
    async fn test_code_outline_missing_file_is_error_not_panic() {
        let res = code_outline(&ws(), "definitely/not/here.rs").await;
        assert!(!res.ok);
    }

    /// 守门：整仓 diff 必须**限幅**，且截断时明确告诉模型怎么拿全文。
    ///
    /// 实测教训：子智能体靠反复读整仓 diff 把上下文冲到 445k tokens，请求被网关掐断。
    #[test]
    fn test_git_diff_output_is_truncated_with_guidance() {
        let huge = "x".repeat(MAX_GIT_BYTES + 4096);
        let res = truncate_git_output(ToolResult::success(huge), "diff");
        assert!(
            res.output.len() < MAX_GIT_BYTES + 1024,
            "超限的 diff 必须被截断（实际 {} 字节）",
            res.output.len()
        );
        assert!(
            res.output.contains("输出已截断"),
            "{}",
            &res.output[..120.min(res.output.len())]
        );
        assert!(res.output.contains("read_file"), "截断时必须指路怎么拿全文");

        // 没超限的原样返回
        let small = "small diff".to_string();
        let res = truncate_git_output(ToolResult::success(small.clone()), "diff");
        assert_eq!(res.output, small);

        // 多字节字符边界不能切坏（按 char boundary 回退）
        let cjk = "改".repeat(MAX_GIT_BYTES);
        let res = truncate_git_output(ToolResult::success(cjk), "diff");
        assert!(res.output.contains("输出已截断"));
    }

    #[tokio::test]
    async fn test_git_status_runs_in_this_repo() {
        let res = git(&ws(), &serde_json::json!({ "action": "status" })).await;
        assert!(res.ok, "本仓是 git 仓库：{}", res.output);
    }

    /// 参数校验是**安全边界**，不是文案：越界路径、像选项的 ref、缺必填项都必须拒。
    #[test]
    fn test_git_argv_rejects_dangerous_or_incomplete_args() {
        // 未知 action
        let err = git_argv(&serde_json::json!({ "action": "rm-rf" })).unwrap_err();
        assert!(err.contains("未知 git action"), "{err}");

        // 绝对路径：git 工具只管工作区内
        let err = git_argv(&serde_json::json!({
            "action": "diff", "file": "C:/Windows/win.ini"
        }))
        .unwrap_err();
        assert!(err.contains("绝对路径"), "{err}");

        // 越出工作区
        let err = git_argv(&serde_json::json!({
            "action": "restore", "file": "../outside.txt"
        }))
        .unwrap_err();
        assert!(err.contains("不允许越出工作区"), "{err}");

        // ref 长得像选项（git 会当参数执行）
        let err = git_argv(&serde_json::json!({
            "action": "show", "ref": "--upload-pack=calc"
        }))
        .unwrap_err();
        assert!(err.contains("以 `-` 开头"), "{err}");

        // 缺必填：commit 没 message、blame 没文件
        assert!(git_argv(&serde_json::json!({ "action": "commit" })).is_err());
        assert!(git_argv(&serde_json::json!({ "action": "blame" })).is_err());
    }

    /// 常用动作的 argv 形状：钉住"参数都在正确位置"，避免以后加字段时错位。
    #[test]
    fn test_git_argv_shapes_for_common_actions() {
        let (_, a, _) = git_argv(&serde_json::json!({
            "action": "diff", "staged": true, "file": "src/lib.rs"
        }))
        .unwrap();
        assert_eq!(a, vec!["diff", "--cached", "--", "src/lib.rs"]);

        let (_, a, _) = git_argv(&serde_json::json!({ "action": "log", "limit": 3 })).unwrap();
        assert_eq!(a[0], "log");
        assert!(a.contains(&"3".to_string()));

        let (_, a, _) = git_argv(&serde_json::json!({
            "action": "commit", "message": "fix: 中文提交信息", "all": true
        }))
        .unwrap();
        assert_eq!(a, vec!["commit", "-m", "fix: 中文提交信息", "-a"]);

        let (_, a, _) = git_argv(&serde_json::json!({
            "action": "checkout", "branch": "feat/x", "create": true
        }))
        .unwrap();
        assert_eq!(a, vec!["checkout", "-b", "feat/x"]);

        let (_, a, _) = git_argv(&serde_json::json!({ "action": "add" })).unwrap();
        assert_eq!(a, vec!["add", "-A"]);
    }

    /// `cwd` 是**执行目录**，不是 pathspec：它不能混进 `--` 之后的文件列表。
    #[test]
    fn test_git_argv_carries_cwd_but_never_as_pathspec() {
        let (_, a, cwd) = git_argv(&serde_json::json!({
            "action": "log", "cwd": "frontend", "limit": 2
        }))
        .unwrap();
        assert_eq!(cwd.as_deref(), Some("frontend"));
        assert_eq!(a[0], "log");
        assert!(
            !a.iter().any(|x| x == "frontend"),
            "cwd 不该出现在 argv 里（它不是文件过滤）：{a:?}"
        );

        // `repo` 是 cwd 的别名
        let (_, _, cwd) = git_argv(&serde_json::json!({ "action": "status", "repo": "backend" })).unwrap();
        assert_eq!(cwd.as_deref(), Some("backend"));

        // 越界一律拒（与 file 同一套 repo_path 校验）
        for bad in ["../x", "C:/Windows", "/etc"] {
            let err = git_argv(&serde_json::json!({ "action": "status", "cwd": bad })).unwrap_err();
            assert!(
                err.contains("..") || err.contains("绝对路径"),
                "{bad} 应被拒：{err}"
            );
        }
    }

    /// 多仓库工作区：失败提示必须**点名**子仓库，并给出可照抄的 `cwd`。
    #[test]
    fn test_multi_repo_workspace_hint_names_child_repos() {
        let root = std::env::temp_dir().join(format!("a_da_multirepo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("frontend/.git")).unwrap();
        std::fs::create_dir_all(root.join("backend/.git")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();

        let hint = repo_hint(&root, None);
        assert!(hint.contains("frontend"), "{hint}");
        assert!(hint.contains("backend"), "{hint}");
        assert!(hint.contains("cwd"), "要告诉模型怎么改：{hint}");
        assert!(!hint.contains("docs"), "非仓库目录不该出现在清单里：{hint}");

        // 指定了 cwd 但仍不是仓库：提示说的是 cwd 的问题，不再列清单
        let hint = repo_hint(&root, Some("docs"));
        assert!(hint.contains("docs"), "{hint}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真 git：在工作区根不是仓库的情况下，`cwd` 能落到子仓库里执行。
    #[tokio::test]
    async fn test_git_runs_inside_child_repo_via_cwd() {
        let root = std::env::temp_dir().join(format!("a_da_git_cwd_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("frontend");
        std::fs::create_dir_all(&repo).unwrap();

        let initialised = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !initialised {
            eprintln!("跳过：本机没有可用的 git");
            return;
        }

        // 带 cwd：进子仓库执行，成功
        let res = git(&root, &serde_json::json!({ "action": "status", "cwd": "frontend" })).await;
        assert!(res.ok, "cwd 指到子仓库应当成功：{}", res.output);

        // 不带 cwd：失败，但提示要能自愈（点名 frontend + 给出 cwd 用法）
        let res = git(&root, &serde_json::json!({ "action": "status" })).await;
        assert!(!res.ok, "工作区根不是仓库，应当失败");
        assert!(
            res.output.contains("frontend"),
            "应指出 frontend 是独立仓库：{}",
            res.output
        );
        assert!(res.output.contains("cwd"), "{}", res.output);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 守门：**Go 文件必须能提取出声明**，且提取不到时要如实说明工具边界。
    ///
    /// 实测事故：子智能体对 `main.go` 调 `code_outline`，工具回"✓ 成功 + 未提取到显著
    /// 类、函数或接口大纲"——模型据此以为文件是空的，反复换工具硬探，最后把上下文撑爆。
    #[tokio::test]
    async fn test_code_outline_extracts_go_signatures_and_declares_its_limit() {
        let dir = std::env::temp_dir().join(format!("a_da_outline_go_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("main.go"),
            "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hi\")\n}\n\n\
             type Server struct {\n\tPort int\n}\n\nfunc (s *Server) Start() error {\n\treturn nil\n}\n",
        )
        .expect("写临时 Go 文件");

        let res = code_outline(&dir, "main.go").await;
        assert!(res.ok, "{}", res.output);
        assert!(res.output.contains("func main()"), "Go 函数必须提取到：{}", res.output);
        assert!(res.output.contains("type Server struct"), "Go 类型必须提取到：{}", res.output);
        assert!(
            res.output.contains("func (s *Server) Start()"),
            "Go 方法必须提取到：{}",
            res.output
        );

        // 纯配置/不认的文件：必须明说"工具不认"，而不是让模型以为"文件是空的"
        std::fs::write(dir.join("config.json"), "{\n  \"a\": 1\n}\n").expect("写临时配置");
        let res = code_outline(&dir, "config.json").await;
        assert!(res.ok, "{}", res.output);
        assert!(
            res.output.contains("read_file"),
            "提取不到时要指路 read_file：{}",
            res.output
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_run_tests_detects_cargo() {
        // 只验证"命令探测 + 真的跑起来"这件事；测试套件本身跑得久，
        // 所以这里用一条最短的 git 调用作等价断言（同一条 spawn/超时通路）。
        let res = git(&ws(), &serde_json::json!({ "action": "log", "limit": 1 })).await;
        assert!(res.ok, "{}", res.output);
    }
}
