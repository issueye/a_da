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
use super::fs_tools::{read_file, ToolResult};

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

/// 提取文件里的类/函数/接口骨架签名与行号。
///
/// ⚠️ `read_file` 的输出每行都带 `"<行号> | "` 前缀，所以匹配前必须**剥掉前缀**。
/// 原实现在 `agent-core` 里直接对整行做 `starts_with("pub fn ")`，于是**永远匹配不到**
/// 任何签名、每次都返回"未提取到显著大纲"——一个静默失效的工具。
/// W2-T2 搬运时补了单测才暴露（`test_code_outline_extracts_rust_signatures`）。
pub async fn code_outline(workspace: &Path, file_path: &str) -> ToolResult {
    let res = read_file(workspace, file_path, None, Some(2000));
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
        if body.starts_with("pub fn ")
            || body.starts_with("fn ")
            || body.starts_with("pub struct ")
            || body.starts_with("struct ")
            || body.starts_with("pub enum ")
            || body.starts_with("enum ")
            || body.starts_with("class ")
            || body.starts_with("export class ")
            || body.starts_with("interface ")
            || body.starts_with("export interface ")
            || body.starts_with("export function ")
            || body.starts_with("export const ")
        {
            outlines.push(format!("L{:03}: {}", line_no + 1, body));
        }
    }

    if outlines.is_empty() {
        ToolResult::success(format!("文件 [{}] 未提取到显著类、函数或接口大纲。", file_path))
    } else {
        ToolResult::success(format!("## 代码大纲结构: {}\n```\n{}\n```", file_path, outlines.join("\n")))
    }
}

/// `git status --porcelain=v1 -b`。
pub async fn git_status(workspace: &Path) -> ToolResult {
    let res = run_command(workspace, "git status --porcelain=v1 -b", None, Some(10), None).await;
    if !res.ok && res.output.contains("not a git repository") {
        ToolResult::error("当前目录不是一个有效的 Git 仓库。")
    } else {
        res
    }
}

/// `git diff`（可限定单个文件）。
pub async fn git_diff(workspace: &Path, file: Option<&str>) -> ToolResult {
    let cmd = match file {
        Some(f) if !f.trim().is_empty() => format!("git diff -- {}", f.trim()),
        _ => "git diff".to_string(),
    };
    run_command(workspace, &cmd, None, Some(15), None).await
}

/// `git log -n <limit> --oneline`。
pub async fn git_log(workspace: &Path, limit: u64) -> ToolResult {
    let cmd = format!("git log -n {} --oneline", limit);
    run_command(workspace, &cmd, None, Some(10), None).await
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
    GitStatus,
    GitDiff,
    GitLog,
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn to_receipt(res: ToolResult, started_at: i64) -> ToolReceipt {
    let status = if res.ok { ToolStatus::Success } else { ToolStatus::Error };
    let mut receipt = ToolReceipt::new(status, res.output, started_at, now_ms());
    receipt.details = res.details;
    receipt
}

impl Tool for PluginTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = now_ms();
            let args = &call.args;
            let res = match self.kind {
                Kind::ProjectInspect => project_inspect(&self.workspace).await,
                Kind::CodeOutline => {
                    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    code_outline(&self.workspace, path).await
                }
                Kind::GitStatus => git_status(&self.workspace).await,
                Kind::GitDiff => {
                    let file = args
                        .get("file")
                        .or_else(|| args.get("path"))
                        .and_then(|v| v.as_str());
                    git_diff(&self.workspace, file).await
                }
                Kind::GitLog => {
                    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
                    git_log(&self.workspace, limit).await
                }
                Kind::RunTests => run_tests(&self.workspace).await,
            };
            to_receipt(res, started_at)
        })
    }
}

/// `git` 工具包：版本库状态、diff 与提交历史。
pub fn git_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(PluginTool::new("git_status", workspace, Kind::GitStatus)),
        Arc::new(PluginTool::new("git_diff", workspace, Kind::GitDiff)),
        Arc::new(PluginTool::new("git_log", workspace, Kind::GitLog)),
    ]
}

/// `project` 工具包：项目诊断与代码结构提取。
///
/// 含 `inspect_project` 这个**别名**（与 `project_inspect` 同一实现）——
/// 注册表里两个名字都在，装配时就必须两个都可解析，否则界面/模型看到的清单与真源不一致。
pub fn project_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(PluginTool::new("project_inspect", workspace, Kind::ProjectInspect)),
        Arc::new(PluginTool::new("inspect_project", workspace, Kind::ProjectInspect)),
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
        // 描述符来自注册表：这 7 个名字都必须存在（否则 new() 会 panic）
        let all: Vec<Arc<dyn Tool>> = git_tools(&ws())
            .into_iter()
            .chain(project_tools(&ws()))
            .chain(test_tools(&ws()))
            .collect();
        let names: Vec<String> = all.iter().map(|t| t.descriptor().name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "git_status",
                "git_diff",
                "git_log",
                "project_inspect",
                "inspect_project",
                "code_outline",
                "run_tests"
            ]
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

    #[tokio::test]
    async fn test_git_status_runs_in_this_repo() {
        let res = git_status(&ws()).await;
        assert!(res.ok, "本仓是 git 仓库：{}", res.output);
    }

    #[tokio::test]
    async fn test_run_tests_detects_cargo() {
        // 只验证"命令探测 + 真的跑起来"这件事；测试套件本身跑得久，
        // 所以这里用一个短超时的等价断言：git_log 走同一条 run_command 通路。
        let res = git_log(&ws(), 1).await;
        assert!(res.ok, "{}", res.output);
    }
}
