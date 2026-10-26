//! 插件工具的 `Tool` 适配器（P1 · SP-15）。
//!
//! # 它解决的是"最后一公里"
//!
//! 到 SP-15 之前，插件脚本已经能被装载与调用（[`ScriptHost`]），但**引擎不认识它**：
//! 引擎只认 [`Tool`]。这一层就是把两者接起来的那一小段——顺带把"脚本结果"翻译成
//! "工具回执"（**全仓唯一转换点**，见 `docs/plugins/README.md` §2 "四类贡献"）。
//!
//! # 描述符从注册表来（决策 1：只认注册表）
//!
//! [`PluginTool::new`] 一律用 `agent_toolkit::find_tool_descriptor` 取描述符：
//! **名字不在注册表里就拒绝构造**。于是插件工具不可能自述"我是只读的"，
//! 也就不可能借自述绕过只读档位与只读子智能体（INV-3 + AGENTS §2 的失败安全）。
//!
//! 代价如实记着：这一档下插件只能**实现已有名字**的工具。放开"插件自带 schema"
//! 是后续决策（计划 §9 决策 1 的 (b)）。
//!
//! # 时间戳
//!
//! 回执要 `started_at` / `finished_at`，而 `ToolContext` 里没有时钟端口——
//! 与既有工具（`ask_user`）同一约定：用 `agent_toolkit::now_ms()` 记墙钟毫秒。

use std::sync::Arc;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt,
};
use agent_base::ports::{
    BoxFuture, InvokeRequest, ModuleHandle, ScriptBudget, ScriptCtx, ScriptError, ScriptHost,
    ScriptOutcome, ScriptToolDeclaration, Tool, ToolContext,
};

/// 系统时钟毫秒（与既有工具同一约定）。
/// 把一次脚本调用的结果翻译成工具回执。**全仓唯一转换点**。
///
/// 为什么强调"唯一"：这套映射是"脚本出错"与"撞预算"对模型含义的唯一定义——
/// 前者是"这个工具坏了"，后者是"换条路子"（可纠正）。两处各写一份，模型收到的话术
/// 就会不一致，而它下一步该怎么做完全取决于这句话。
///
/// 映射表（设计 §3.6）：
///
/// | 脚本侧 | 回执 | 对模型的含义 |
/// |---|---|---|
/// | `Ok(outcome)` | `Success` | 正常结果 |
/// | `BudgetExceeded` / `TimedOut` | `Timeout` | **可纠正**：换条路子，别原样重试 |
/// | `Aborted` | `Aborted` | 用户/系统中止了这次调用 |
/// | `UnknownExport` | `Error` | 插件里没有这个工具（装配期本该发现） |
/// | `Script` / `Load` / `Unavailable` | `Error` | 脚本自己的问题 |
pub fn outcome_to_receipt(
    outcome: Result<ScriptOutcome, ScriptError>,
    started_at: i64,
    finished_at: i64,
    tool_name: &str,
) -> ToolReceipt {
    match outcome {
        Ok(o) => {
            let mut receipt = ToolReceipt::success(o.output, started_at, finished_at);
            receipt.details = o.data;
            receipt
        }
        Err(ScriptError::BudgetExceeded { kind }) => ToolReceipt::timeout(
            format!(
                "插件工具 `{tool_name}` 撞上沙箱预算（{kind}），已停止等待。\
                 不要原样重试同一调用：把处理范围改小、拆成分步，或换一个工具。"
            ),
            started_at,
            finished_at,
        ),
        Err(ScriptError::TimedOut { limit_ms }) => ToolReceipt::timeout(
            format!(
                "插件工具 `{tool_name}` 超过 {} 秒没有返回，已停止等待。\
                 不要原样重试同一调用：缩小范围、改成分步/后台执行，或换一个工具。",
                limit_ms / 1000
            ),
            started_at,
            finished_at,
        ),
        Err(ScriptError::Aborted) => {
            ToolReceipt::aborted(format!("插件工具 `{tool_name}` 被中止"), started_at, finished_at)
        }
        Err(ScriptError::UnknownExport { export }) => ToolReceipt::error(
            format!("插件里没有名为 `{export}` 的工具（装配期本应发现）"),
            started_at,
            finished_at,
        ),
        Err(ScriptError::Script { message }) => {
            ToolReceipt::error(format!("插件脚本报错：{message}"), started_at, finished_at)
        }
        Err(ScriptError::Load { message }) => {
            ToolReceipt::error(format!("插件装载失败：{message}"), started_at, finished_at)
        }
        Err(ScriptError::Unavailable { message }) => {
            ToolReceipt::error(format!("插件运行时不可用：{message}"), started_at, finished_at)
        }
    }
}

/// 一个插件提供的工具（把 `ScriptHost` 的调用包装成引擎认得的 [`Tool`]）。
pub struct PluginTool {
    /// **取自注册表**（决策 1），不是插件自述。
    descriptor: ToolDescriptor,
    host: Arc<dyn ScriptHost>,
    handle: ModuleHandle,
    plugin_id: String,
    budget: ScriptBudget,
}

impl PluginTool {
    /// 构造：名字必须在**注册表**里（决策 1：只认注册表）。
    pub fn new(
        tool_name: &str,
        host: Arc<dyn ScriptHost>,
        handle: ModuleHandle,
        plugin_id: impl Into<String>,
    ) -> Result<Self, String> {
        let descriptor = agent_toolkit::find_tool_descriptor(tool_name).ok_or_else(|| {
            format!(
                "插件工具 `{tool_name}` 不在内置注册表里——当前口径（决策 1）只认注册表：\
                 插件工具必须先用已注册的工具名，自述的描述符不予采信。\
                 若要支持插件自带 schema，需先拍板放开（见 docs/plugins/README.md §2 四类贡献）"
            )
        })?;
        Ok(Self {
            descriptor: descriptor.clone(),
            host,
            handle,
            plugin_id: plugin_id.into(),
            budget: ScriptBudget::default(),
        })
    }

    /// 覆盖默认预算（组合根按产品声明给；引擎的 `tool_timeout` 仍是外层上限）。
    pub fn with_budget(mut self, budget: ScriptBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Create a plugin-defined tool.
    ///
    /// Since third-party declarations cannot claim read-only status, custom tools are
    /// conservatively classified as write-classified and require approval.
    ///
    /// 描述符用 [`Access::External`]，**不是** `Mutates { Batch("paths") }`：
    /// 后者是一句谎话——插件工具的 schema 里没有 `paths` 字段，而声明了批量路径
    /// 就等于向回滚/检查点承诺“我能按路径撤销”。外部工具的作用域根本不在工作区坐标系里。
    pub fn from_declaration(
        declaration: &ScriptToolDeclaration,
        host: Arc<dyn ScriptHost>,
        handle: ModuleHandle,
        plugin_id: impl Into<String>,
    ) -> Result<Self, String> {
        let name = declaration.name.trim();
        if name.is_empty() || name.contains(' ') {
            return Err("插件工具名称不能为空且不能包含空格".to_string());
        }
        if !declaration.parameters.is_object() {
            return Err(format!("插件工具 `{name}` 的 parameters 必须是 JSON Schema 对象"));
        }
        let descriptor = agent_toolkit::find_tool_descriptor(name).cloned().unwrap_or_else(|| ToolDescriptor {
            name: name.to_string(),
            summary: if declaration.description.trim().is_empty() {
                format!("插件工具 {name}")
            } else {
                declaration.description.clone()
            },
            schema: declaration.parameters.clone(),
            access: Access::External,
            approval: ApprovalPolicy::Always,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        });
        Ok(Self {
            descriptor,
            host,
            handle,
            plugin_id: plugin_id.into(),
            budget: ScriptBudget::default(),
        })
    }
}

impl Tool for PluginTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = agent_toolkit::now_ms();
            let script_ctx = ScriptCtx {
                budget: self.budget,
                cancel: ctx.cancel,
                thread_id: ctx.thread_id,
                origin_id: Some(self.plugin_id.as_str()),
            };
            let outcome = self
                .host
                .invoke(
                    &self.handle,
                    &InvokeRequest {
                        export: call.name.clone(),
                        args: call.args.clone(),
                    },
                    &script_ctx,
                )
                .await;
            outcome_to_receipt(outcome, started_at, agent_toolkit::now_ms(), &call.name)
        })
    }

    // 刻意**不覆写** `long_running()`：插件工具默认 `false`，
    // 于是受引擎策略超时约束——第三方脚本不该有"我是长任务"的免死金牌。
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::ports::{ModuleHandle, ScriptError};
    use agent_base::testing::{DiagCollector, NeverCancel, RecordingSink};
    use std::sync::Arc;

    fn receipt_for(outcome: Result<ScriptOutcome, ScriptError>) -> ToolReceipt {
        outcome_to_receipt(outcome, 100, 250, "read_file")
    }

    fn ok_outcome(output: &str) -> ScriptOutcome {
        ScriptOutcome {
            ok: true,
            output: output.to_string(),
            data: None,
            duration_ms: 150,
            truncated: false,
        }
    }

    /// 唯一转换点：**成功**带上输出，耗时从时间戳派生。
    #[test]
    fn test_success_maps_to_success_receipt() {
        let r = receipt_for(Ok(ok_outcome("done")));
        assert!(r.ok());
        assert_eq!(r.output, "done");
        assert_eq!(r.duration_ms(), 150);
    }

    /// 可纠正的两类必须映成 `Timeout`，且文案要**告诉模型换路子**（否则它只会原样重试）。
    #[test]
    fn test_recoverable_failures_map_to_timeout_with_actionable_text() {
        for err in [
            ScriptError::BudgetExceeded {
                kind: "loop_iterations".to_string(),
            },
            ScriptError::TimedOut { limit_ms: 5_000 },
        ] {
            let r = receipt_for(Err(err.clone()));
            assert_eq!(r.status, agent_base::domain::ToolStatus::Timeout, "{err:?}");
            assert!(
                r.output.contains("不要原样重试"),
                "超时回执必须给出下一步，实际：{}",
                r.output
            );
        }
    }

    /// 其余失败分成三类，各自的说法不能混：中止 / 入口不存在 / 脚本报错。
    #[test]
    fn test_other_failures_are_classified() {
        assert_eq!(
            receipt_for(Err(ScriptError::Aborted)).status,
            agent_base::domain::ToolStatus::Aborted
        );
        let unknown = receipt_for(Err(ScriptError::UnknownExport {
            export: "nope".to_string(),
        }));
        assert_eq!(unknown.status, agent_base::domain::ToolStatus::Error);
        assert!(unknown.output.contains("装配期本应发现"), "{}", unknown.output);

        let script = receipt_for(Err(ScriptError::Script {
            message: "boom".to_string(),
        }));
        assert!(script.output.contains("boom"));
    }

    /// Builtin wrappers still use the canonical descriptor, while custom
    /// plugin tools receive a conservative write/approval descriptor.
    #[test]
    fn test_custom_tool_gets_conservative_descriptor() {
        let host: Arc<dyn ScriptHost> = Arc::new(crate::scripts::TsScriptHost::new());
        let handle = ModuleHandle {
            id: "script-x".to_string(),
        };
        let declaration = ScriptToolDeclaration {
            name: "plugin_custom".to_string(),
            label: None,
            description: "plugin custom tool".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        };
        let custom = PluginTool::from_declaration(&declaration, host.clone(), handle.clone(), "p")
            .expect("valid custom declaration is dynamically registered");
        assert!(!custom.descriptor().is_readonly(), "custom tool registration must fail safe as write");
        assert_eq!(custom.descriptor().approval, ApprovalPolicy::Always);
        assert_eq!(
            custom.descriptor().access,
            Access::External,
            "未知作用域的工具用 External：不能谎称知道它改哪些路径"
        );
        assert_eq!(
            custom.descriptor().rollback,
            RollbackPolicy::None,
            "外部工具无法在本地回滚，不许声称能撤销"
        );

        // Builtin names retain their canonical descriptors.
        let tool = PluginTool::new("read_file", host, handle, "p").expect("已注册名字应当可以构造");
        assert_eq!(tool.descriptor().name, "read_file");
        assert!(tool.descriptor().is_readonly(), "读写性来自注册表");
    }

    /// SP-15 的端到端：**真插件 + 真沙箱**，回执由引擎认得。
    #[tokio::test]
    async fn test_executes_a_real_plugin_tool_end_to_end() {
        let dir = std::env::temp_dir().join(format!("a_da_plugintool_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("t.ts");
        // Builtin wrapper names continue to use the canonical descriptor.
        std::fs::write(
            &path,
            "export default { tools: [ { name: 'read_file', description: 'x', \
             execute: async (args: any) => ({ ok: true, output: 'from-plugin:' + args.path }) } ] }\n",
        )
        .expect("写临时插件失败");

        let host = Arc::new(crate::scripts::TsScriptHost::new());
        let events = RecordingSink::new();
        let scope = agent_base::testing::MockScope::new("ws");
        let cancel = NeverCancel;
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
            hooks: None,
        };
        let load_ctx = ScriptCtx {
            budget: ScriptBudget::default(),
            cancel: &cancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        };
        let handle = host
            .load(
                &agent_base::ports::LoadRequest {
                    source: agent_base::ports::ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &load_ctx,
            )
            .await
            .expect("装载应当成功");

        let tool = PluginTool::new("read_file", host.clone(), handle, "plugin:test")
            .expect("构造应当成功")
            .with_budget(ScriptBudget::default());
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            args: serde_json::json!({ "path": "a.rs" }),
        };
        let receipt = tool.execute(&call, &ctx).await;
        assert!(receipt.ok(), "实际：{receipt:?}");
        assert_eq!(receipt.output, "from-plugin:a.rs");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1 DoD 的那条：**同步死循环必须得到 `Timeout` 回执，而不是挂住**。
    #[tokio::test]
    async fn test_infinite_loop_yields_timeout_receipt() {
        let dir = std::env::temp_dir().join(format!("a_da_plugintool_spin_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("spin.ts");
        std::fs::write(
            &path,
            "export default { tools: [ { name: 'read_file', description: 'x', \
             execute: async () => { while (true) {} } } ] }\n",
        )
        .expect("写临时插件失败");

        let host = Arc::new(crate::scripts::TsScriptHost::new());
        let events = RecordingSink::new();
        let scope = agent_base::testing::MockScope::new("ws");
        let cancel = NeverCancel;
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
            hooks: None,
        };
        let load_ctx = ScriptCtx {
            budget: ScriptBudget::default(),
            cancel: &cancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        };
        let handle = host
            .load(
                &agent_base::ports::LoadRequest {
                    source: agent_base::ports::ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &load_ctx,
            )
            .await
            .expect("装载应当成功");

        let tool = PluginTool::new("read_file", host, handle, "plugin:test").expect("构造应当成功");
        let call = ToolCall {
            id: "call_2".to_string(),
            name: "read_file".to_string(),
            args: serde_json::json!({}),
        };

        let started = std::time::Instant::now();
        let receipt = tool.execute(&call, &ctx).await;
        let elapsed = started.elapsed();

        assert_eq!(
            receipt.status,
            agent_base::domain::ToolStatus::Timeout,
            "死循环必须映成 Timeout 回执（P1 的 DoD 就是这条），实际 {receipt:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "必须秒级返回，实际 {elapsed:?}——那才是「挂住」的形态"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SP-14：插件打的 `console.log` 必须**结构化带回来**——
    /// 进回执的 `details.console`（给排查看）而**不进** `output`（那是给模型看的，混进去等于污染上下文），
    /// 并且 `script.finished` 诊断上带字节数。
    #[tokio::test]
    async fn test_plugin_console_output_is_captured_into_the_receipt() {
        let dir = std::env::temp_dir().join(format!("a_da_sp14_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("log.ts");
        std::fs::write(
            &path,
            "export default { tools: [ { name: 'read_file', description: 'x', \
             execute: async () => { console.log('from-plugin-log'); \
             console.error('plugin-warn'); return { ok: true, output: 'ok' } } } ] }\n",
        )
        .expect("写临时插件失败");

        let diags = Arc::new(DiagCollector::new());
        let collector = diags.clone();
        let host = Arc::new(
            crate::scripts::TsScriptHost::new().with_diag(Arc::new(move |d| collector.push(d))),
        );
        let events = RecordingSink::new();
        let scope = agent_base::testing::MockScope::new("ws");
        let cancel = NeverCancel;
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
            hooks: None,
        };
        let load_ctx = ScriptCtx {
            budget: ScriptBudget::default(),
            cancel: &cancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        };
        let handle = host
            .load(
                &agent_base::ports::LoadRequest {
                    source: agent_base::ports::ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &load_ctx,
            )
            .await
            .expect("装载应当成功");

        let tool = PluginTool::new("read_file", host, handle, "plugin:test").expect("构造应当成功");
        let receipt = tool
            .execute(
                &ToolCall {
                    id: "c".to_string(),
                    name: "read_file".to_string(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await;

        assert!(receipt.ok());
        let console = receipt
            .details
            .as_ref()
            .and_then(|d| d.get("console"))
            .expect("控制台输出必须结构化带回来（details.console）");
        assert!(
            console["stdout"].as_str().unwrap_or("").contains("from-plugin-log"),
            "{console:?}"
        );
        assert!(
            console["stderr"].as_str().unwrap_or("").contains("plugin-warn"),
            "{console:?}"
        );
        assert!(
            console["stdoutBytes"].as_u64().unwrap_or(0) > 0,
            "原始字节数要带上（截断多少的依据）：{console:?}"
        );
        assert!(
            !receipt.output.contains("from-plugin-log"),
            "控制台输出**不许**混进给模型看的 output：{}",
            receipt.output
        );

        let finished = diags
            .snapshot()
            .into_iter()
            .find(|d| d.point == "script.finished")
            .expect("有 script.finished");
        assert!(
            finished.bytes.unwrap_or(0) > 0,
            "诊断要带控制台字节数：{finished:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 遥测出口是宿主级的：接了收集器就该看见一进一出（与 SP-11 的实现对齐）。
    #[tokio::test]
    async fn test_plugin_tool_path_reports_script_telemetry() {
        let dir = std::env::temp_dir().join(format!("a_da_plugintool_diag_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("d.ts");
        std::fs::write(
            &path,
            "export default { tools: [ { name: 'read_file', description: 'x', \
             execute: async () => ({ ok: true, output: 'ok' }) } ] }\n",
        )
        .expect("写临时插件失败");

        let diags = Arc::new(DiagCollector::new());
        let collector = diags.clone();
        let host = Arc::new(
            crate::scripts::TsScriptHost::new().with_diag(Arc::new(move |d| collector.push(d))),
        );
        let events = RecordingSink::new();
        let scope = agent_base::testing::MockScope::new("ws");
        let cancel = NeverCancel;
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
            hooks: None,
        };
        let load_ctx = ScriptCtx {
            budget: ScriptBudget::default(),
            cancel: &cancel,
            thread_id: "t1",
            origin_id: Some("plugin:test"),
        };
        let handle = host
            .load(
                &agent_base::ports::LoadRequest {
                    source: agent_base::ports::ScriptSource::Path(path),
                    workspace: Some(dir.clone()),
                },
                &load_ctx,
            )
            .await
            .expect("装载应当成功");

        let tool = PluginTool::new("read_file", host, handle, "plugin:test").expect("构造应当成功");
        let _ = tool
            .execute(
                &ToolCall {
                    id: "c".to_string(),
                    name: "read_file".to_string(),
                    args: serde_json::json!({}),
                },
                &ctx,
            )
            .await;

        let points: Vec<String> = diags.snapshot().into_iter().map(|d| d.point).collect();
        assert!(points.iter().any(|p| p == "script.invoked"), "{points:?}");
        assert!(points.iter().any(|p| p == "script.finished"), "{points:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
