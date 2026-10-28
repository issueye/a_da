//! 脚本宿主的**生产实现**（P1 · SP-10 / SP-12）：基于 `ada-ts-engine`（进程内 Boa）。//!
//! # 这一层负责什么
//!
//! 1. 把 [`ScriptHost`] 端口的语义落到进程内沙箱上（装载 → 调用 → 释放）；
//! 2. **把"撞预算"与"脚本抛错"分开**：前者映射成 [`ScriptError::BudgetExceeded`] /
//!    [`ScriptError::TimedOut`]（调用方据此给模型一条"换条路"的回执），后者是脚本自己的错；
//! 3. 输出按预算截断，且**如实标注**（`truncated`），不静默切掉。
//!
//! # 刻意没做的事（留给后续任务，避免在这里替产品做决定）
//!
//! - **实例策略**（每调用新建 vs 每会话常驻）：当前"每次调用新建运行时"（设计文档 §3.3 的
//!   默认口径，等决策 2 拍板）；句柄只承载元信息，因此释放是幂等的。
//! - **预算 → 进程内指令上限的细分**：当前沿用 `ada-ts-engine::Limits::default()`
//!   （宽但有限，`while(true){}` 必然被打断）。按调用方预算逐次收紧属 SP-11（等决策 2）。
//! - **内联源码**：沙箱按路径装载（要读盘、要做路径沙箱判定），所以 `ScriptSource::Inline`
//!   在这里**如实拒绝**，而不是偷偷写临时文件凑合。

use std::time::{Duration, Instant};

pub mod assembly;
pub mod needs;
pub mod plugin_tool;
pub mod registry;
pub mod subprocess;
pub mod telemetry;
pub mod validate;
pub use assembly::{collect_plugin_tools, PluginToolAssembly, SkippedPluginTool};
pub use plugin_tool::{outcome_to_receipt, PluginTool};
pub use subprocess::{resolve_runner_path, SubprocessScriptHost};
pub use validate::{validate_plugin, ExportCheck, ValidateReport};

use agent_base::domain::{
    DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_SCRIPT_FINISHED, POINT_SCRIPT_INVOKED,
};
use agent_base::ports::{
    BoxFuture, CancelToken, InvokeRequest, LoadRequest, ModuleHandle, ScriptCtx, ScriptError,
    ScriptHookDeclaration, ScriptHookOutcome, ScriptHost, ScriptNeeds, ScriptOutcome,
    ScriptSkillDeclaration, ScriptSource, ScriptToolDeclaration,
};

use crate::plugins::PluginSandbox;
use crate::scripts::registry::{LoadedScript, ScriptRegistry};

/// 轮询取消信号；插件工具可能在沙箱里等很久（例如提问），必须能被用户中止。
async fn wait_cancel(cancel: &dyn CancelToken) {
    if cancel.is_cancelled() {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if cancel.is_cancelled() {
            return;
        }
    }
}

/// 宿主级的遥测出口。
///
/// 用 `Arc<dyn Fn>` 而不是借用：出口在构造时接一次，之后每次调用都要用；
/// 组合根（谁装配宿主谁接）通常自己也是个 `Arc` 里的东西。
pub type DiagOutlet = std::sync::Arc<dyn Fn(DiagnosticEvent) + Send + Sync>;

/// 按预算截断脚本输出，并**如实标注**。
///
/// 两种宿主实现（进程内 / 子进程）共用同一套截断口径（P4·SP-70）：
/// 各写一份的话，"换个档位输出到底截没截断"就会不一致。
pub(crate) fn cap_output(output: String, limit: u64) -> (String, bool) {
    let limit = limit as usize;
    if limit == 0 || output.len() <= limit {
        return (output, false);
    }
    // 按字符边界截，避免切出半个 UTF-8 字符
    let mut cut = limit;
    while cut > 0 && !output.is_char_boundary(cut) {
        cut -= 1;
    }
    (
        format!("{}…（输出超过 {} 字节，已截断）", &output[..cut], limit),
        true,
    )
}

/// 基于进程内 Boa 的脚本宿主。
#[derive(Default)]
pub struct TsScriptHost {
    registry: ScriptRegistry,
    /// 宿主级的遥测出口（构造时接，见 `ports::scripts` 的说明）。
    /// 没接时退回 `tracing`：宁可只进日志，也不要伪造事件信封。
    diag: Option<DiagOutlet>,
}

impl TsScriptHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// 接上遥测出口（组合根用；测试可以接一个收集器）。
    pub fn with_diag(mut self, outlet: DiagOutlet) -> Self {
        self.diag = Some(outlet);
        self
    }

    /// 报一条遥测：有出口就交出结构化诊断，没接就退回日志。
    ///
    /// **两条路都不伪造事件信封**——`seq` 与轮次上下文不归脚本宿主管。
    fn report_diag(&self, diag: DiagnosticEvent) {
        match &self.diag {
            Some(outlet) => outlet(diag),
            None => tracing::debug!(
                target: "a_da::diag",
                "[diag] script {} ← {:?} outcome={:?} {}ms",
                diag.point,
                diag.origin,
                diag.outcome,
                diag.duration_ms.unwrap_or(0)
            ),
        }
    }

    fn lookup(&self, handle: &ModuleHandle) -> Result<LoadedScript, ScriptError> {
        self.registry.get(handle)
    }
}

impl ScriptHost for TsScriptHost {
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
                ScriptSource::Inline { .. } => {
                    return Err(ScriptError::Load {
                        message: "内联脚本暂不支持：沙箱按路径装载（要做路径沙箱判定）。\
                                  请给 `ScriptSource::Path`"
                            .to_string(),
                    })
                }
            };

            // 装载期就把"读不到 / 转译不过"挡掉：别把语法错误拖到第一次调用时才炸。
            let source = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ScriptError::Load {
                    message: format!("读取脚本失败（{}）：{e}", path.display()),
                })?;
            ada_ts_engine::compiler::transpile_ts_module_cached(&source, path.to_str())
                .map_err(|e| ScriptError::Load {
                    message: format!("转译脚本失败（{}）：{e}", path.display()),
                })?;

            // P4·SP-71：**按声明注入能力**（唯一判据是源码首部的那条指令，见 `needs`）。
            // 解析放在这里而不是调用方传进来：装载期与调用期必须**同一套**能力，
            // 而两者都从这里取（`LoadedScript.needs`）——两处各解析一次迟早对不上。
            let declared = needs::parse_needs(&source);
            let engine_needs = to_engine_needs(declared.needs);

            // 装载期探明可用入口（沙箱的 inspect 会真的执行一遍插件装载流程，
            // 所以这一步同时验证了"这个模块能被装起来"）。
            let inspected = PluginSandbox::inspect_plugin_full(&path, &workspace, engine_needs)
                .await
                .map_err(|e| ScriptError::Load {
                    message: format!("装载脚本失败（{}）：{e}", path.display()),
                })?;
            let tools: Vec<ScriptToolDeclaration> = inspected
                .tools
                .into_iter()
                .map(|tool| ScriptToolDeclaration {
                    name: tool.name,
                    label: tool.label,
                    description: tool.description,
                    parameters: tool.parameters,
                    // 自述只读由沙箱归一化成 `"read"` / `"write"`，这里只做映射。
                    access: agent_base::ports::ScriptToolAccess::from_declared(Some(&tool.access)),
                    long_running: tool.long_running,
                })
                .collect();
            let exports: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
            let prompts: Vec<agent_base::ports::ScriptPromptFragment> = inspected
                .prompts
                .into_iter()
                .map(|p| agent_base::ports::ScriptPromptFragment {
                    id: p.id,
                    content: p.content,
                })
                .collect();
            // P2·SP-45：技能声明也挂在句柄上。**去掉空正文的那些**（沙箱已经滤过一遍，
            // 这里再滤一次是因为端口契约不该依赖某个实现的过滤行为）。
            // 技能不再来自脚本声明（2026-10-19）：Skill 是**目录 + SKILL.md**
            // （docs/SKILL_SPEC.md），由装配层扫 `skills/<name>/SKILL.md`。
            //
            // 🔴 脚本里的 `registerSkill` 现在只是被记录（`ignored_skills`）并告警：
            // 在这里"顺手兼容"会让技能有两个真源（脚本声明 vs 磁盘目录），
            // 而两者不一致时没人说得清哪个才算数。
            let skills: Vec<ScriptSkillDeclaration> = Vec::new();
            if !inspected.ignored_skills.is_empty() {
                tracing::warn!(
                    "[skills] 插件声明了 registerSkill（已移除的旧 API），这些技能被忽略：{}；\
                     请改为 skills/<name>/SKILL.md",
                    inspected.ignored_skills.join("、")
                );
            }
            // P3a·SP-50：事件注册（事件名 + 声明动作 + 顺序）。**没有 handler 的声明照收**：
            // 它会在调用期被如实报成"未实现 handler"（装配期过滤掉反而看不出问题）。
            let hooks: Vec<ScriptHookDeclaration> = inspected
                .events
                .into_iter()
                .filter(|h| !h.event.trim().is_empty())
                .map(|h| ScriptHookDeclaration {
                    point: h.event,
                    actions: h.actions,
                    order: h.order,
                })
                .collect();

            self.registry.insert(LoadedScript {
                path,
                workspace,
                exports,
                tools,
                prompts,
                skills,
                hooks,
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

            // 未声明这个点位 = **调用方弄错了**（装配期就该校验过），与"插件自己抛错"是两件事
            if !loaded.hooks.iter().any(|h| h.point == point) {
                return Err(ScriptError::UnknownExport {
                    export: point.to_string(),
                });
            }

            // 🔴 这里**刻意不发** `script.invoked/finished`：钩子的"一进一出"由
            // `HookRuntime` 以 `HookObservation` 报给引擎（那边才有事件信封与轮次上下文），
            // 这里再报一遍就是同一件事两套埋点——排查时没人知道哪条才准。
            let timeout_s = ctx.budget.timeout_ms.div_ceil(1000).max(1);
            let raw = PluginSandbox::call_hook_budgeted(
                &loaded.path,
                point,
                payload,
                &loaded.workspace,
                timeout_s,
                to_engine_needs(loaded.needs),
                ada_ts_engine::Limits::tight(ctx.budget.loop_iterations),
            )
            .await
            .map_err(|e| {
                let msg = e.to_string();
                match failure_kind(&msg) {
                    FailureKind::Limit(kind) => ScriptError::BudgetExceeded { kind },
                    FailureKind::Timeout => ScriptError::TimedOut {
                        limit_ms: ctx.budget.timeout_ms,
                    },
                    FailureKind::Other => ScriptError::Load { message: msg },
                }
            })?;

            // 沙箱把失败写成 `{"error": "..."}`（不是 Err）：那是**插件的问题**，
            // 归到 `Script`（调用方据失败安全把它当"这条意见作废"处理）
            if let Some(err) = raw.get("error").and_then(|v| v.as_str()) {
                return Err(match failure_kind(err) {
                    // 撞限额/超时是**可纠正**的，别混进"插件坏了"
                    FailureKind::Limit(kind) => ScriptError::BudgetExceeded { kind },
                    FailureKind::Timeout => ScriptError::TimedOut {
                        limit_ms: ctx.budget.timeout_ms,
                    },
                    FailureKind::Other => ScriptError::Script {
                        message: err.to_string(),
                    },
                });
            }

            Ok(ScriptHookOutcome {
                action: raw
                    .get("action")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                reason: raw
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                note: raw.get("note").and_then(|v| v.as_str()).map(|s| s.to_string()),
                data: raw.get("data").cloned(),
            })
        })
    }

    fn invoke<'a>(
        &'a self,
        handle: &'a ModuleHandle,
        call: &'a InvokeRequest,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ScriptOutcome, ScriptError>> {
        Box::pin(async move {
            // 取消要在**进沙箱之前**查一次：进了同步段就只能靠进程内限额兜底了
            if ctx.cancel.is_cancelled() {
                return Err(ScriptError::Aborted);
            }
            let loaded = self.lookup(handle)?;

            // P1·SP-11：端口把**诊断出口**带进来了，就真的用它——每次调用记一进一出。
            //
            // 出口给的是 `DiagnosticEvent` 而不是事件信封：`seq` 与轮次上下文在引擎手里，
            // 宿主伪造不出来（伪造的后果是"seq 严格递增"的账本里一堆假违约）。
            let origin = Origin::Plugin {
                id: ctx.origin_id.unwrap_or("<unknown>").to_string(),
            };
            let report = |point: &str, outcome: Option<DiagOutcome>, ms: Option<u64>, console: Option<serde_json::Value>| {
                let mut ev = DiagnosticEvent::new(DiagFamily::Script, point, origin.clone());
                if let Some(o) = outcome {
                    ev = ev.with_outcome(o);
                }
                if let Some(m) = ms {
                    ev = ev.with_duration_ms(m);
                }
                let mut data = serde_json::json!({ "export": call.export });
                if let Some(c) = console {
                    // P1·SP-14：控制台输出**只报元数据**（字节数 + 是否截断），
                    // 内容进日志——诊断缓冲要跨端传输，正文塞进去会把它刷爆（§2.2 第 3 条律）。
                    let bytes = c.get("stdoutBytes").and_then(|v| v.as_u64()).unwrap_or(0)
                        + c.get("stderrBytes").and_then(|v| v.as_u64()).unwrap_or(0);
                    if bytes > 0 {
                        ev = ev.with_bytes(bytes);
                    }
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("console".to_string(), c);
                    }
                }
                ev = ev.with_data(data);
                self.report_diag(ev);
            };
            report(POINT_SCRIPT_INVOKED, None, None, None);

            // 入口不存在 = **调用方写错了名字**，与"脚本自己抛错"是两件事：
            // 前者不该执行脚本（也不该等一个超时），后者才是脚本的问题。
            if !loaded.exports.iter().any(|e| e == &call.export) {
                return Err(ScriptError::UnknownExport {
                    export: call.export.clone(),
                });
            }

            // 墙钟预算 → 沙箱的超时（沙箱按秒计，向上取整到至少 1 秒）
            let timeout_s = ctx.budget.timeout_ms.div_ceil(1000).max(1);
            let started = Instant::now();

            let result = PluginSandbox::call_tool_budgeted(
                &loaded.path,
                &call.export,
                call.args.clone(),
                &loaded.workspace,
                Some(ctx.thread_id),
                timeout_s,
                to_engine_needs(loaded.needs),
                // SP-11：进程内限额来自**同一个预算**——墙钟兜"等待型"，
                // 指令上限兜"同步死循环"，两者不能各说各话。
                ada_ts_engine::Limits::tight(ctx.budget.loop_iterations),
            );
            tokio::pin!(result);
            let result = tokio::select! {
                result = &mut result => result,
                _ = wait_cancel(ctx.cancel) => return Err(ScriptError::Aborted),
            };

            let elapsed_ms = started.elapsed().as_millis() as u64;

            // 失败路径也要报"结束"——否则面板上只留下"开始"，看不出它怎么收场的
            let outcome: Result<ScriptOutcome, ScriptError> = match result {
                Ok(r) if r.ok => {
                    let (output, truncated) =
                        cap_output(r.output, ctx.budget.max_output_bytes);
                    Ok(ScriptOutcome {
                        ok: true,
                        output,
                        data: r.details,
                        duration_ms: elapsed_ms,
                        truncated,
                    })
                }
                // 沙箱返回 Err 的两类：装载/转译失败，或者（修好之后）撞限额/超时的直传
                Err(e) => {
                    let msg = e.to_string();
                    Err(match failure_kind(&msg) {
                        FailureKind::Limit(kind) => ScriptError::BudgetExceeded { kind },
                        FailureKind::Timeout => ScriptError::TimedOut {
                            limit_ms: ctx.budget.timeout_ms,
                        },
                        FailureKind::Other => ScriptError::Load { message: msg },
                    })
                }
                // 撞限额/超时在沙箱里是**回执**（`ok=false` + 说明），不是 Err——所以这里要分开判
                Ok(r) => Err(match failure_kind(&r.output) {
                    FailureKind::Limit(kind) => ScriptError::BudgetExceeded { kind },
                    FailureKind::Timeout => ScriptError::TimedOut {
                        limit_ms: ctx.budget.timeout_ms,
                    },
                    FailureKind::Other => ScriptError::Script { message: r.output },
                }),
            };

            // 捕获到的控制台输出随回执的 `data` 回来（SP-14）；日志里留一份正文供排查
            let console_info = match &outcome {
                Ok(o) => o.data.as_ref().and_then(|d| d.get("console")).cloned(),
                Err(_) => None,
            };
            if let Some(c) = &console_info {
                let out = c.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
                let err = c.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
                let truncated = c.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false);
                if !out.is_empty() || !err.is_empty() {
                    tracing::debug!(
                        target: "a_da::scripts",
                        "[{origin_label}] console stdout={out:?} stderr={err:?} truncated={truncated}",
                        origin_label = origin.label(),
                    );
                }
            }

            report(
                POINT_SCRIPT_FINISHED,
                Some(match &outcome {
                    Ok(_) => DiagOutcome::Passed,
                    Err(ScriptError::BudgetExceeded { .. }) | Err(ScriptError::TimedOut { .. }) => {
                        DiagOutcome::TimedOut
                    }
                    Err(ScriptError::Aborted) => DiagOutcome::Aborted,
                    Err(_) => DiagOutcome::Failed,
                }),
                Some(elapsed_ms),
                console_info,
            );
            outcome
        })
    }

    fn dispose<'a>(&'a self, handle: ModuleHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // 幂等：不在表里也算成功（调用方不必为了"放两次"写判断）
            self.registry.remove(&handle);
        })
    }
}

/// 端口层的抽象能力 → `ada-ts-engine` 的具体注入清单（**唯一映射点**，P4·SP-71）。
///
/// 端口不认识 `Needs`、引擎不认识 `ScriptNeeds`，转换只能有一处：
/// 各写一份迟早出现"端口说给了 fs、引擎那边没注入"这种对不上的账。
pub(crate) fn to_engine_needs(needs: ScriptNeeds) -> ada_ts_engine::Needs {
    // `@ada-needs` 只是**模块注入清单**，不再承担授权语义：
    // 声明了哪几项，沙箱里就注入哪几项；未声明的能力在本插件里不存在。
    ada_ts_engine::Needs {
        fs_read: needs.fs_read,
        fs_write: needs.fs_write,
        net: needs.net,
        sqlite: needs.sqlite,
        exec: needs.exec,
    }
}

/// 从带标记的错误串里取回"撞的是哪一项限额"（标记由 `ada-ts-engine` 打的）。
fn limit_kind_of(msg: &str) -> String {
    for kind in ["loop_iterations", "recursion_depth", "stack_size"] {
        if msg.contains(kind) {
            return kind.to_string();
        }
    }
    "unknown".to_string()
}

/// 沙箱失败串的三分：撞限额 / 墙钟超时 / 其它。
///
/// 为什么要分：前两类是**可纠正**的（映射成 `Timeout` 回执让模型换路子），
/// 第三类才是"这次的错"。沙箱目前只给字符串，所以这里做一次显式分类——
/// 等 SP-15 让沙箱返回结构化结果时，这段就该被删掉。
enum FailureKind {
    Limit(String),
    Timeout,
    Other,
}

fn failure_kind(msg: &str) -> FailureKind {
    if ada_ts_engine::is_limit_error(msg) {
        return FailureKind::Limit(limit_kind_of(msg));
    }
    // 沙箱自己的超时文案（`插件沙箱执行超时（超过 N 秒）`）；只在**装载/调用路径**里出现
    if msg.contains("沙箱执行超时") || (msg.contains("超过") && msg.contains("秒没有返回")) {
        return FailureKind::Timeout;
    }
    FailureKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::{DiagCollector, NeverCancel};

    fn write_plugin(body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("a_da_script_host_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join(format!(
            "p_{}.ts",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, body).expect("写临时插件失败");
        (dir, path)
    }

    fn ctx_budget(timeout_ms: u64) -> ScriptCtx<'static> {
        ScriptCtx {
            budget: agent_base::ports::ScriptBudget {
                timeout_ms,
                ..Default::default()
            },
            cancel: &NeverCancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        }
    }

    /// SP-19：把实现交给**通用端口契约**验一遍。
    ///
    /// 契约（`agent_conformance::verify_script_host_contract`）覆盖：装载 → 调用 → 释放往返、
    /// **未知入口**、**预算**（同步死循环必须秒级返回而不是挂住）、**取消**、释放幂等。
    ///
    /// 为什么交给契约而不是在这里重写一遍：P4 还会有"子进程档"这**第二个**实现，
    /// 通用断言写一次、两个实现都用它验；各写一遍的断言迟早会分叉，
    /// 而分叉的表现是"某个实现悄悄不满足契约却没人发现"。
    ///
    /// 契约里那条死循环探针同时守着接通时踩到的两个 `ada-ts-engine` bug
    /// （行级转译器补分号、`CallAsyncFunction` 吞错误）——它们的回归用例在 ts-engine 里。
    ///
    /// 下面几条留在这里的是**本实现的专有行为**（契约管不到的那些）。
    #[tokio::test]
    async fn test_passes_script_host_port_contract() {
        let probe = agent_conformance::write_script_probe().expect("造契约探针失败");
        let host = TsScriptHost::new();
        agent_conformance::verify_script_host_contract(&host, &probe)
            .await
            .expect("ScriptHost 端口契约必须全绿");
    }

    /// 装载期就把问题挡住：语法/转译过不去的脚本不该等到调用时才炸。
    #[tokio::test]
    async fn test_load_rejects_unparsable_script() {
        let (dir, path) = write_plugin("export default { tools: [ { name: 'x', } \n");
        let host = TsScriptHost::new();
                let ctx = ctx_budget(5_000);

        let res = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await;
        assert!(
            matches!(res, Err(ScriptError::Load { .. })),
            "转译失败的脚本必须报 Load，实际 {res:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SP-11：诊断出口必须**真的被用上**（一进一出各一条）——端口里带一个没人用的出口
    /// 就是"声明了没接线"，正是这一整套设计在防的东西。
    #[tokio::test]
    async fn test_script_diagnostics_are_reported() {
        use agent_base::domain::{DiagOutcome, Origin};

        let (dir, path) = write_plugin(
            "export default { tools: [ { name: 'echo', description: '回显', \
             execute: async (args: any) => ({ ok: true, output: 'x' + args.text }) } ] }\n",
        );
        // 遥测出口是**宿主级**的：构造时接一个收集器
        let diags = std::sync::Arc::new(DiagCollector::new());
        let collector = diags.clone();
        let host = TsScriptHost::new().with_diag(std::sync::Arc::new(move |d| {
            collector.push(d);
        }));
        let ctx = ctx_budget(20_000);

        let handle = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");
        host.invoke(
            &handle,
            &InvokeRequest {
                export: "echo".into(),
                args: serde_json::json!({ "text": "hi" }),
            },
            &ctx,
        )
        .await
        .expect("调用应当成功");

        let reported = diags.snapshot();
        let points: Vec<&str> = reported.iter().map(|d| d.point.as_str()).collect();
        assert!(points.contains(&"script.invoked"), "缺少 invoked：{points:?}");
        assert!(points.contains(&"script.finished"), "缺少 finished：{points:?}");

        let finished = reported
            .iter()
            .find(|d| d.point == "script.finished")
            .expect("有 finished");
        assert_eq!(finished.outcome, Some(DiagOutcome::Passed), "结果要如实带上");
        assert!(finished.duration_ms.is_some(), "耗时要如实带上");
        assert!(
            matches!(&finished.origin, Origin::Plugin { id } if id == "plugin:test"),
            "来源要按插件归因，实际 {:?}",
            finished.origin
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SP-11：预算里的**进程内循环上限**必须真的收紧到沙箱上。
    ///
    /// 给一个比默认（一千万）小得多的预算，断言它在**远早于墙钟超时**的位置被限额打断——
    /// 若上限没被传下去，这条会等到 20 秒的墙钟才返回（表现为慢而不是错，正是要防的形态）。
    #[tokio::test]
    async fn test_budget_loop_limit_is_honored() {
        let (dir, path) = write_plugin(
            "export default { tools: [ { name: 'spin', description: '死循环', \
             execute: async () => { while (true) {} } } ] }\n",
        );
        let host = TsScriptHost::new();
                // 墙钟给 60 秒（足够宽），循环上限给 50_000（很紧）
        let ctx = ScriptCtx {
            budget: agent_base::ports::ScriptBudget {
                timeout_ms: 60_000,
                loop_iterations: 50_000,
                ..Default::default()
            },
            cancel: &NeverCancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        };

        let handle = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");

        let started = std::time::Instant::now();
        let res = host
            .invoke(
                &handle,
                &InvokeRequest {
                    export: "spin".into(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await;
        let elapsed = started.elapsed();

        assert!(
            matches!(res, Err(ScriptError::BudgetExceeded { .. })),
            "必须报预算超限，实际 {res:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "紧的循环上限必须**远早于墙钟**生效，实际 {elapsed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SP-13 端到端：插件默认档下 **`fetch` 在插件里根本不存在**，显式声明网络之后才出现。
    ///
    /// 两个方向都断言，是为了区分"门禁生效"与"沙箱本来就坏"——只测前者的话，
    /// 一个连 JS 都跑不起来的沙箱也能让 `typeof fetch` 是 undefined。
    #[tokio::test]
    async fn test_plugin_default_needs_hides_the_network() {
        let (dir, path) = write_plugin(
            "export default { tools: [ { name: 'probe', description: '看有没有网', \
             execute: async () => ({ ok: true, output: typeof fetch }) } ] }\n",
        );
        let host = TsScriptHost::new();
                let ctx = ctx_budget(20_000);

        let handle = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path.clone()),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");
        let closed = host
            .invoke(
                &handle,
                &InvokeRequest {
                    export: "probe".into(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await
            .expect("调用应当成功");
        assert_eq!(
            closed.output, "undefined",
            "插件默认档下 `fetch` 必须不存在（连原生桥都不该挂上去）"
        );

        // 声明了网络 → 同一个插件就能看到 fetch（开关是活的）
        let open = PluginSandbox::call_tool_with_needs(
            &path,
            "probe",
            serde_json::json!({}),
            &dir,
            20,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("调用应当成功");
        assert_eq!(open.output, "function", "声明网络之后 fetch 必须存在");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 输出上限：**截断要如实标注**，不许静默切掉。
    #[test]
    fn test_output_cap_marks_truncation() {
        let (kept, truncated) = cap_output("abcdef".to_string(), 100);
        assert_eq!(kept, "abcdef");
        assert!(!truncated);

        let (cut, truncated) = cap_output("x".repeat(50), 10);
        assert!(truncated);
        assert!(cut.contains("已截断"), "{cut}");
    }

    /// P4·SP-71 的 DoD 第一条：**不声明 `net` 的插件拿不到 `fetch`**——
    /// 而且声明之后**真的能拿到**（两个方向都断言，才分得清"门禁生效"与"沙箱本来就坏"）。
    #[tokio::test]
    async fn test_declared_needs_gate_the_network_per_plugin() {
        // ① 没有指令 → 宿主地板：fs 开、net 关 → `fetch` 不存在
        let (dir, path) = write_plugin(
            "export default { tools: [ { name: 'probe', description: '看有没有网', \
             execute: async () => ({ ok: true, output: typeof fetch }) } ] }\n",
        );
        let host = TsScriptHost::new();
        let ctx = ctx_budget(20_000);
        let handle = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path.clone()),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");
        let closed = host
            .invoke(
                &handle,
                &InvokeRequest {
                    export: "probe".into(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await
            .expect("调用应当成功");
        assert_eq!(
            closed.output, "undefined",
            "没声明 net 的插件连 fetch 都不该存在（不是'注入了再拦'）"
        );

        // ② 声明了 `net` → 同一个插件就能看到 fetch
        let (dir2, path2) = write_plugin(
            "// @ada-needs net\n\
             export default { tools: [ { name: 'probe', description: '看有没有网', \
             execute: async () => ({ ok: true, output: typeof fetch }) } ] }\n",
        );
        let host2 = TsScriptHost::new();
        let handle2 = host2
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path2),
                    workspace: Some(dir2.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");
        let open = host2
            .invoke(
                &handle2,
                &InvokeRequest {
                    export: "probe".into(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await
            .expect("调用应当成功");
        assert_eq!(open.output, "function", "声明了 net 就该拿到 fetch（开关是活的）");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// 声明是**权威的**：`// @ada-needs net` 只给网络（不等于"地板 + 声明"）。
    ///
    /// 这条守的是"声明了什么 = 拿到什么"——读者不需要在脑子里把两层叠加一遍。
    #[tokio::test]
    async fn test_declaration_replaces_the_floor_instead_of_adding_to_it() {
        // 用 `require('node:fs')` 探 fs 能力：没有 fs 时这条路径取不到模块
        let (dir, path) = write_plugin(
            "// @ada-needs net\n\
             export default { tools: [ { name: 'probe', description: '看 fs', \
             execute: async () => { \
               const fs = require('node:fs'); \
               return { ok: true, output: typeof fs.readFile }; } } ] }\n",
        );
        let host = TsScriptHost::new();
        let ctx = ctx_budget(20_000);
        let handle = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &ctx,
            )
            .await
            .expect("装载应当成功");
        let res = host
            .invoke(
                &handle,
                &InvokeRequest {
                    export: "probe".into(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await;
        // 两种可接受的现象：取模块抛错（fs 未注入），或拿到空对象
        let no_fs = match &res {
            Ok(o) => o.output == "undefined" || o.output.contains("未注入"),
            Err(e) => format!("{e:?}").contains("未注入") || format!("{e:?}").contains("Script"),
        };
        assert!(no_fs, "只声明 net 时不该有 fs 能力：{res:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内联源码如实拒绝（沙箱按路径装载），而不是偷偷写临时文件凑合。
    #[tokio::test]
    async fn test_inline_source_is_honestly_rejected() {
        let host = TsScriptHost::new();
                let ctx = ctx_budget(5_000);
        let res = host
            .load(
                &LoadRequest {
                    source: ScriptSource::Inline {
                        filename: Some("x.ts".into()),
                        source: "export default {}".into(),
                    },
                    workspace: None,
                },
                &ctx,
            )
            .await;
        assert!(matches!(res, Err(ScriptError::Load { .. })), "实际 {res:?}");
    }
}
