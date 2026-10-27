//! 内置**插件工具**的 `Tool` 实现（W2-T2）。
//!
//! 背景：`project_inspect` / `code_outline` / `run_tests` 这些工具原先只存在于
//! `agent-core` 的 `execute_builtin_plugin_tool` 字符串分派里——它们是"插件工具"，
//! 却不是一等 `Tool`，因此**进不了 catalog**、也没有 `ToolDescriptor` 可被各消费侧读取。
//!
//! 本文件把它们的**逻辑**搬成自由函数（唯一真源），再由薄薄的 [`PluginTool`] 包成 `Tool`：
//! - 自由函数：给 legacy 分派复用（`agent-core` 只做委派，不复制逻辑，R2）；
//! - `Tool` 实现：进 catalog，供 `agent.spec.json` 的 `toolkits` 声明装配。
//!
//! **描述符来自注册表**（`registry::find_tool_descriptor`），不在这里另写一份 schema——
//! 那是第 7 张名单的开始。
//!
//! 2026-10-27：`git` 已搬出基座，改由 `plugins/git-tools`（TS 插件）提供——
//! 基座只补 ts 引擎能力，不再自带 git 实现。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use super::cmd_tools::run_command;
use super::fs_tools::{read_file, ToolPaths, ToolResult};

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
    let res = read_file(ToolPaths::new(workspace), file_path, None, Some(2000));
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
                Kind::RunTests => run_tests(&self.workspace).await,
            };
            to_receipt(res, started_at)
        })
    }
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
        // 描述符来自注册表：这 3 个名字都必须存在（否则 new() 会 panic）。
        // 2026-10-27：`git` 已搬到 `plugins/git-tools`（插件自述描述符），
        // 基座不再提供 git 工具——这里自然也不再有它。
        let all: Vec<Arc<dyn Tool>> = project_tools(&ws())
            .into_iter()
            .chain(test_tools(&ws()))
            .collect();
        let names: Vec<String> = all.iter().map(|t| t.descriptor().name.clone()).collect();
        assert_eq!(
            names,
            vec!["project_inspect", "code_outline", "run_tests"],
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

    /// 只验证"命令真的被 spawn 起来 + 输出回得来"这件事（同一条 spawn/超时通路）。
    ///
    /// 2026-10-27：原先借 `git` 工具做等价断言，它已搬到 `plugins/git-tools`；
    /// 改用 `run_command` 跑一条最短命令，测的仍是"子进程通路"，且与 git 解耦。
    #[tokio::test]
    async fn test_run_command_spawns_and_reports_output() {
        let res = run_command(&ws(), "echo ok", None, Some(30), None).await;
        assert!(res.ok, "{}", res.output);
        assert!(res.output.contains("ok"), "{}", res.output);
    }
}
