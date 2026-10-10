//! 子智能体执行器（W4-T6：**并入 `AgentRuntime::run_turn`**）。
//!
//! # 为什么必须并入
//!
//! 并入前这里有一套**独立的多轮循环**（自己按步数迭代 + 自己的模型流解析 +
//! 自己的工具分派），也就是仓库里的**第二份引擎**——INV-1「单一引擎」在子智能体侧不成立。
//! 它带来的实际问题：取消不穿透、工具元数据另有一套判定、审批与回执结构各写一遍。
//!
//! 现在隔离性由**装配**决定，而不是靠"另写一个循环"：
//!
//! | 隔离维度 | 靠什么保证 |
//! |---|---|
//! | 只拿到 profile 人格（AGENTS.md §3） | [`ProfilePrompt`] |
//! | 上下文一次性、不污染主会话 | [`EphemeralSessionStore`] |
//! | 只读档位挡写工具（运行期第二道防线） | [`ReadonlyEnforcingGate`] |
//! | 只装裁切后的工具（第一道防线） | [`filter_subagent_tools`] + `CompositeToolCatalog` |
//! | 父会话取消**真的**传进来（W4-T3） | `WatchedCancel` → 引擎 `CancelToken` |

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_adapter::cancel::WatchedCancel;
use agent_adapter::clock::SystemClock;
use agent_adapter::model::client::NetworkModelClient;
use agent_adapter::scope::WorkspaceScope;
use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::engine::{AgentRuntime, RunPolicy, TurnRequest};
use agent_base::ports::{EventSink, Tool};
use agent_runtime::CompositeToolCatalog;
use tokio::sync::{mpsc, watch};

use super::ports::{EphemeralSessionStore, ProfilePrompt, ReadonlyEnforcingGate};
use super::types::{SubagentMode, SubagentProfile, SubagentRunResult, SubagentStepUpdate};
use agent_base::model::ProviderConfig;
use crate::checkpoint::CheckpointManager;

/// 子智能体永远禁止调用的**套娃/递归与交互**工具。
///
/// 1. 递归工具：`invoke_subagent`
/// 2. 交互提问：`ask_user`（进程内子代理不允许 ask_user，必须在子环境中自主闭环）
pub const NEVER_FOR_SUBAGENT: &[&str] = &["invoke_subagent", "ask_user"];

/// 尚未进 `ToolDescriptor` 注册表、但**确实存在实现**的 legacy 工具名。
///
/// W4-T5 之后**已清空**：`invoke_subagent` 现在是一等 `Tool`（`subagents/tool.rs`）
/// 且描述符在注册表里，所以它不再是"legacy 专属"。
/// 这张表刻意保留为空——它是"临时豁免必须收敛"的机制：
/// 一旦某个名字进了注册表，`test_subagent_tool_lists_have_no_ghost_names` 会要求把它从这里删掉。
pub const LEGACY_ONLY_TOOLS: &[&str] = &[];

/// 按 profile 裁切可用工具（**基于 `ToolDescriptor`**，W4-T6）。
///
/// 三层收窄，缺一不可：
/// 1. **递归黑名单**（`NEVER_FOR_SUBAGENT`）+ profile 自己的黑名单；
/// 2. **白名单**（支持 `*` 通配）——只收窄不放宽（AGENTS.md §12）；
/// 3. **只读模式**：非只读工具一律剔除（读的是描述符的 `is_readonly()`，INV-3 单一真源）。
pub fn filter_subagent_tools(
    profile: &SubagentProfile,
    all_tools: &[Arc<dyn Tool>],
) -> Vec<Arc<dyn Tool>> {
    let allowed_set: HashSet<&str> = profile.allowed_tools.iter().map(|s| s.as_str()).collect();
    let mut disallowed_set: HashSet<&str> = NEVER_FOR_SUBAGENT.iter().copied().collect();
    if let Some(ref list) = profile.disallowed_tools {
        for item in list {
            disallowed_set.insert(item.as_str());
        }
    }

    all_tools
        .iter()
        .filter(|tool| {
            let desc = tool.descriptor();
            let name = desc.name.as_str();
            if disallowed_set.contains(name) {
                return false;
            }
            if !allowed_set.contains("*") && !allowed_set.contains(name) {
                return false;
            }
            if profile.mode == SubagentMode::Readonly && !desc.is_readonly() {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

pub struct RunSubagentOptions {
    pub profile: SubagentProfile,
    pub task: String,
    pub additional_context: Option<String>,
    pub workspace: PathBuf,
    pub parent_config: ProviderConfig,
    pub checkpoint_mgr: Option<Arc<CheckpointManager>>,
    pub abort_rx: Option<watch::Receiver<bool>>,
    pub update_tx: Option<mpsc::Sender<SubagentStepUpdate>>,
    /// 产品的工具包声明（`spec.toolkits`）。
    ///
    /// W4-T6：子智能体的工具不再来自 `prompt::builtin_tools()` 那张**手写清单**，
    /// 而是按产品声明走**同一处装配**（`tools_for_toolkits`），再按 profile 裁切。
    /// 这样"产品声明了什么能力"与"子智能体能用什么"是同一个真源。
    pub toolkits: Vec<String>,
    /// 模型客户端注入点。
    ///
    /// 生产路径留 `None` → 用真实的 [`NetworkModelClient`]；
    /// 测试注入脚本化模型——**注入点存在不等于生产在用替身**（默认值就是真实实现）。
    pub model: Option<Arc<dyn agent_base::ports::ModelClient>>,
}

/// 把引擎的领域事件投影成子智能体进度（W4-T6）。
///
/// `EventSink::emit` 是同步的，所以用 `try_send`：阻塞引擎会让"界面消费慢"
/// 变成"子智能体停摆"，代价远大于丢一条进度。
struct SubagentProgressSink {
    tx: Option<mpsc::Sender<SubagentStepUpdate>>,
    max_steps: Option<usize>,
    summary: Mutex<String>,
    tool_calls: AtomicUsize,
    steps: AtomicUsize,
}

impl SubagentProgressSink {
    fn new(tx: Option<mpsc::Sender<SubagentStepUpdate>>, max_steps: Option<usize>) -> Self {
        Self {
            tx,
            max_steps,
            summary: Mutex::new(String::new()),
            tool_calls: AtomicUsize::new(0),
            steps: AtomicUsize::new(0),
        }
    }

    fn summary(&self) -> String {
        self.summary.lock().expect("子智能体摘要锁中毒").clone()
    }

    fn tool_calls(&self) -> usize {
        self.tool_calls.load(Ordering::Relaxed)
    }

    fn steps(&self) -> usize {
        self.steps.load(Ordering::Relaxed)
    }

    fn send(&self, status: &str, action: Option<String>, tool: Option<String>) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.try_send(SubagentStepUpdate {
            thread_id: None,
            step: self.steps(),
            max_steps: self.max_steps,
            status: status.to_string(),
            current_action: action,
            tool_call_summary: tool,
        });
    }
}

impl EventSink for SubagentProgressSink {
    fn emit(&self, event: AgentEvent) {
        match &event.body {
            AgentEventBody::TextDelta { text } => {
                self.summary
                    .lock()
                    .expect("子智能体摘要锁中毒")
                    .push_str(text);
            }
            AgentEventBody::ToolCallStarted { name, .. } => {
                self.tool_calls.fetch_add(1, Ordering::Relaxed);
                self.send(
                    "running",
                    Some(format!("调用工具 {name}")),
                    Some(name.clone()),
                );
            }
            // 一次模型调用结束 = 一步
            AgentEventBody::UsageReported { .. } => {
                self.steps.fetch_add(1, Ordering::Relaxed);
                self.send("running", Some("思考与规划中...".to_string()), None);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_result(
    ok: bool,
    summary: String,
    steps: usize,
    tool_calls: usize,
    elapsed: Duration,
    error: Option<String>,
) -> SubagentRunResult {
    SubagentRunResult {
        ok,
        summary,
        steps_executed: steps,
        duration_ms: elapsed.as_millis() as u64,
        tool_calls_count: tool_calls,
        output_file: None,
        error_message: error,
    }
}

/// 子智能体隔离执行器（**走 `AgentRuntime::run_turn`**）。
pub async fn run_subagent(options: RunSubagentOptions) -> SubagentRunResult {
    let start_time = Instant::now();
    let max_steps = options.profile.max_steps;

    if !options.profile.enabled {
        return build_result(
            false,
            format!("子智能体 [{}] 已被禁用", options.profile.name),
            0,
            0,
            start_time.elapsed(),
            Some("子智能体已被禁用".to_string()),
        );
    }

    // 1. 工具：按**产品声明**装配（同一处真源）→ 按 profile 裁切
    let all_tools = match agent_toolkit::tools_for_toolkits(&options.toolkits, &options.workspace) {
        Ok(t) => t,
        Err(e) => {
            return build_result(
                false,
                format!("子智能体工具装配失败：{e}"),
                0,
                0,
                start_time.elapsed(),
                Some(e),
            )
        }
    };
    let authorized = filter_subagent_tools(&options.profile, &all_tools);

    // 2. 模型配置（profile 可覆盖模型名）
    let mut config = options.parent_config.clone();
    if let Some(ref override_cfg) = options.profile.model_override {
        if let Some(ref m) = override_cfg.model {
            config.model = m.clone();
        }
    }

    // 3. 任务文本（与 legacy 逐字一致：委派任务 + 可选补充上下文 + 收尾要求）
    let mut user_prompt = format!("【委派任务】\n{}", options.task);
    if let Some(ref ctx) = options.additional_context {
        if !ctx.trim().is_empty() {
            user_prompt.push_str(&format!("\n\n【补充上下文/参考信息】\n{}", ctx.trim()));
        }
    }
    user_prompt.push_str(
        "\n\n请针对上述任务要求，自主使用工具调研或处理。完成后直接给出结构化、高信息密度的最终总结与建议。",
    );

    // 4. 装配隔离运行时
    let sink = Arc::new(SubagentProgressSink::new(options.update_tx.clone(), max_steps));
    let readonly = options.profile.mode == SubagentMode::Readonly;
    let model: Arc<dyn agent_base::ports::ModelClient> = options
        .model
        .clone()
        .unwrap_or_else(|| Arc::new(NetworkModelClient::new()));
    let runtime = AgentRuntime::new(
        model,
        Arc::new(CompositeToolCatalog::new(authorized, None)),
        Arc::new(ReadonlyEnforcingGate::new(readonly)),
        Arc::new(EphemeralSessionStore::new()),
        Arc::new(ProfilePrompt::new(options.profile.system_prompt.clone())),
        Arc::new(WorkspaceScope::new(options.workspace.clone())),
        Arc::new(SystemClock),
        RunPolicy {
            max_steps: max_steps.map(|s| s as u32),
            max_parallel_tools: 1,
            tool_timeout: Some(Duration::from_secs(120)),
        },
    );

    // 5. 取消：父会话的停止键必须真的传进引擎（W4-T3）
    let cancel = match options.abort_rx.clone() {
        Some(rx) => WatchedCancel::new(rx),
        None => WatchedCancel::never(),
    };

    // 6. 一次性隔离会话
    let thread_id = format!("subagent_{}", uuid::Uuid::new_v4().simple());
    let req = TurnRequest::new(&thread_id, config).with_user_prompt(user_prompt);

    let outcome = runtime.run_turn(req, sink.as_ref(), &cancel).await;

    let summary = sink.summary();
    let summary = if summary.trim().is_empty() {
        "子智能体已完成委派步骤。".to_string()
    } else {
        summary
    };
    let steps = sink.steps();
    let tool_calls = sink.tool_calls();
    let elapsed = start_time.elapsed();

    match outcome {
        Ok(o) => {
            let (ok, error) = match o.stop_reason {
                TurnStopReason::Completed => (true, None),
                TurnStopReason::BudgetExhausted { limit_steps } => (
                    false,
                    Some(format!("子智能体达到步数上限（{limit_steps} 步），未完成收敛")),
                ),
                TurnStopReason::Aborted => (false, Some("子智能体被取消".to_string())),
                TurnStopReason::ModelError => (false, Some("模型调用失败".to_string())),
                TurnStopReason::Denied => (false, Some("调用被策略拒绝".to_string())),
            };
            sink.send(
                if ok { "done" } else { "failed" },
                Some("执行完毕".to_string()),
                None,
            );
            build_result(
                ok,
                summary,
                o.steps_taken.max(steps as u32) as usize,
                tool_calls,
                elapsed,
                error,
            )
        }
        Err(e) => {
            sink.send("failed", Some(format!("执行失败：{e}")), None);
            build_result(false, summary, steps, tool_calls, elapsed, Some(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::builtins::builtin_subagents;
    use super::*;

    /// W4-T6 守门：裁切必须**基于描述符**，且只读档位挡得住写工具。
    #[test]
    fn test_filter_uses_descriptors_and_blocks_writes_in_readonly() {
        let ws = std::path::Path::new("E:/subagent_filter_ws");
        let all = agent_toolkit::tools_for_toolkits(
            &["core".to_string(), "fs".to_string(), "command".to_string()],
            ws,
        )
        .expect("工具包可装配");

        let researcher = builtin_subagents()
            .into_iter()
            .find(|p| p.id == "researcher")
            .expect("内置 researcher");
        assert_eq!(researcher.mode, SubagentMode::Readonly);

        let allowed = filter_subagent_tools(&researcher, &all);
        let names: Vec<String> = allowed.iter().map(|t| t.descriptor().name.clone()).collect();

        assert!(names.contains(&"read_file".to_string()), "{names:?}");
        assert!(!names.contains(&"write_file".to_string()), "只读子体不能拿到写工具：{names:?}");
        assert!(!names.contains(&"edit_file".to_string()), "{names:?}");
        assert!(!names.contains(&"run_command".to_string()), "{names:?}");
        // 递归工具永远不给
        assert!(!names.contains(&"invoke_subagent".to_string()), "{names:?}");
        // 交互提问工具进程内子代理永远不给
        assert!(!names.contains(&"ask_user".to_string()), "进程内子代理永远不得拿到 ask_user：{names:?}");
        // 白名单之外的不给（researcher 没声明 batch_write）
        assert!(!names.contains(&"batch_write".to_string()), "{names:?}");
    }

    /// 通配白名单的 profile（general_purpose）在读写模式下拿到写工具，但**永远**拿不到递归工具与提问工具。
    #[test]
    fn test_wildcard_profile_gets_writes_but_never_recursion() {
        let ws = std::path::Path::new("E:/subagent_filter_ws");
        let all = agent_toolkit::tools_for_toolkits(&["core".to_string(), "fs".to_string()], ws)
            .expect("工具包可装配");

        let general = builtin_subagents()
            .into_iter()
            .find(|p| p.id == "general_purpose")
            .expect("内置 general_purpose");
        assert_eq!(general.mode, SubagentMode::Readwrite);
        assert_eq!(general.max_steps, None, "内置子代理默认不做步数限制");

        let allowed = filter_subagent_tools(&general, &all);
        let names: Vec<String> = allowed.iter().map(|t| t.descriptor().name.clone()).collect();
        assert!(names.contains(&"write_file".to_string()), "{names:?}");
        assert!(!names.contains(&"invoke_subagent".to_string()), "递归必须被黑名单挡住：{names:?}");
        assert!(!names.contains(&"ask_user".to_string()), "进程内子代理即使通配也不得拿到 ask_user：{names:?}");
    }

    /// 进度投影：文本累加成摘要、工具调用计数、一次模型调用算一步。
    #[test]
    fn test_progress_sink_projects_events() {
        let (tx, mut rx) = mpsc::channel::<SubagentStepUpdate>(16);
        let sink = SubagentProgressSink::new(Some(tx), Some(5));

        sink.emit(AgentEvent::new(1, 0, "sub", AgentEventBody::TextDelta { text: "结论".into() }));
        sink.emit(AgentEvent::new(2, 0, "sub", AgentEventBody::TextDelta { text: "如下".into() }));
        sink.emit(AgentEvent::new(
            3,
            0,
            "sub",
            AgentEventBody::ToolCallStarted {
                call_id: "c1".into(),
                name: "read_file".into(),
                args: serde_json::json!({}),
            },
        ));
        sink.emit(AgentEvent::new(
            4,
            0,
            "sub",
            AgentEventBody::UsageReported {
                usage: Default::default(),
                duration_ms: 5,
            },
        ));

        assert_eq!(sink.summary(), "结论如下");
        assert_eq!(sink.tool_calls(), 1);
        assert_eq!(sink.steps(), 1);

        // 进度确实发出去了（工具名 + 步骤）
        let first = rx.try_recv().expect("应有工具进度");
        assert_eq!(first.tool_call_summary.as_deref(), Some("read_file"));
        let second = rx.try_recv().expect("应有步骤进度");
        assert_eq!(second.step, 1);
        assert_eq!(second.max_steps, Some(5));
    }

    /// 进度通道满时**丢进度而不是阻塞引擎**（引擎停摆的代价远大于丢一条进度）。
    #[test]
    fn test_progress_sink_never_blocks_on_full_channel() {
        let (tx, _rx) = mpsc::channel::<SubagentStepUpdate>(1);
        let sink = SubagentProgressSink::new(Some(tx), Some(3));
        // 远超容量；只要不 panic/不阻塞就算通过
        for i in 0..50 {
            sink.emit(AgentEvent::new(
                i,
                0,
                "sub",
                AgentEventBody::ToolCallStarted {
                    call_id: format!("c{i}"),
                    name: "read_file".into(),
                    args: serde_json::json!({}),
                },
            ));
        }
        assert_eq!(sink.tool_calls(), 50, "计数不受丢进度影响");
    }

    /// **W4-T3 出口判据**：父会话的取消必须真的让子智能体停下来。
    ///
    /// 场景刻意做成"子智能体正在跑一条长命令"：取消要连穿三层
    /// （父会话 → 子智能体引擎 → `run_command` 的进程树），
    /// 所以这条断言同时验证 **W4-T3**（子智能体取消）与 **W4-T1**（命令中止真接线）。
    #[tokio::test]
    async fn test_parent_cancel_stops_the_subagent() {
        use agent_base::model::{StreamDelta, ToolCallInfo};
        use agent_base::testing::ScriptedModelClient;

        let sleep_cmd = if cfg!(windows) {
            "ping 127.0.0.1 -n 20 > nul"
        } else {
            "sleep 20"
        };

        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                StreamDelta::ToolCall {
                    call: ToolCallInfo {
                        id: "c1".into(),
                        name: "run_command".into(),
                        args: serde_json::json!({ "command": sleep_cmd, "timeout": 30 }).to_string(),
                    },
                },
                StreamDelta::Done { stop_reason: "tool_calls".into() },
            ],
            vec![
                StreamDelta::Text { text: "不该走到这里".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]));

        // `general_purpose` 是 Readwrite + 通配白名单 → 能拿到 `run_command`
        let profile = builtin_subagents()
            .into_iter()
            .find(|p| p.id == "general_purpose")
            .expect("内置 general_purpose");

        let (abort_tx, abort_rx) = watch::channel(false);
        let (update_tx, _update_rx) = mpsc::channel::<SubagentStepUpdate>(16);

        let options = RunSubagentOptions {
            profile,
            task: "跑一条长命令".to_string(),
            additional_context: None,
            workspace: std::env::current_dir().unwrap(),
            parent_config: ProviderConfig {
                id: "p".into(),
                name: "p".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
            checkpoint_mgr: None,
            abort_rx: Some(abort_rx),
            update_tx: Some(update_tx),
            toolkits: vec!["core".to_string(), "command".to_string()],
            model: Some(model),
        };

        let started = Instant::now();
        let handle = tokio::spawn(async move { run_subagent(options).await });
        // 等命令真的跑起来再按"停止键"
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = abort_tx.send(true);

        let res = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("取消后子智能体必须及时收尾（取消没穿透）")
            .expect("任务不应 panic");
        let elapsed = started.elapsed();

        assert!(!res.ok, "被取消的子智能体不得报告成功：{:?}", res.summary);
        assert!(
            res.error_message
                .as_deref()
                .map(|m| m.contains("取消"))
                .unwrap_or(false),
            "错误信息应说明是取消：{:?}",
            res.error_message
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "取消必须立刻生效，而不是等命令跑完（实际 {elapsed:?}）"
        );
        assert!(
            !res.summary.contains("不该走到这里"),
            "取消后不该继续跑第二轮：{}",
            res.summary
        );
    }
}
