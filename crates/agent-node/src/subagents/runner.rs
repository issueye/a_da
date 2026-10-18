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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_adapter::cancel::WatchedCancel;
use agent_adapter::clock::SystemClock;
use agent_adapter::model::client::NetworkModelClient;
use agent_adapter::scope::WorkspaceScope;
use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::engine::{AgentRuntime, RunPolicy, TurnRequest};
use agent_base::ports::{AfterRequest, BeforeOutcome, BeforeRequest, EventSink, HookCtx, HookRunner, Tool};
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

/// 子智能体的工具池：产品声明 + 按 **profile 档位**补齐的执行工具。
///
/// 两条教训都在这里：
/// 1. 产品刻意不声明 `fs` 时（PM 就是这种：它自己不写代码），子智能体手上只剩
///    `code_outline`（行首前缀启发式）与 `git_diff`（只看已修改文件）——实测因此
///    读不到任何已提交文件，80 次调用把上下文撑到 445k tokens 后失败。**读**必须补。
/// 2. 只补读又走向另一个极端：PM 委派"写测试并跑 go test"时，子智能体回一句
///    "环境无法写盘"——因为 `fs` 写工具与 `command` 都来自**产品**声明，而 PM 没声明。
///
/// 所以边界改成按 **profile 档位**：
/// - `Readonly`（调研/评审类）→ 只补只读文件工具，写工具一律不给；
/// - `Readwrite`（执行类）→ 补完整的 `fs` + `command`，让"写文件、跑测试"真的能做。
///
/// 产品的角色边界（PM 自己不写代码）不受影响：这里只影响**被委派的子智能体**，
/// 而它本来就按 profile 定义职责（另有 `ReadonlyEnforcingGate` 作为运行期第二道防线）。
pub fn subagent_tool_pool(
    product_toolkits: &[String],
    workspace: &std::path::Path,
    profile_mode: SubagentMode,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let mut pool = agent_toolkit::tools_for_toolkits(product_toolkits, workspace)?;
    let readwrite = profile_mode == SubagentMode::Readwrite;

    // 产品没声明 `fs` → 按档位补：只读档位只补读工具，执行档位补全套
    if !product_toolkits.iter().any(|t| t == "fs") {
        let existing: HashSet<String> =
            pool.iter().map(|t| t.descriptor().name.clone()).collect();
        if let Some(fs_tools) = agent_toolkit::tools_for_toolkit("fs", workspace) {
            pool.extend(
                fs_tools
                    .into_iter()
                    .filter(|t| readwrite || t.descriptor().is_readonly())
                    .filter(|t| !existing.contains(&t.descriptor().name)),
            );
        }
    }

    // 产品没声明 `command` → 执行档位补上（"跑 go test / go vet" 是它的职责）
    if readwrite && !product_toolkits.iter().any(|t| t == "command") {
        let existing: HashSet<String> =
            pool.iter().map(|t| t.descriptor().name.clone()).collect();
        if let Some(cmd_tools) = agent_toolkit::tools_for_toolkit("command", workspace) {
            pool.extend(
                cmd_tools
                    .into_iter()
                    .filter(|t| !existing.contains(&t.descriptor().name)),
            );
        }
    }

    Ok(pool)
}

/// 子智能体会话标题：**唯一短 id 必须放在最前面**。
///
/// 同一个父会话先后派多个同 profile 的子智能体时，只看任务前缀它们长得一模一样，
/// 标签页/侧边栏里就成了"只派了一个"（实测三个子会话标题逐字相同）。会话 id 本身是
/// 唯一的（`thread_<millis>_<6hex>`），把它显式带进标题、且靠前放（界面会截断尾部），
/// 界面才区分得开。
pub fn subagent_session_title(thread_id: &str, profile_name: &str, short_task: &str) -> String {
    let short_id = thread_id.rsplit('_').next().unwrap_or("").to_string();
    format!("子代理 #{short_id} [{profile_name}]: {short_task}")
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
    pub parent_thread_id: Option<String>,
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
    /// 子智能体初始化就绪、独立会话分配完成时的启动回调
    pub on_started: Option<Arc<dyn Fn(&str, &str, &str, &std::path::Path) + Send + Sync>>,
    /// 父节点登记的**上下文窗口上限**（tokens）：子智能体撞顶时提前失败并如实自报。
    ///
    /// 子智能体是上下文爆掉的重灾区（实测一次 445k tokens 被网关掐断，报告只写了一半）。
    pub context_window: Option<u64>,
    /// Optional lifecycle hook runner inherited from the parent product.
    pub hook_runner: Option<Arc<dyn HookRunner>>,
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
    /// 上一轮模型调用已结束、下一轮的正文还没开始写——此时要先补一个空行分隔。
    ///
    /// 引擎每轮都把 `TextDelta` 直接追加进摘要；不补分隔的话，多轮的正文会
    /// **首尾粘成一片**（`...environment.The outline tool...`），
    /// 界面上的"子智能体执行总结与汇报"读起来就是一坨连体字。
    round_separator_pending: AtomicBool,
    /// 引擎报出的失败原文（`AgentEventBody::Failed`）。
    ///
    /// 早先这里被 `_ => {}` 丢掉，回执里只剩一句笼统的"模型调用失败"——
    /// 于是"为什么失败"只能靠猜（实测那次是流到一半断掉，错误文本全没了）。
    failure_reason: Mutex<Option<String>>,
    /// 见过的最大 prompt 用量：子智能体失败的常见根因是上下文被工具输出撑爆。
    peak_prompt_tokens: AtomicUsize,
}

impl SubagentProgressSink {
    fn new(tx: Option<mpsc::Sender<SubagentStepUpdate>>, max_steps: Option<usize>) -> Self {
        Self {
            tx,
            max_steps,
            summary: Mutex::new(String::new()),
            tool_calls: AtomicUsize::new(0),
            steps: AtomicUsize::new(0),
            round_separator_pending: AtomicBool::new(false),
            failure_reason: Mutex::new(None),
            peak_prompt_tokens: AtomicUsize::new(0),
        }
    }

    fn failure_reason(&self) -> Option<String> {
        self.failure_reason.lock().expect("子智能体失败原因锁中毒").clone()
    }

    fn peak_prompt_tokens(&self) -> Option<u64> {
        match self.peak_prompt_tokens.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n as u64),
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
                let mut summary = self.summary.lock().expect("子智能体摘要锁中毒");
                // 轮次之间的分隔：让多轮正文在界面上分行，而不是粘成一整段
                if self.round_separator_pending.swap(false, Ordering::Relaxed)
                    && !summary.trim().is_empty()
                {
                    summary.push_str("\n\n");
                }
                summary.push_str(text);
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
            AgentEventBody::UsageReported { usage, .. } => {
                self.steps.fetch_add(1, Ordering::Relaxed);
                self.round_separator_pending.store(true, Ordering::Relaxed);
                if usage.prompt_tokens > 0 {
                    self.peak_prompt_tokens
                        .fetch_max(usage.prompt_tokens as usize, Ordering::Relaxed);
                }
                self.send("running", Some("思考与规划中...".to_string()), None);
            }
            // 失败原文必须留住：这是"为什么失败"的唯一来源
            AgentEventBody::Failed { message } => {
                let mut slot = self.failure_reason.lock().expect("子智能体失败原因锁中毒");
                if slot.is_none() {
                    *slot = Some(message.clone());
                }
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
    thread_id: Option<String>,
    prompt_tokens: Option<u64>,
) -> SubagentRunResult {
    SubagentRunResult {
        ok,
        summary,
        steps_executed: steps,
        duration_ms: elapsed.as_millis() as u64,
        tool_calls_count: tool_calls,
        output_file: None,
        error_message: error,
        thread_id,
        prompt_tokens,
    }
}

/// 子智能体隔离执行器（**走 `AgentRuntime::run_turn`**）。
async fn run_subagent_before(
    runner: Option<&Arc<dyn HookRunner>>,
    point: &str,
    payload: serde_json::Value,
) -> BeforeOutcome {
    let Some(runner) = runner else {
        return BeforeOutcome::Pass;
    };
    let cancel = WatchedCancel::never();
    let observations = Mutex::new(Vec::new());
    let on_event = |obs| {
        if let Ok(mut events) = observations.lock() {
            events.push(obs);
        }
    };
    let ctx = HookCtx {
        thread_id: "",
        turn_id: "",
        call_id: None,
        cancel: &cancel,
        on_event: &on_event,
    };
    runner
        .before(
            BeforeRequest {
                point: point.to_string(),
                subject: None,
                payload,
            },
            &ctx,
        )
        .await
}

async fn run_subagent_after(
    runner: Option<&Arc<dyn HookRunner>>,
    point: &str,
    payload: serde_json::Value,
) {
    let Some(runner) = runner else {
        return;
    };
    let cancel = WatchedCancel::never();
    let observations = Mutex::new(Vec::new());
    let on_event = |obs| {
        if let Ok(mut events) = observations.lock() {
            events.push(obs);
        }
    };
    let ctx = HookCtx {
        thread_id: "",
        turn_id: "",
        call_id: None,
        cancel: &cancel,
        on_event: &on_event,
    };
    let _ = runner
        .after(
            AfterRequest {
                point: point.to_string(),
                subject: None,
                payload,
            },
            &ctx,
        )
        .await;
}

pub async fn run_subagent(options: RunSubagentOptions) -> SubagentRunResult {
    let start_time = Instant::now();
    let max_steps = options.profile.max_steps;

    let before = run_subagent_before(
        options.hook_runner.as_ref(),
        agent_base::ports::hooks::points::BEFORE_SUBAGENT_START,
        serde_json::json!({
            "subagentId": options.profile.id,
            "taskChars": options.task.chars().count(),
            "parentThreadId": options.parent_thread_id,
        }),
    )
    .await;
    if let BeforeOutcome::Deny { reason } | BeforeOutcome::AbortTurn { reason } = before {
        return build_result(
            false,
            "���������ӵ���ʼ�����ӵ�λ�ܾ�".to_string(),
            0,
            0,
            start_time.elapsed(),
            Some(reason),
            None,
            None,
        );
    }

    if !options.profile.enabled {
        return build_result(
            false,
            format!("子智能体 [{}] 已被禁用", options.profile.name),
            0,
            0,
            start_time.elapsed(),
            Some("子智能体已被禁用".to_string()),
            None,
            None,
        );
    }

    // 1. 工具：按**产品声明**装配（同一处真源）→ 产品没声明 `fs` 时补只读文件工具 → 按 profile 裁切
    let all_tools = match subagent_tool_pool(&options.toolkits, &options.workspace, options.profile.mode) {
        Ok(t) => t,
        Err(e) => {
            return build_result(
                false,
                format!("子智能体工具装配失败：{e}"),
                0,
                0,
                start_time.elapsed(),
                Some(e),
                None,
                None,
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

    // 4. 为子智能体生成独立会话 ID 并落盘持久化，支持在界面标签与工作区独立查看
    let now_millis = agent_toolkit::now_ms();
    let rand_suffix = uuid::Uuid::new_v4().simple().to_string();
    let thread_id = format!("thread_{}_{}", now_millis, &rand_suffix[..6]);

    let short_task: String = options.task.chars().take(24).collect();
    let short_task = short_task.replace(['\r', '\n'], " ");
    let title = subagent_session_title(&thread_id, &options.profile.name, &short_task);

    let session_mgr = crate::session::SessionManager::new(None);
    let _ = session_mgr.create_session(
        &thread_id,
        &options.workspace.to_string_lossy(),
        Some(&title),
        options.parent_thread_id.as_deref(),
        Some(&options.profile.id),
    );

    let session_store = EphemeralSessionStore::new()
        .with_persistence(options.workspace.clone(), session_mgr.clone());

    if let Some(ref cb) = options.on_started {
        cb(
            &thread_id,
            options.parent_thread_id.as_deref().unwrap_or(""),
            &options.profile.id,
            &options.workspace,
        );
    }

    // 5. 装配隔离运行时
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
        Arc::new(session_store),
        Arc::new(ProfilePrompt::new(options.profile.system_prompt.clone())),
        Arc::new(WorkspaceScope::new(options.workspace.clone())),
        Arc::new(SystemClock),
        RunPolicy {
            max_steps: max_steps.map(|s| s as u32),
            max_parallel_tools: 1,
            tool_timeout: Some(Duration::from_secs(120)),
        },
    );

    // 6. 取消：父会话的停止键必须真的传进引擎（W4-T3）
    let cancel = match options.abort_rx.clone() {
        Some(rx) => WatchedCancel::new(rx),
        None => WatchedCancel::never(),
    };

    let req = TurnRequest::new(&thread_id, config)
        .with_user_prompt(user_prompt)
        .with_context_window(options.context_window);

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
    let failure_reason = sink.failure_reason();
    let prompt_tokens = sink.peak_prompt_tokens();

    let result = match outcome {
        Ok(o) => {
            let (ok, error) = match o.stop_reason {
                TurnStopReason::Completed => (true, None),
                TurnStopReason::BudgetExhausted { limit_steps } => (
                    false,
                    Some(format!("子智能体达到步数上限（{limit_steps} 步），未完成收敛")),
                ),
                // 上下文撞顶：如实自报数字，并给出可执行的下一步
                TurnStopReason::ContextWindowExceeded { prompt_tokens, window } => (
                    false,
                    Some(format!(
                        "子智能体上下文已达窗口上限（{prompt_tokens}/{window} tokens）：\
                         请把任务拆小、或换更大上下文窗口的模型"
                    )),
                ),
                TurnStopReason::Aborted => (false, Some("子智能体被取消".to_string())),
                // 引擎报出的原文必须带上——笼统一句"模型调用失败"等于什么都没说
                TurnStopReason::ModelError => (
                    false,
                    Some(match failure_reason.clone() {
                        Some(detail) => format!("模型调用失败：{detail}"),
                        None => "模型调用失败".to_string(),
                    }),
                ),
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
                Some(thread_id.clone()),
                prompt_tokens,
            )
        }
        Err(e) => {
            sink.send("failed", Some(format!("执行失败：{e}")), None);
            build_result(
                false,
                summary,
                steps,
                tool_calls,
                elapsed,
                Some(match failure_reason {
                    Some(detail) => format!("{e}（{detail}）"),
                    None => e.to_string(),
                }),
                Some(thread_id.clone()),
                prompt_tokens,
            )
        }
    };
    run_subagent_after(
        options.hook_runner.as_ref(),
        agent_base::ports::hooks::points::AFTER_SUBAGENT_END,
        serde_json::json!({
            "subagentId": options.profile.id,
            "threadId": thread_id,
            "ok": result.ok,
            "durationMs": result.duration_ms,
            "error": result.error_message,
        }),
    )
    .await;
    result
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

    /// 守门：产品没声明 `fs`（PM 就是这种）时，子智能体**仍必须拿到只读文件工具**，
    /// 但绝不能因此获得写能力。
    ///
    /// 实测事故：PM 派出去的调研子体手上只有 `code_outline` 与 `git_diff`，
    /// 读不到任何已提交文件，靠 80 次调用硬猜，把上下文撑到 445k tokens 后模型调用失败。
    ///
    /// 另一半是"只补读"矫枉过正：PM 委派"写测试并跑 go test"时子智能体回"环境无法写盘"。
    /// 所以现在按 **profile 档位**决定补什么：只读档位只补读，执行档位补写 + 命令。
    #[test]
    fn test_subagent_pool_follows_profile_mode() {
        let ws = std::path::Path::new("E:/subagent_pool_ws");
        let pm_like: Vec<String> = vec![
            "core".to_string(),
            "git".to_string(),
            "project".to_string(),
        ];

        // 1) 只读档位：能读，绝不拿到写/命令
        let pool = subagent_tool_pool(&pm_like, ws, SubagentMode::Readonly)
            .expect("PM 风格工具包必须能装配");
        let names: Vec<String> = pool.iter().map(|t| t.descriptor().name.clone()).collect();
        for expect in ["read_file", "list_files", "search_files"] {
            assert!(names.contains(&expect.to_string()), "子智能体必须能读：缺 {expect}：{names:?}");
        }
        for banned in ["write_file", "edit_file", "batch_write", "batch_replace", "run_command"] {
            assert!(
                !names.contains(&banned.to_string()),
                "只读档位不得拿到 {banned}：{names:?}"
            );
        }

        // 2) 执行档位：必须真的能写盘、能跑命令（PM 委派"写测试 + go test"就靠它）
        let pool = subagent_tool_pool(&pm_like, ws, SubagentMode::Readwrite)
            .expect("PM 风格工具包必须能装配");
        let names: Vec<String> = pool.iter().map(|t| t.descriptor().name.clone()).collect();
        for expect in ["read_file", "write_file", "edit_file", "run_command"] {
            assert!(
                names.contains(&expect.to_string()),
                "执行档位必须拿到 {expect}（否则会回「环境无法写盘」）：{names:?}"
            );
        }

        // 3) 只读调研 profile 从同一个池子里仍然拿不到写工具（profile 是最后一道闸）
        let researcher = builtin_subagents()
            .into_iter()
            .find(|p| p.id == "researcher")
            .expect("内置 researcher");
        let pool = subagent_tool_pool(&pm_like, ws, researcher.mode).expect("装配");
        let allowed: Vec<String> = filter_subagent_tools(&researcher, &pool)
            .iter()
            .map(|t| t.descriptor().name.clone())
            .collect();
        assert!(
            allowed.contains(&"read_file".to_string()),
            "只读调研子体拿不到 read_file 就没法干活：{allowed:?}"
        );
        assert!(
            !allowed.contains(&"write_file".to_string()),
            "只读档位不得拿到写工具：{allowed:?}"
        );

        // 4) 产品自己声明了 `fs` / `command` 时语义不变
        let coding_like: Vec<String> =
            vec!["core".to_string(), "fs".to_string(), "command".to_string()];
        let pool = subagent_tool_pool(&coding_like, ws, SubagentMode::Readwrite)
            .expect("coding 风格工具包必须能装配");
        let names: Vec<String> = pool.iter().map(|t| t.descriptor().name.clone()).collect();
        assert!(names.contains(&"write_file".to_string()), "{names:?}");
        assert!(names.contains(&"run_command".to_string()), "{names:?}");
    }

    /// 守门：**子会话标题必须带唯一短 id，且放在最前面**。
    ///
    /// 实测事故：同一个父会话先后派了三个同 profile 的子智能体，三个标题逐字相同
    /// （`子代理 [全能执行专员]: 在工作区 …`），界面上看起来"只派了一个"。
    #[test]
    fn test_subagent_title_carries_unique_short_id() {
        let t1 = subagent_session_title("thread_1791721619138_36fb30", "全能执行专员", "在工作区 E:/x 中创建");
        let t2 = subagent_session_title("thread_1791721898753_00afd4", "全能执行专员", "在工作区 E:/x 中创建");
        assert_ne!(t1, t2, "同 profile 连派多个也必须能区分：{t1} / {t2}");
        assert!(t1.starts_with("子代理 #36fb30"), "短 id 要靠前（界面会截断尾部）：{t1}");
        assert!(t2.starts_with("子代理 #00afd4"), "{t2}");
        assert!(t1.contains("全能执行专员"), "{t1}");
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
            context_window: None,
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
            parent_thread_id: None,
            toolkits: vec!["core".to_string(), "command".to_string()],
            model: Some(model),
            on_started: None,
            hook_runner: None,
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

    /// 守门：多轮正文之间**必须有分隔**，不能首尾粘成一整段。
    ///
    /// 引擎每轮都把 `TextDelta` 直接追加进摘要；不补分隔的话，界面上的
    /// "子智能体执行总结与汇报"读起来就是一坨连体字
    /// （实测原文：`...environment.The outline tool doesn't render...`）。
    #[test]
    fn test_summary_separates_rounds_so_text_is_not_glued() {
        let sink = SubagentProgressSink::new(None, None);
        let mut seq = 0u64;
        let mut emit = |body: AgentEventBody| {
            seq += 1;
            sink.emit(AgentEvent::new(seq, seq as i64, "t_sub", body));
        };

        emit(AgentEventBody::TextDelta { text: "第一轮：先看目录。".into() });
        emit(AgentEventBody::TextDelta { text: "再看模块清单。".into() });
        emit(AgentEventBody::UsageReported {
            usage: agent_base::model::TokenUsage::default(),
            duration_ms: 10,
        });
        emit(AgentEventBody::TextDelta { text: "第二轮：读 go.mod。".into() });

        assert_eq!(
            sink.summary(),
            "第一轮：先看目录。再看模块清单。\n\n第二轮：读 go.mod。",
            "同一轮的碎块要连起来，跨轮要另起一段"
        );
    }

    /// 守门：引擎报出的失败原文必须留住，并记下**见过的最大上下文用量**。
    ///
    /// 早先 `Failed` 被 `_ => {}` 丢掉，回执里只剩笼统一句"模型调用失败"——实测那次
    /// 是流到一半断掉（报告写到一半），错误文本全没了，"为什么失败"只能靠猜。
    /// 同时子智能体失败最常见的原因就是上下文被工具输出撑爆（实测 445k tokens），
    /// 这个数字也必须进回执，界面才不用猜。
    #[test]
    fn test_failure_reason_and_peak_context_are_kept() {
        let sink = SubagentProgressSink::new(None, None);
        let mut seq = 0u64;
        let mut emit = |body: AgentEventBody| {
            seq += 1;
            sink.emit(AgentEvent::new(seq, seq as i64, "t_sub", body));
        };

        emit(AgentEventBody::UsageReported {
            usage: agent_base::model::TokenUsage {
                prompt_tokens: 445_080,
                completion_tokens: 557,
                total_tokens: 445_637,
                ..Default::default()
            },
            duration_ms: 10,
        });
        emit(AgentEventBody::UsageReported {
            usage: agent_base::model::TokenUsage {
                prompt_tokens: 120_000,
                ..Default::default()
            },
            duration_ms: 10,
        });
        emit(AgentEventBody::Failed {
            message: "error decoding response body".into(),
        });

        assert_eq!(
            sink.peak_prompt_tokens(),
            Some(445_080),
            "必须记住见过的最大 prompt 用量（上下文撑爆是首要失败根因）"
        );
        assert_eq!(
            sink.failure_reason().as_deref(),
            Some("error decoding response body"),
            "引擎报出的失败原文必须留住"
        );
    }
}
