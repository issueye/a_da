//! **子进程档**脚本宿主（P4 · SP-70）：`ScriptHost` 的第二个实现。
//!
//! # 它解决什么
//!
//! 进程内档（Boa）**没有内存护栏**：Boa 没有堆上限 API，宿主的墙钟超时也拦不住同步段里的
//! 内存暴涨。插件把内存吃光，死的是**整个 agent 进程**。这个档把插件挪到一个独立进程里：
//!
//! - **崩溃隔离**：插件把自己跑死，宿主照常在，下一次调用重新起一个子进程；
//! - **超时即杀**：墙钟到点就 `kill`（`kill_on_drop`），不需要协议里再设计"中止"消息；
//! - **能力按声明**（SP-71）：`needs` 在**子进程启动时**就定死，未声明即不注入。
//!
//! # 它**不**解决什么（必须说清）
//!
//! **它不做内存限额**。给子进程设 RSS 上限要看平台（Windows Job Object / Linux rlimit），
//! 本档没有做。所以"内存暴涨的插件"表现为：宿主在超时到点时**把它杀掉**并如实回报
//! ——进程死掉这件事被隔离住了，但"它在死之前能把机器内存吃多少"没有被限制。
//! 这条限制写在 `docs/quality/unfinished-features.md`。
//!
//! # 代价
//!
//! **每次调用一次进程启动**（数十毫秒级）。这是**非默认档**：默认仍是进程内档，
//! 由产品/宿主的显式选择启用。

use std::path::{Path, PathBuf};

use std::time::Instant;

use agent_base::domain::{DiagOutcome, Origin};
use agent_base::ports::{
    BoxFuture, InvokeRequest, LoadRequest, ModuleHandle, ScriptCtx, ScriptError, ScriptHookDeclaration,
    ScriptHookOutcome, ScriptHost, ScriptNeeds, ScriptOutcome, ScriptSkillDeclaration, ScriptSource,
    ScriptToolDeclaration,
};

use crate::scripts::registry::{LoadedScript, ScriptRegistry};
use crate::scripts::telemetry::ScriptTelemetry;
use crate::scripts::DiagOutlet;

/// 子进程档的脚本宿主。
pub struct SubprocessScriptHost {
    runner: PathBuf,
    registry: ScriptRegistry,
    diag: Option<DiagOutlet>,
}

impl SubprocessScriptHost {
    /// 按**可执行文件位置**解析子进程入口。
    ///
    /// 解析不到就**大声失败**（`ScriptError::Unavailable`）而不是静默退回进程内档：
    /// "以为在隔离里跑、其实在宿主进程里跑"是最坏的一种误解。
    pub fn new() -> Result<Self, ScriptError> {
        let runner = resolve_runner_path().ok_or_else(|| ScriptError::Unavailable {
            message: format!(
                "找不到插件子进程入口 `runner.exe`：可执行文件同目录下没有它，\
                 也没有设置 A_DA_PLUGIN_RUNNER。子进程档需要它——不静默退回进程内档。\
                 （当前可执行文件：{}）",
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "<未知>".to_string())
            ),
        })?;
        Ok(Self::with_runner(runner))
    }

    /// 显式给出入口路径（测试与打包宿主用）。
    pub fn with_runner(runner: impl Into<PathBuf>) -> Self {
        Self {
            runner: runner.into(),
            registry: ScriptRegistry::new(),
            diag: None,
        }
    }

    /// 接上遥测出口（与进程内档同一套出口类型，见 `scripts::telemetry`）。
    pub fn with_diag(mut self, outlet: DiagOutlet) -> Self {
        self.diag = Some(outlet);
        self
    }

    /// 入口路径（自检/日志用）。
    pub fn runner_path(&self) -> &Path {
        &self.runner
    }

    fn lookup(&self, handle: &ModuleHandle) -> Result<LoadedScript, ScriptError> {
        self.registry.get(handle)
    }

    /// 跑一次请求：把请求写进子进程 stdin、读回一行结果，**超时即杀**。
    async fn round_trip(
        &self,
        request: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, ScriptError> {
        let mut cmd = tokio::process::Command::new(&self.runner);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 🔴 **进程退出即回收**：超时分支里宿主直接不再等它，drop 这个 Child 就把它杀掉。
            // 没有这一条，"超时"只意味着宿主不等了，插件进程还在后台吃内存。
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| ScriptError::Unavailable {
            message: format!("启动插件子进程失败（{}）：{e}", self.runner.display()),
        })?;

        let payload = request.to_string();
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(|e| ScriptError::Unavailable {
                    message: format!("向插件子进程写请求失败：{e}"),
                })?;
            // 关闭 stdin（drop）：子进程靠"读到 EOF"知道请求写完了
        }

        let wait = child.wait_with_output();
        let output = match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait)
            .await
        {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(ScriptError::Unavailable {
                    message: format!("等待插件子进程失败：{e}"),
                })
            }
            Err(_) => {
                // 超时：`wait_with_output` 的 future 已被丢弃，`kill_on_drop` 会杀掉子进程
                return Err(ScriptError::TimedOut { limit_ms: timeout_ms });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // 子进程**自己崩了**（OOM / 被平台杀掉）也走这里：如实说清"它没产出结果"，
            // 而不是编一个"脚本返回空"。
            return Err(ScriptError::Script {
                message: format!(
                    "插件子进程没有产出结果（退出码 {:?}）：{}",
                    output.status.code(),
                    stderr.trim().chars().take(400).collect::<String>()
                ),
            });
        }
        serde_json::from_str(line).map_err(|e| ScriptError::Script {
            message: format!("插件子进程的响应不是合法 JSON（{e}）：{line}"),
        })
    }

    fn needs_json(needs: ScriptNeeds) -> serde_json::Value {
        serde_json::json!({
            "fs_read": needs.fs_read,
            "fs_write": needs.fs_write,
            "net": needs.net,
            "sqlite": needs.sqlite,
            "exec": needs.exec,
        })
    }
}

/// 解析子进程入口：环境变量优先，其次**可执行文件同目录**。
///
/// 同目录那一条同时覆盖两种形态：发布包（`dist/` 里并排）与 Cargo 测试
/// （`target/debug/deps/xxx.exe` 的上一级是 `target/debug/`）。
pub fn resolve_runner_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("A_DA_PLUGIN_RUNNER") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "runner.exe"
    } else {
        "runner"
    };
    // ① 同目录（发布形态 / `target/debug`）
    let same_dir = dir.join(name);
    if same_dir.is_file() {
        return Some(same_dir);
    }
    // ② 测试形态：`target/debug/deps/test-xxx.exe` → 上一级
    if let Some(parent) = dir.parent() {
        let sibling = parent.join(name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    None
}

impl ScriptHost for SubprocessScriptHost {
    fn load<'a>(
        &'a self,
        req: &'a LoadRequest,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ModuleHandle, ScriptError>> {
        Box::pin(async move {
            if ctx.cancel.is_cancelled() {
                return Err(ScriptError::Aborted);
            }
            let (path, workspace) = match &req.source {
                ScriptSource::Path(p) => {
                    let ws = req
                        .workspace
                        .clone()
                        .or_else(|| p.parent().map(|d| d.to_path_buf()))
                        .ok_or_else(|| ScriptError::Load {
                            message: format!("脚本 `{}` 既没有工作区也没有父目录", p.display()),
                        })?;
                    (p.clone(), ws)
                }
                // 与进程内档同一口径：沙箱按**路径**装载（要做路径沙箱判定），
                // 内联源码如实拒绝，而不是偷偷写临时文件凑合。
                ScriptSource::Inline { .. } => {
                    return Err(ScriptError::Load {
                        message: "内联脚本暂不支持：子进程档同样按路径装载。请给 `ScriptSource::Path`"
                            .to_string(),
                    })
                }
            };

            // 能力来自源码首部的声明（SP-71），与进程内档**同一个解析器**。
            let source = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ScriptError::Load {
                    message: format!("读取脚本失败（{}）：{e}", path.display()),
                })?;
            let declared = crate::scripts::needs::parse_needs(&source);

            let resp = self
                .round_trip(
                    serde_json::json!({
                        "kind": "inspect",
                        "path": path.to_string_lossy(),
                        "workspace": workspace.to_string_lossy(),
                        "needs": Self::needs_json(declared.needs),
                        "timeoutMs": ctx.budget.timeout_ms,
                        "loopIterations": ctx.budget.loop_iterations,
                    }),
                    ctx.budget.timeout_ms,
                )
                .await
                .map_err(|e| ScriptError::Load {
                    message: format!("装载脚本失败（{}）：{e:?}", path.display()),
                })?;

            if resp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                return Err(ScriptError::Load {
                    message: format!(
                        "装载脚本失败（{}）：{}",
                        path.display(),
                        resp.get("error").and_then(|v| v.as_str()).unwrap_or("未知原因")
                    ),
                });
            }

            let tools: Vec<crate::plugins::PluginToolDeclaration> = serde_json::from_value(
                resp.get("tools").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            let tools: Vec<ScriptToolDeclaration> = tools
                .into_iter()
                .map(|tool| ScriptToolDeclaration {
                    name: tool.name,
                    label: tool.label,
                    description: tool.description,
                    parameters: tool.parameters,
                })
                .collect();
            let prompts: Vec<crate::plugins::PluginPromptDeclaration> = serde_json::from_value(
                resp.get("prompts").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            let hooks: Vec<crate::plugins::PluginHookDeclaration> = serde_json::from_value(
                resp.get("hooks").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            // 技能不再来自脚本声明（2026-10-19）：目录约定 `skills/<name>/SKILL.md`
            // 由装配层扫描；子进程只负责工具/提示词/钩子。
            let resolved_skills: Vec<agent_base::ports::ScriptSkillDeclaration> = Vec::new();

            self.registry.insert(LoadedScript {
                path,
                workspace,
                exports: tools.iter().map(|t| t.name.clone()).collect(),
                tools,
                prompts: prompts
                    .into_iter()
                    .map(|p| agent_base::ports::ScriptPromptFragment {
                        id: p.id,
                        content: p.content,
                    })
                    .collect(),
                skills: resolved_skills,
                hooks: hooks
                    .into_iter()
                    .filter(|h| !h.point.trim().is_empty())
                    .map(|h| ScriptHookDeclaration {
                        point: h.point,
                        actions: h.actions,
                        order: h.order,
                    })
                    .collect(),
                needs: declared.needs,
            })
        })
    }

    fn exports(&self, handle: &ModuleHandle) -> Result<Vec<String>, ScriptError> {
        Ok(self.lookup(handle)?.exports)
    }

    fn tool_declarations(&self, handle: &ModuleHandle) -> Result<Vec<ScriptToolDeclaration>, ScriptError> {
        Ok(self.lookup(handle)?.tools)
    }

    fn prompt_fragments(
        &self,
        handle: &ModuleHandle,
    ) -> Result<Vec<agent_base::ports::ScriptPromptFragment>, ScriptError> {
        Ok(self.lookup(handle)?.prompts)
    }

    fn skills(&self, handle: &ModuleHandle) -> Result<Vec<ScriptSkillDeclaration>, ScriptError> {
        Ok(self.lookup(handle)?.skills)
    }

    fn hooks(&self, handle: &ModuleHandle) -> Result<Vec<ScriptHookDeclaration>, ScriptError> {
        Ok(self.lookup(handle)?.hooks)
    }

    fn invoke<'a>(
        &'a self,
        handle: &'a ModuleHandle,
        call: &'a InvokeRequest,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ScriptOutcome, ScriptError>> {
        Box::pin(async move {
            if ctx.cancel.is_cancelled() {
                return Err(ScriptError::Aborted);
            }
            let loaded = self.lookup(handle)?;
            if !loaded.exports.iter().any(|e| e == &call.export) {
                // 与进程内档同一语义：入口不存在 = **调用方写错了名字**，不该执行脚本
                return Err(ScriptError::UnknownExport {
                    export: call.export.clone(),
                });
            }

            let origin = Origin::Plugin {
                id: ctx.origin_id.unwrap_or("<unknown>").to_string(),
            };
            let telemetry = ScriptTelemetry::new(self.diag.clone(), origin, &call.export);
            telemetry.entered();
            let started = Instant::now();

            let result = self
                .round_trip(
                    serde_json::json!({
                        "kind": "call",
                        "path": loaded.path.to_string_lossy(),
                        "workspace": loaded.workspace.to_string_lossy(),
                        "export": call.export,
                        "args": call.args,
                        "needs": Self::needs_json(loaded.needs),
                        "timeoutMs": ctx.budget.timeout_ms,
                        "loopIterations": ctx.budget.loop_iterations,
                    }),
                    ctx.budget.timeout_ms,
                )
                .await;
            let elapsed_ms = started.elapsed().as_millis() as u64;

            let outcome = match result {
                Ok(resp) if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                    let tool_result: agent_toolkit::ToolResult = serde_json::from_value(
                        resp.get("result").cloned().unwrap_or(serde_json::json!({})),
                    )
                    .map_err(|e| ScriptError::Script {
                        message: format!("子进程返回的工具结果无法解析：{e}"),
                    })?;
                    // 与进程内档同一口径：输出按预算截断并**如实标注**
                    let (output, truncated) =
                        crate::scripts::cap_output(tool_result.output, ctx.budget.max_output_bytes);
                    Ok(ScriptOutcome {
                        ok: tool_result.ok,
                        output,
                        data: tool_result.details,
                        duration_ms: elapsed_ms,
                        truncated,
                    })
                }
                Ok(resp) => Err(ScriptError::Script {
                    message: resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("子进程未给出失败原因")
                        .to_string(),
                }),
                Err(e) => Err(e),
            };

            telemetry.finished(
                match &outcome {
                    Ok(_) => DiagOutcome::Passed,
                    Err(ScriptError::TimedOut { .. }) | Err(ScriptError::BudgetExceeded { .. }) => {
                        DiagOutcome::TimedOut
                    }
                    Err(ScriptError::Aborted) => DiagOutcome::Aborted,
                    Err(_) => DiagOutcome::Failed,
                },
                elapsed_ms,
                None,
            );
            outcome
        })
    }

    fn invoke_hook<'a>(
        &'a self,
        handle: &'a ModuleHandle,
        point: &'a str,
        payload: serde_json::Value,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ScriptHookOutcome, ScriptError>> {
        Box::pin(async move {
            if ctx.cancel.is_cancelled() {
                return Err(ScriptError::Aborted);
            }
            let loaded = self.lookup(handle)?;
            if !loaded.hooks.iter().any(|h| h.point == point) {
                return Err(ScriptError::UnknownExport {
                    export: point.to_string(),
                });
            }
            // 钩子的"一进一出"由 `HookRuntime` 报（见进程内档同处的说明），
            // 这里**刻意不重复**发 script.* 埋点。
            let resp = self
                .round_trip(
                    serde_json::json!({
                        "kind": "hook",
                        "path": loaded.path.to_string_lossy(),
                        "workspace": loaded.workspace.to_string_lossy(),
                        "point": point,
                        "payload": payload,
                        "needs": Self::needs_json(loaded.needs),
                        "timeoutMs": ctx.budget.timeout_ms,
                        "loopIterations": ctx.budget.loop_iterations,
                    }),
                    ctx.budget.timeout_ms,
                )
                .await?;

            if resp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                return Err(ScriptError::Script {
                    message: resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("子进程未给出失败原因")
                        .to_string(),
                });
            }
            let raw = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
            if let Some(err) = raw.get("error").and_then(|v| v.as_str()) {
                return Err(ScriptError::Script {
                    message: err.to_string(),
                });
            }
            Ok(ScriptHookOutcome {
                action: raw.get("action").and_then(|v| v.as_str()).map(String::from),
                reason: raw.get("reason").and_then(|v| v.as_str()).map(String::from),
                note: raw.get("note").and_then(|v| v.as_str()).map(String::from),
                data: raw.get("data").cloned(),
            })
        })
    }

    fn dispose<'a>(&'a self, handle: ModuleHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // 幂等（同进程内档）：不在表里也算成功
            self.registry.remove(&handle);
        })
    }
}

/// 让宿主能在日志里自述"这个档位用的是哪个入口"。
impl std::fmt::Debug for SubprocessScriptHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubprocessScriptHost")
            .field("runner", &self.runner)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 找不到入口时**大声失败**，而不是静默退回进程内档：
    /// "以为在隔离里跑、其实在宿主进程里跑"是最坏的一种误解。
    #[test]
    fn test_missing_runner_is_reported_loudly() {
        let host = SubprocessScriptHost::with_runner("/definitely/not/here/runner");
        let ctx = ScriptCtx {
            budget: agent_base::ports::ScriptBudget::default(),
            cancel: &agent_base::testing::NeverCancel,
            thread_id: "t1",
            origin_id: Some("plugin:x"),
        };
        let err = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(host.load(
                &LoadRequest {
                    source: ScriptSource::Path(PathBuf::from("/tmp/p.ts")),
                    workspace: Some(PathBuf::from("/tmp")),
                },
                &ctx,
            ))
            .expect_err("入口不存在必须报错");
        assert!(
            format!("{err:?}").contains("启动插件子进程失败")
                || format!("{err:?}").contains("读取脚本失败"),
            "{err:?}"
        );
    }

    /// 内联源码同样如实拒绝（与进程内档同一口径）。
    #[test]
    fn test_inline_source_is_rejected() {
        let host = SubprocessScriptHost::with_runner("/x");
        let ctx = ScriptCtx {
            budget: agent_base::ports::ScriptBudget::default(),
            cancel: &agent_base::testing::NeverCancel,
            thread_id: "t1",
            origin_id: None,
        };
        let mut req = LoadRequest {
            source: ScriptSource::Inline {
                filename: Some("x.ts".into()),
                source: "export default {}".into(),
            },
            workspace: None,
        };
        req.workspace = None;
        let err = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(host.load(&req, &ctx))
            .expect_err("内联源码必须被拒");
        assert!(matches!(err, ScriptError::Load { .. }), "{err:?}");
    }
}
