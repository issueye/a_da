use super::*;
use crate::domain::{
    Access, Execution, POINT_HOOK_ENTERED, POINT_HOOK_FINISHED, POINT_HOOK_SKIPPED,
    POINT_TURN_ABORTED, PathSelector, RollbackPolicy, ToolCall, ToolDescriptor,
};
use crate::model::{ProviderConfig, ToolCallInfo};
use crate::ports::{BoxFuture, Tool, ToolContext};
use crate::testing::*;

fn mock_provider_config() -> ProviderConfig {
    ProviderConfig {
        id: "test".to_string(),
        name: "test".to_string(),
        protocol: Default::default(),
        base_url: "https://api.test.com".to_string(),
        api_key: "sk-test".to_string(),
        model: "test-model".to_string(),
        max_output_tokens: None,
        custom_headers: None,
        proxy_url: None,
    }
}

fn make_test_descriptor(
    name: &str,
    approval: ApprovalPolicy,
    termination: Termination,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        summary: format!("test tool {name}"),
        schema: serde_json::json!({
            "type": "object",
            "properties": { "arg": { "type": "string" } }
        }),
        access: Access::ReadOnly,
        approval,
        rollback: RollbackPolicy::None,
        execution: Execution::Sequential,
        termination,
    }
}

// ── W5-T5：`FailDirection` 真的参与裁决（INV-4）──────────────────────────

/// 确定答案（`User` / `Policy`）**不受方向影响**——否则"用户明确拒绝"会被 fail-open 翻成放行。
#[test]
fn test_direction_never_overrides_a_definite_answer() {
    for direction in [FailDirection::Closed, FailDirection::Open] {
        let user_denied = ApprovalOutcome::denied(AnsweredBy::User, "用户点了拒绝");
        assert!(
            !AgentRuntime::apply_fail_direction(user_denied, direction).approved,
            "用户明确拒绝不得被 {direction:?} 覆盖"
        );

        let policy_allowed = ApprovalOutcome::allowed(AnsweredBy::Policy);
        assert!(
            AgentRuntime::apply_fail_direction(policy_allowed, direction).approved,
            "策略明确放行不得被 {direction:?} 覆盖"
        );
    }
}

/// 无判定依据（`Timeout` / `Aborted`）**由方向裁决**。
#[test]
fn test_direction_arbitrates_when_there_is_no_answer() {
    // Closed（默认，安全方向）→ 拒绝
    let denied = AgentRuntime::apply_fail_direction(
        ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
        FailDirection::Closed,
    );
    assert!(!denied.approved);
    assert_eq!(denied.by, AnsweredBy::Timeout, "来路必须保留，事后要能分辨");
    assert!(denied.reason.as_deref().unwrap_or("").contains("超时"));

    // Open（显式配置的 fail-open）→ 放行：**这是 direction() 存在的意义**
    let allowed = AgentRuntime::apply_fail_direction(
        ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
        FailDirection::Open,
    );
    assert!(allowed.approved, "声明 Open 的闸门在超时时必须放行");

    // Aborted 同口径
    assert!(
        !AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Aborted, "会话中止"),
            FailDirection::Closed
        )
        .approved
    );
    assert!(
        AgentRuntime::apply_fail_direction(
            ApprovalOutcome::denied(AnsweredBy::Aborted, "会话中止"),
            FailDirection::Open
        )
        .approved
    );
}

/// **安全网**：闸门声明 `Closed` 却对"没答案"给了放行 → 引擎必须翻成拒绝。
///
/// 这条是"声明与行为打架时以声明为准"的兜底——否则一个写错的闸门会静默 fail-open。
#[test]
fn test_closed_direction_is_a_safety_net_over_a_wrong_gate() {
    let wrong = ApprovalOutcome::allowed(AnsweredBy::Timeout);
    let fixed = AgentRuntime::apply_fail_direction(wrong, FailDirection::Closed);
    assert!(!fixed.approved, "声明 Closed 时不得放行无判定依据的调用");
    assert_eq!(fixed.by, AnsweredBy::Timeout);
    assert!(
        fixed.reason.as_deref().unwrap_or("").contains("Closed"),
        "理由应说明是方向裁决：{:?}",
        fixed.reason
    );
}

/// 端到端：声明 `Open` 的闸门在超时时，工具**真的被执行**（不只是纯函数层面对）。
#[tokio::test]
async fn test_open_direction_lets_the_tool_run_after_timeout() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));

    let tool = Arc::new(MockTool::new(
        make_test_descriptor(
            "gated_tool",
            ApprovalPolicy::Always,
            Termination::ContinueTurn,
        ),
        ToolReceipt::success("工具真的跑了", 0, 1),
    ));
    let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "gated_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "done".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    // 闸门：等待超时（没人回答），但声明 fail-open
    let gate = Arc::new(
        RecordingApprovalGate::with_outcomes(vec![ApprovalOutcome::denied(
            AnsweredBy::Timeout,
            "等待超时",
        )])
        .with_direction(FailDirection::Open),
    );

    let rt = AgentRuntime::new(
        model,
        catalog,
        gate,
        store,
        prompt,
        scope,
        clock,
        RunPolicy {
            max_steps: Some(5),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    );

    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_open", mock_provider_config()).with_user_prompt("go");
    let outcome = rt
        .run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
    let executed = sink.snapshot().iter().any(|e| {
        matches!(
            &e.body,
            AgentEventBody::ToolCallFinished { receipt, .. }
                if receipt.status == ToolStatus::Success
        )
    });
    assert!(
        executed,
        "声明 Open 时超时必须放行并真的执行工具：{:?}",
        sink.snapshot()
    );
}

/// 端到端对照：同场景下声明 `Closed`（默认）→ 工具**不被执行**，以拒绝回执回模型。
#[tokio::test]
async fn test_closed_direction_denies_after_timeout() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));

    let tool = Arc::new(MockTool::new(
        make_test_descriptor(
            "gated_tool",
            ApprovalPolicy::Always,
            Termination::ContinueTurn,
        ),
        ToolReceipt::success("工具不该跑", 0, 1),
    ));
    let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "gated_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "done".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let gate = Arc::new(RecordingApprovalGate::with_outcomes(vec![
        ApprovalOutcome::denied(AnsweredBy::Timeout, "等待超时"),
    ]));
    assert_eq!(
        ApprovalGate::direction(gate.as_ref()),
        FailDirection::Closed,
        "默认方向必须是 Closed"
    );

    let rt = AgentRuntime::new(
        model,
        catalog,
        gate,
        store,
        prompt,
        scope,
        clock,
        RunPolicy {
            max_steps: Some(5),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    );

    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_closed", mock_provider_config()).with_user_prompt("go");
    rt.run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    let receipts: Vec<ToolStatus> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1, "应恰好一个工具回执：{receipts:?}");
    assert_eq!(
        receipts[0],
        ToolStatus::Denied,
        "默认 Closed 时超时必须拒绝，而不是执行工具"
    );
}

// ── P3a 钩子（SP-51 / SP-52）：引擎侧的行为与成对埋点 ────────────────────

/// 造一个"模型调一次工具、然后收尾"的运行时（钩子由调用方装）。
fn runtime_with_one_tool(
    runner: Option<Arc<dyn HookRunner>>,
    tool_receipt: ToolReceipt,
) -> (AgentRuntime, Arc<InMemoryToolCatalog>) {
    let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![Arc::new(
        MockTool::new(
            make_test_descriptor(
                "run_command",
                ApprovalPolicy::Never,
                Termination::ContinueTurn,
            ),
            tool_receipt,
        ),
    )]));
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "run_command".into(),
                    args: r#"{"command":"rm -rf /"}"#.into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "done".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));
    let mut rt = AgentRuntime::new(
        model,
        catalog.clone(),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("system")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy {
            max_steps: Some(5),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    );
    if let Some(r) = runner {
        rt = rt.with_hooks(r);
    }
    (rt, catalog)
}

/// 从回执里取出工具结果的**正文**（模型读到的就是它）。
fn tool_output_from_store(store: &InMemorySessionStore, thread_id: &str) -> String {
    store
        .get(thread_id)
        .into_iter()
        .filter_map(|m| match m {
            AgentMessage::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// SP-51 的核心：插件在 `beforeToolCall` 拦下 → **理由以普通工具结果回模型**，
/// 且工具**确实没有执行**。
///
/// 这条守的是"可纠正"：模型要读到"为什么被拦"，而不是一句"执行失败"。
#[tokio::test]
async fn test_before_tool_call_hook_denies_with_a_normal_tool_result() {
    let runner = Arc::new(ScriptedHookRunner::new().deny_at(
        "beforeToolCall",
        "这条命令会删掉整个磁盘，请改用更精确的路径",
    ));
    let (rt, catalog) =
        runtime_with_one_tool(Some(runner.clone()), ToolReceipt::success("真的跑了", 0, 5));
    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_hook_deny", mock_provider_config()).with_user_prompt("go");
    rt.run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    // ① 工具没被执行（回执不是那件工具自己产出的）
    let receipts: Vec<ToolReceipt> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    assert_eq!(
        receipts[0].status,
        ToolStatus::Denied,
        "必须是'被拒'而不是'执行失败'"
    );
    assert!(
        !receipts[0].output.contains("真的跑了"),
        "工具绝不能被执行：{}",
        receipts[0].output
    );
    assert!(
        receipts[0].output.contains("会删掉整个磁盘"),
        "插件给的理由必须原样回给模型（否则模型无从纠正）：{}",
        receipts[0].output
    );

    // ② 也没有 `ToolCallStarted`（没执行就没有开始）
    assert!(
        !sink
            .snapshot()
            .iter()
            .any(|e| matches!(&e.body, AgentEventBody::ToolCallStarted { .. })),
        "被拦下的调用不该发 ToolCallStarted"
    );
    let _ = catalog;
}

/// SP-51：`AbortTurn` 结束**整轮**（比拦下一次更强），且工具没执行。
#[tokio::test]
async fn test_before_tool_call_hook_can_abort_the_whole_turn() {
    let runner = Arc::new(
        ScriptedHookRunner::new().abort_at("beforeToolCall", "检测到高危操作，本轮已终止"),
    );
    let (rt, _) = runtime_with_one_tool(Some(runner), ToolReceipt::success("不该跑", 0, 5));
    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_hook_abort", mock_provider_config()).with_user_prompt("go");
    let outcome = rt
        .run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    assert_eq!(
        outcome.stop_reason,
        TurnStopReason::Aborted,
        "整轮应当被中止"
    );
    assert!(
        !sink
            .snapshot()
            .iter()
            .any(|e| matches!(&e.body, AgentEventBody::ToolCallStarted { .. })),
        "中止之后不该执行工具"
    );
    // 中止原因要如实带上（"谁中止的"是 §1.2 记的缺口）
    let aborted = sink
        .snapshot()
        .into_iter()
        .find_map(|e| match e.body {
            AgentEventBody::Diagnostic(d) if d.point == POINT_TURN_ABORTED => Some(d),
            _ => None,
        })
        .expect("要有 turn.aborted 诊断");
    assert!(
        aborted
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("beforeToolCall"),
        "中止理由要指出是哪个点位：{aborted:?}"
    );
}

/// SP-51：`afterToolCall` 的旁注**追进模型正在读的那段文本**，且**不改状态**。
#[tokio::test]
async fn test_after_tool_call_note_is_appended_without_changing_status() {
    let store = Arc::new(InMemorySessionStore::new());
    let runner = Arc::new(ScriptedHookRunner::new().note_at("afterToolCall", "记得跑一遍测试"));
    let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![Arc::new(
        MockTool::new(
            make_test_descriptor(
                "run_command",
                ApprovalPolicy::Never,
                Termination::ContinueTurn,
            ),
            ToolReceipt::success("命令输出", 0, 5),
        ),
    )]));
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "run_command".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "done".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));
    let rt = AgentRuntime::new(
        model,
        catalog,
        Arc::new(RecordingApprovalGate::new(true)),
        store.clone(),
        Arc::new(FixedPrompt::new("system")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy {
            max_steps: Some(5),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    )
    .with_hooks(runner);

    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_hook_note", mock_provider_config()).with_user_prompt("go");
    rt.run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    let text = tool_output_from_store(&store, "t_hook_note");
    assert!(text.contains("命令输出"), "原输出要留着：{text}");
    assert!(
        text.contains("[插件旁注]") && text.contains("记得跑一遍测试"),
        "旁注要追进模型读到的文本，并标明来源：{text}"
    );
    let receipts: Vec<ToolStatus> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt.status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, vec![ToolStatus::Success], "事后**不得**改状态");
}

/// SP-52：**成对性契约**——每条 `hook.entered` 恰好一个 `hook.finished`，
/// 且配对键一致；没人订阅时报 `hook.skipped`。
#[tokio::test]
async fn test_hook_diagnostics_are_paired_and_absence_is_reported() {
    let runner = Arc::new(ScriptedHookRunner::new().note_at("afterToolCall", "旁注"));
    let (rt, _) = runtime_with_one_tool(Some(runner), ToolReceipt::success("ok", 0, 5));
    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_hook_pairs", mock_provider_config()).with_user_prompt("go");
    rt.run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");

    let points: Vec<(String, Option<String>)> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            AgentEventBody::Diagnostic(d) if d.point.starts_with("hook.") => Some((
                d.point.clone(),
                d.data
                    .get("callId")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            )),
            _ => None,
        })
        .collect();

    let entered: Vec<&(String, Option<String>)> = points
        .iter()
        .filter(|(p, _)| p == POINT_HOOK_ENTERED)
        .collect();
    let finished: Vec<&(String, Option<String>)> = points
        .iter()
        .filter(|(p, _)| p == POINT_HOOK_FINISHED)
        .collect();
    assert!(
        !entered.is_empty(),
        "钩子被调用过就必须有 entered：{points:?}"
    );
    assert_eq!(
        entered.len(),
        finished.len(),
        "成对性是硬约束（每个 entered 恰好一个 finished）：{points:?}"
    );
    for e in &entered {
        assert!(
            finished.iter().any(|f| f.1 == e.1),
            "配对键必须一致：entered={:?} finished={:?}",
            e.1,
            finished.iter().map(|f| f.1.clone()).collect::<Vec<_>>()
        );
    }

    // 没订阅的点位（这里：afterToolCall 有订阅、beforeApproval 没被问过）
    // —— 用"没人订阅"的替身再跑一遍，断言 skipped 如实出现
    let quiet = Arc::new(ScriptedHookRunner::new().unsubscribed());
    let (rt2, _) = runtime_with_one_tool(Some(quiet), ToolReceipt::success("ok", 0, 5));
    let sink2 = RecordingSink::new();
    let req2 = TurnRequest::new("t_hook_none", mock_provider_config()).with_user_prompt("go");
    rt2.run_turn(req2, &sink2, &NeverCancel)
        .await
        .expect("轮次应正常结束");
    let skipped: Vec<_> = sink2
        .snapshot()
        .into_iter()
        .filter_map(|e| match e.body {
            AgentEventBody::Diagnostic(d) if d.point == POINT_HOOK_SKIPPED => Some(d),
            _ => None,
        })
        .collect();
    assert!(!skipped.is_empty(), "没人订阅也要报（缺席可见）");
    assert_eq!(skipped[0].reason.as_deref(), Some("no_subscriber"));
    assert_eq!(
        skipped.iter().find_map(|d| d
            .data
            .get("point")
            .and_then(|v| v.as_str())
            .filter(|p| *p == "beforeToolCall")),
        Some("beforeToolCall"),
        "要指出是哪个点位没人管：{:?}",
        skipped[0]
    );
}

/// 没装钩子的产品**一条钩子诊断都不发**——否则面板会被"没人订阅"刷满，
/// 真问题反而被淹掉（这与"缺席可见"不矛盾：没开这个能力的产品，点位压根不问）。
#[tokio::test]
async fn test_no_hook_diagnostics_when_hooks_are_not_installed() {
    let (rt, _) = runtime_with_one_tool(None, ToolReceipt::success("ok", 0, 5));
    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_no_hooks", mock_provider_config()).with_user_prompt("go");
    rt.run_turn(req, &sink, &NeverCancel)
        .await
        .expect("轮次应正常结束");
    assert!(
        !sink.snapshot().iter().any(|e| matches!(
            &e.body,
            AgentEventBody::Diagnostic(d) if d.point.starts_with("hook.")
        )),
        "没装钩子就不该有点位诊断：{:?}",
        sink.snapshot()
    );
}

// ── P3b：改写（SP-60）与改写后的重校验（SP-61/62）────────────────────

/// 记录"工具到底是被哪份参数调用的"。
struct CapturingTool {
    descriptor: ToolDescriptor,
    seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl Tool for CapturingTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        if let Ok(mut v) = self.seen.lock() {
            v.push(call.args.clone());
        }
        Box::pin(async { ToolReceipt::success("跑完了", 0, 5) })
    }
}

/// 按**参数内容**判定的假闸门：命令里出现 `rm -rf` 才要问。
///
/// 用它才能表达"风险单调性"要验的那件事——同一个工具的**不同参数**风险不同，
/// 而改写不得把风险降下去。
#[derive(Default)]
struct DangerScanGate {
    asked: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl ApprovalGate for DangerScanGate {
    fn needs_approval<'a>(
        &'a self,
        _tool: &'a str,
        _policy: &'a ApprovalPolicy,
        args: &'a serde_json::Value,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move { args.to_string().contains("rm -rf") })
    }
    fn decide<'a>(
        &'a self,
        req: ApprovalRequest,
        _cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, ApprovalOutcome> {
        if let Ok(mut v) = self.asked.lock() {
            v.push(req.args.clone());
        }
        Box::pin(async { ApprovalOutcome::allowed(AnsweredBy::Policy) })
    }
    fn direction(&self) -> FailDirection {
        FailDirection::Closed
    }
}

/// 造一个"模型调一次 `probe`、再收尾"的运行时；`probe` 的参数由用例给。
fn runtime_with_capturing_probe(
    runner: Arc<dyn HookRunner>,
    access: Access,
    schema: serde_json::Value,
    approval_policy: ApprovalPolicy,
    raw_args: &str,
) -> (
    AgentRuntime,
    Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    Arc<DangerScanGate>,
) {
    use crate::model::{StreamDelta as SD, ToolCallInfo};
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tool: Arc<dyn Tool> = Arc::new(CapturingTool {
        descriptor: ToolDescriptor {
            name: "probe".to_string(),
            summary: "探针".to_string(),
            schema,
            access,
            approval: approval_policy,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        },
        seen: seen.clone(),
    });
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            SD::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "probe".into(),
                    args: raw_args.to_string(),
                },
            },
            SD::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            SD::Text {
                text: "done".into(),
            },
            SD::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));
    let gate = Arc::new(DangerScanGate::default());
    let rt = AgentRuntime::new(
        model,
        Arc::new(InMemoryToolCatalog::with_tools(vec![tool])),
        gate.clone(),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("system")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy {
            max_steps: Some(5),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    )
    .with_hooks(runner);
    (rt, seen, gate)
}

/// 跑一轮并返回（看到的参数、接收器）。
async fn run_probe_turn(rt: &AgentRuntime, thread: &str) -> RecordingSink {
    let sink = RecordingSink::new();
    rt.run_turn(
        TurnRequest::new(thread, mock_provider_config()).with_user_prompt("go"),
        &sink,
        &NeverCancel,
    )
    .await
    .expect("轮次应正常结束");
    sink
}

fn diag_reason(sink: &RecordingSink, point: &str) -> Option<String> {
    sink.snapshot().into_iter().find_map(|e| match e.body {
        AgentEventBody::Diagnostic(d) if d.point == point => d.reason,
        _ => None,
    })
}

/// SP-60/61：改写**生效**——工具真的拿到新参数，而且留了改写痕迹（指纹）。
#[tokio::test]
async fn test_replacement_is_applied_and_the_tool_sees_new_args() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeToolCall",
        serde_json::json!({ "cmd": "echo 更精确的路径" }),
    ));
    let (rt, seen, _gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({ "type": "object" }),
        ApprovalPolicy::Never,
        r#"{"cmd":"echo 太宽了"}"#,
    );
    let sink = run_probe_turn(&rt, "t_replace").await;

    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 1, "工具应当被执行一次：{seen:?}");
    assert_eq!(
        seen[0].get("cmd").and_then(|v| v.as_str()),
        Some("echo 更精确的路径"),
        "执行用的必须是**改写后**的参数：{seen:?}"
    );
    let replaced = sink
        .snapshot()
        .into_iter()
        .find_map(|e| match e.body {
            AgentEventBody::Diagnostic(d) if d.point == POINT_HOOK_REPLACE_APPLIED => Some(d),
            _ => None,
        })
        .expect("改写生效必须留下 hook.replace 诊断");
    assert_eq!(
        replaced.origin,
        Origin::Plugin {
            id: "fake-plugin".to_string()
        },
        "要能归因到哪个插件改的"
    );
    assert!(
        replaced.data.get("beforeHash").is_some() && replaced.data.get("afterHash").is_some(),
        "改前改后的指纹要带上：{:?}",
        replaced.data
    );
}

/// SP-61（安全序的关键一条）：**审批读的是改写后的 args**。
///
/// 用例设计成"改之前无害、改之后危险"：如果审批读的是原参数，它就永远不会被问到——
/// 那正是这条链要防的形态（插件把危险命令塞进参数、审批却按旧参数放行）。
#[tokio::test]
async fn test_approval_reads_the_rewritten_args() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeToolCall",
        serde_json::json!({ "cmd": "rm -rf /workspace" }),
    ));
    let (rt, seen, gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({ "type": "object" }),
        ApprovalPolicy::DangerScan {
            patterns: vec!["rm -rf".to_string()],
        },
        r#"{"cmd":"echo 无害"}"#,
    );
    let _sink = run_probe_turn(&rt, "t_risk_up").await;

    let asked = gate.asked.lock().expect("lock").clone();
    assert_eq!(
        asked.len(),
        1,
        "改写后的危险参数**必须**触发审批：{asked:?}"
    );
    assert!(
        asked[0].to_string().contains("rm -rf"),
        "闸门看到的必须是改写后的参数：{asked:?}"
    );
    let seen = seen.lock().expect("lock").clone();
    assert!(
        seen[0].to_string().contains("rm -rf"),
        "审批通过后执行的是改写后的参数：{seen:?}"
    );
}

/// SP-62：**风险单调性**——改写不得把"需要审批"降成"不需要"。
///
/// 这是最容易被绕过的一环：执行的是改写后的参数，所以只要允许把危险形式改成无害形式
/// 骗过审批，闸门就形同虚设。被拒时**用回原参数**，并留下理由。
#[tokio::test]
async fn test_risk_downgrade_is_rejected_and_original_args_are_used() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeToolCall",
        serde_json::json!({ "cmd": "echo 看起来人畜无害" }),
    ));
    let (rt, seen, gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({ "type": "object" }),
        ApprovalPolicy::DangerScan {
            patterns: vec!["rm -rf".to_string()],
        },
        r#"{"cmd":"rm -rf /workspace"}"#,
    );
    let sink = run_probe_turn(&rt, "t_risk_down").await;

    let asked = gate.asked.lock().expect("lock").clone();
    assert_eq!(asked.len(), 1, "原参数该问就问：{asked:?}");
    assert!(
        asked[0].to_string().contains("rm -rf"),
        "闸门看到的应当是**原**参数：{asked:?}"
    );
    let seen = seen.lock().expect("lock").clone();
    assert!(
        seen[0].to_string().contains("rm -rf"),
        "被拒的改写不得生效，执行的必须是原参数：{seen:?}"
    );
    let reason = diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).expect("降级企图必须留痕");
    assert!(
        reason.starts_with("risk_downgrade"),
        "理由要说清是风险降级：{reason}"
    );
}

/// SP-62：改写**不得把路径挪出作用域**——越界路径必须被重新校验拦下。
#[tokio::test]
async fn test_out_of_scope_path_in_replacement_is_rejected() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeToolCall",
        serde_json::json!({ "path": "/etc/passwd" }),
    ));
    let (rt, seen, _gate) = runtime_with_capturing_probe(
        runner,
        Access::Mutates {
            paths: PathSelector::Single("path"),
        },
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
        ApprovalPolicy::Never,
        r#"{"path":"ws/a.txt"}"#,
    );
    let sink = run_probe_turn(&rt, "t_path").await;

    let seen = seen.lock().expect("lock").clone();
    assert_eq!(
        seen[0].get("path").and_then(|v| v.as_str()),
        Some("ws/a.txt"),
        "越界改写必须被拒、用回原参数：{seen:?}"
    );
    let reason = diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).expect("越界改写必须留痕");
    assert!(reason.contains("path_out_of_scope"), "{reason}");
}

/// SP-62：改写不得让参数**脱离工具契约**（schema 由描述符声明，插件改不动它）。
#[tokio::test]
async fn test_schema_mismatch_in_replacement_is_rejected() {
    let runner = Arc::new(
        ScriptedHookRunner::new()
            .replace_at("beforeToolCall", serde_json::json!({ "count": "不是数字" })),
    );
    let (rt, seen, _gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
            "required": ["count"]
        }),
        ApprovalPolicy::Never,
        r#"{"count":1}"#,
    );
    let sink = run_probe_turn(&rt, "t_schema").await;

    let seen = seen.lock().expect("lock").clone();
    assert_eq!(
        seen[0].get("count").and_then(|v| v.as_i64()),
        Some(1),
        "{seen:?}"
    );
    assert_eq!(
        diag_reason(&sink, POINT_HOOK_REPLACE_REJECTED).as_deref(),
        Some("schema_mismatch"),
        "schema 不符必须留痕"
    );
}

/// SP-62：**描述符字段不可改**——载荷只带 args，工具名与读写性都改不动。
#[tokio::test]
async fn test_replacement_cannot_change_the_tool_identity() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeToolCall",
        serde_json::json!({ "tool": "read_file", "cmd": "echo hi" }),
    ));
    let (rt, seen, _gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({ "type": "object" }),
        ApprovalPolicy::Never,
        r#"{"cmd":"echo 原样"}"#,
    );
    let sink = run_probe_turn(&rt, "t_identity").await;

    let started: Vec<String> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            AgentEventBody::ToolCallStarted { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec!["probe".to_string()],
        "工具名改不动：{started:?}"
    );
    assert_eq!(seen.lock().expect("lock").len(), 1);
}

/// 没有插件订阅（或插件只观察）时，改写路径不许影响任何东西。
#[tokio::test]
async fn test_no_replacement_path_when_nothing_subscribes() {
    let runner = Arc::new(ScriptedHookRunner::new().unsubscribed());
    let (rt, seen, _gate) = runtime_with_capturing_probe(
        runner,
        Access::ReadOnly,
        serde_json::json!({ "type": "object" }),
        ApprovalPolicy::Never,
        r#"{"cmd":"原样"}"#,
    );
    let sink = run_probe_turn(&rt, "t_nosub").await;
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen[0].get("cmd").and_then(|v| v.as_str()), Some("原样"));
    assert!(
        !sink.snapshot().iter().any(|e| matches!(
            &e.body,
            AgentEventBody::Diagnostic(d)
                if d.point == POINT_HOOK_REPLACE_APPLIED
                    || d.point == POINT_HOOK_REPLACE_REJECTED
        )),
        "没订阅就没有改写这回事"
    );
}

#[tokio::test]
async fn test_turn_completes_without_tools() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Thinking {
            thinking: "思考中...".into(),
        },
        StreamDelta::Text {
            text: "你好！".into(),
        },
        StreamDelta::Done {
            stop_reason: "stop".into(),
        },
    ]]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store.clone(),
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config()).with_user_prompt("嗨");

    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
    assert_eq!(outcome.steps_taken, 1);

    // 验证事件序列（只看对话面）
    let kinds = conversational_kinds(&sink);
    assert_eq!(
        kinds,
        vec![
            "turn.started",
            "thinking.delta",
            "text.delta",
            "usage",
            "turn.finished"
        ]
    );

    // 验证恰好一个 TurnFinished
    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);

    // 验证事件单调有序
    let events = sink.snapshot();
    for i in 1..events.len() {
        assert_eq!(events[i].seq, events[i - 1].seq + 1);
    }

    // 验证消息记录落库：用户消息 + Assistant 消息
    let messages = store.get("t1");
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], AgentMessage::User { .. }));
    assert!(matches!(messages[1], AgentMessage::Assistant { .. }));
}

/// 只看**对话面**的事件名序列。
///
/// 诊断遥测（P0 埋点）刻意不属于这条序列：把它混进来会让"这一轮到底发了什么"
/// 变成噪音。它自己另有断言（见 `test_diagnostics_carry_trace_and_stay_out_of_conversation`），
/// 所以这里过滤掉**不等于**放过它。
fn conversational_kinds(sink: &RecordingSink) -> Vec<&'static str> {
    sink.snapshot()
        .into_iter()
        .filter(|e| !matches!(e.body, AgentEventBody::Diagnostic(_)))
        .map(|e| e.body.kind())
        .collect()
}

#[tokio::test]
async fn test_turn_aborted_before_start() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));
    let model = Arc::new(ScriptedModelClient::new(vec![]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = ManualCancel::new();
    cancel.cancel(); // 启动前即取消

    let req = TurnRequest::new("t1", mock_provider_config()).with_user_prompt("嗨");
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::Aborted);
    assert_eq!(outcome.steps_taken, 0);

    let kinds = conversational_kinds(&sink);
    assert_eq!(kinds, vec!["turn.started", "turn.finished"]);

    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);
}

#[tokio::test]
async fn test_turn_aborted_during_stream() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    // 模型在流结束时自报 aborted
    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Thinking {
            thinking: "想...".into(),
        },
        StreamDelta::Done {
            stop_reason: "aborted".into(),
        },
    ]]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::Aborted);

    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);
}

#[tokio::test]
async fn test_turn_model_error_yields_single_finished() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let model = Arc::new(ScriptedModelClient::new(vec![vec![StreamDelta::Error {
        error: "连接超时 504".into(),
    }]]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::ModelError);

    let kinds = conversational_kinds(&sink);
    assert_eq!(kinds, vec!["turn.started", "failed", "turn.finished"]);

    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);
}

/// SP-43：`prompt.composed` 必须**按片段**报出来（"这段是谁加的、多少字节"）。
///
/// `FixedPrompt` 是不细分的替身，所以这里自造一个会逐段报告的源——
/// 否则这条用例只能证明"有 segments 字段"，证明不了"片段真的被报出来"。
#[tokio::test]
async fn test_prompt_composed_diagnostic_reports_segments() {
    use crate::ports::{PromptSegment, PromptSource};

    struct SegmentedPrompt;
    impl PromptSource for SegmentedPrompt {
        fn system_prompt(&self) -> String {
            "AB".to_string()
        }
        fn prompt_segments(&self) -> Vec<PromptSegment> {
            vec![
                PromptSegment {
                    id: "persona".to_string(),
                    source: "builtin:core".to_string(),
                    bytes: 1,
                },
                PromptSegment {
                    id: "extra".to_string(),
                    source: "plugin:p1".to_string(),
                    bytes: 1,
                },
            ]
        }
    }

    let runtime = AgentRuntime::new(
        Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "ok".into() },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ]])),
        Arc::new(InMemoryToolCatalog::new()),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(SegmentedPrompt),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    runtime
        .run_turn(
            TurnRequest::new("t1", mock_provider_config()),
            &sink,
            &NeverCancel,
        )
        .await
        .unwrap();

    let composed = sink
        .snapshot()
        .into_iter()
        .find_map(|e| match e.body {
            AgentEventBody::Diagnostic(d) if d.point == POINT_PROMPT_COMPOSED => Some(d),
            _ => None,
        })
        .expect("每轮必须发 prompt.composed");
    assert_eq!(composed.bytes, Some(2), "总字节要如实上报");
    let segments = composed
        .data
        .get("segments")
        .and_then(|v| v.as_array())
        .expect("要带 segments");
    assert_eq!(segments.len(), 2, "{segments:?}");
    assert_eq!(segments[0]["id"], "persona");
    assert_eq!(segments[1]["source"], "plugin:p1", "来源要能看出是插件加的");
    assert_eq!(composed.data["totalBytes"], serde_json::json!(2));
}

/// SP-08 契约（P0 埋点地基）：诊断遥测**真的被发出来、带得上关联键、且不进对话序列**。
///
/// 守三件事：
/// 1. 每轮**必须有** `prompt.composed`（否则"提示词组装"这个扩展点等于没有观测）；
/// 2. 全部事件携带**同一个 `turn_id`**，工具事件还带 `call_id` 与来源（归因可用）；
/// 3. 诊断不进对话面序列——它是给人排查用的，不是对话的一部分。
#[tokio::test]
async fn test_diagnostics_carry_trace_and_stay_out_of_conversation() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
    let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 1010, 1020)));
    catalog.register(tool);

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "call_1".into(),
                    name: "echo".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "完成".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );
    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    runtime
        .run_turn(
            TurnRequest::new("t1", mock_provider_config()),
            &sink,
            &cancel,
        )
        .await
        .unwrap();

    let events = sink.snapshot();

    // 1. 提示词组装的埋点必须在（每轮一次），且如实上报字节数
    let composed = events
        .iter()
        .find_map(|e| match &e.body {
            AgentEventBody::Diagnostic(d) if d.point == POINT_PROMPT_COMPOSED => Some(d),
            _ => None,
        })
        .expect("每轮必须发 `prompt.composed`——否则提示词组装这个扩展点没有观测");
    assert_eq!(composed.family, DiagFamily::Prompt);
    assert_eq!(composed.origin, Origin::Engine);
    assert!(composed.bytes.unwrap_or(0) > 0, "提示词长度必须如实上报");

    // 2. 关联键：一轮里所有事件共享同一个 turn_id；工具事件带 call_id 与来源
    let turn_ids: Vec<&str> = events
        .iter()
        .filter_map(|e| e.trace.turn_id.as_deref())
        .collect();
    assert_eq!(
        turn_ids.len(),
        events.len(),
        "引擎发出的每个事件都必须带 turn_id"
    );
    assert!(
        turn_ids.iter().all(|t| *t == turn_ids[0]),
        "同一轮不得出现两个 turn_id：{turn_ids:?}"
    );

    let started = events
        .iter()
        .find(|e| matches!(e.body, AgentEventBody::ToolCallStarted { .. }))
        .expect("这一轮应当有一次工具调用");
    assert_eq!(started.trace.call_id.as_deref(), Some("call_1"));
    assert_eq!(
        started.trace.origin,
        Origin::Builtin {
            name: "echo".into()
        }
    );
    assert!(started.trace.step_index >= 1, "工具调用发生在第 1 步或之后");

    // 3. 诊断不进对话面
    let conv = conversational_kinds(&sink);
    assert!(
        !conv.contains(&"diagnostic"),
        "诊断不得混进对话序列：{conv:?}"
    );
    assert!(conv.contains(&"tool.started") && conv.contains(&"tool.finished"));
}

/// 归因复位：工具调用结束后，后续事件不应继续挂着那个 `call_id`。
#[tokio::test]
async fn test_trace_call_id_is_cleared_after_tool_call() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
    catalog.register(Arc::new(MockTool::new(
        desc,
        ToolReceipt::success("ok", 0, 0),
    )));

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "call_9".into(),
                    name: "echo".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "完成".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );
    let sink = RecordingSink::new();
    runtime
        .run_turn(
            TurnRequest::new("t1", mock_provider_config()),
            &sink,
            &NeverCancel,
        )
        .await
        .unwrap();

    let finished = sink
        .snapshot()
        .into_iter()
        .find(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .expect("必须有收尾事件");
    assert_eq!(
        finished.trace.call_id, None,
        "工具调用结束后 call_id 必须复位，否则后续事件的归因是错的"
    );
    assert_eq!(finished.trace.origin, Origin::Engine);
}

#[tokio::test]
async fn test_tool_call_started_before_finished_and_multi_turn() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    // 注册一个简单工具
    let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
    let receipt = ToolReceipt::success("echoed_result", 1010, 1020);
    let tool = Arc::new(MockTool::new(desc, receipt));
    catalog.register(tool);

    // 轮次 1：模型发起工具调用；轮次 2：收到工具回执后完成
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "call_1".into(),
                    name: "echo".into(),
                    args: "{\"arg\":\"hello\"}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "工具执行成功！".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store.clone(),
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
    assert_eq!(outcome.steps_taken, 2);

    // 断言 ToolCallStarted 先于 ToolCallFinished
    let kinds = sink.kinds();
    let started_pos = kinds.iter().position(|k| *k == "tool.started").unwrap();
    let finished_pos = kinds.iter().position(|k| *k == "tool.finished").unwrap();
    assert!(started_pos < finished_pos);

    // 恰好一个收尾
    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);

    // 消息中包含了 ToolResult
    let messages = store.get("t1");
    assert_eq!(messages.len(), 3); // assistant(call) -> tool_result -> assistant(text)
    assert!(matches!(messages[1], AgentMessage::ToolResult { .. }));
}

/// 守门：**上下文撞顶必须提前失败**，而不是发一个注定被拒的请求。
///
/// 实测事故：445k tokens 的请求被网关在流式过程中掐断，报告只写了一半，
/// 界面只看到笼统的"模型调用失败"。这里用"上一轮如实报回的用量 + 登记的窗口"
/// 在模型调用**之前**判定：撞顶就 `ContextWindowExceeded` 收尾，且不再发起第 2 次调用。
#[tokio::test]
async fn test_context_window_guard_stops_before_over_limit_request() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let desc = make_test_descriptor("echo", ApprovalPolicy::Never, Termination::ContinueTurn);
    let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("echoed", 0, 0)));
    catalog.register(tool);

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "call_ctx".into(),
                    name: "echo".into(),
                    args: "{}".into(),
                },
            },
            // 上一轮的真实用量：445k（远超 128k 窗口）
            StreamDelta::Usage {
                usage: TokenUsage {
                    prompt_tokens: 445_080,
                    completion_tokens: 557,
                    total_tokens: 445_637,
                    ..Default::default()
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "不该走到这里".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );
    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t_ctx", mock_provider_config()).with_context_window(Some(128_000));

    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(
        outcome.stop_reason,
        TurnStopReason::ContextWindowExceeded {
            prompt_tokens: 445_637,
            window: 128_000
        },
        "必须如实自报是上下文撞顶（不是笼统的模型错误）"
    );
    assert_eq!(outcome.steps_taken, 1, "只跑了第 1 轮，第 2 轮不该被发起");

    // 撞顶由**停止原因**承载（不再额外发 Failed：那是"笼统一句模型调用失败"的老毛病）
    let events = sink.snapshot();
    assert!(
        !events
            .iter()
            .any(|e| matches!(&e.body, AgentEventBody::Failed { .. })),
        "撞顶不该再发 Failed——理由由 TurnStopReason 承载，调度层据此自动压缩续跑"
    );
    assert!(
            !events.iter().any(|e| matches!(&e.body, AgentEventBody::TextDelta { text } if text.contains("不该走到这里"))),
            "撞顶后不得再发起模型调用"
        );
}

#[tokio::test]
async fn test_approval_denied_returns_as_tool_result_without_throwing() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());

    // 注册必须审批的工具，审批闸门配置为拒绝
    let desc = make_test_descriptor(
        "dangerous_cmd",
        ApprovalPolicy::Always,
        Termination::ContinueTurn,
    );
    let tool = Arc::new(MockTool::new(desc, ToolReceipt::success("ok", 0, 0)));
    catalog.register(tool);

    let approval = Arc::new(RecordingApprovalGate::new(false)); // 拒绝

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "call_dang".into(),
                    name: "dangerous_cmd".into(),
                    args: "{\"arg\":\"rm -rf\"}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "操作被拒绝，放弃执行。".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval.clone(),
        store.clone(),
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(outcome.stop_reason, TurnStopReason::Completed);

    // 断言审批闸门被调了一次
    assert_eq!(approval.recorded_calls().len(), 1);
    assert_eq!(approval.recorded_calls()[0].tool, "dangerous_cmd");

    // 断言 Denied 作为工具结果返回给模型
    let messages = store.get("t1");
    assert_eq!(messages.len(), 3);
    match &messages[1] {
        AgentMessage::ToolResult {
            is_error, status, ..
        } => {
            assert_eq!(is_error, &Some(true));
            assert_eq!(status.as_deref(), Some("denied"));
        }
        _ => panic!("必须为 ToolResult"),
    }
}

#[tokio::test]
async fn test_termination_end_turn_stops_entire_turn() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    // 声明 Termination::EndTurn 的工具（例如 finish 或 terminate）
    let desc = make_test_descriptor("finish", ApprovalPolicy::Never, Termination::EndTurn);
    let tool = Arc::new(MockTool::new(
        desc,
        ToolReceipt::success("任务已结束", 1000, 1005),
    ));
    catalog.register(tool);

    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::ToolCall {
            call: ToolCallInfo {
                id: "call_fin".into(),
                name: "finish".into(),
                args: "{}".into(),
            },
        },
        StreamDelta::Done {
            stop_reason: "tool_calls".into(),
        },
    ]]));

    let runtime = AgentRuntime::new(
        model,
        catalog,
        approval,
        store,
        prompt,
        scope,
        clock,
        RunPolicy::default(),
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    // 步数为 1，直接完成整轮
    assert_eq!(outcome.stop_reason, TurnStopReason::Completed);
    assert_eq!(outcome.steps_taken, 1);

    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);
}

#[tokio::test]
async fn test_budget_exhausted_reported_explicitly() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));
    let catalog = Arc::new(InMemoryToolCatalog::new());
    let approval = Arc::new(RecordingApprovalGate::new(true));

    let desc = make_test_descriptor(
        "step_tool",
        ApprovalPolicy::Never,
        Termination::ContinueTurn,
    );
    catalog.register(Arc::new(MockTool::new(
        desc,
        ToolReceipt::success("ok", 0, 0),
    )));

    // 持续返回工具调用
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "step_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c2".into(),
                    name: "step_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
    ]));

    let policy = RunPolicy {
        max_steps: Some(1), // 预算上限设为 1 步
        max_parallel_tools: 1,
        tool_timeout: None,
    };

    let runtime = AgentRuntime::new(
        model, catalog, approval, store, prompt, scope, clock, policy,
    );

    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new("t1", mock_provider_config());
    let outcome = runtime.run_turn(req, &sink, &cancel).await.unwrap();

    assert_eq!(
        outcome.stop_reason,
        TurnStopReason::BudgetExhausted { limit_steps: 1 }
    );
    // W3-T5 补强：`steps_taken` 必须等于**实际执行**的步数。
    // 原来这里只断言 stop_reason，于是 off-by-one（报 2 而非 1）长期无人发现。
    assert_eq!(
        outcome.steps_taken, 1,
        "max_steps=1 时只应执行 1 步；被预算拒绝的那次尝试不算执行"
    );

    let turn_finished_count = sink
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e.body, AgentEventBody::TurnFinished { .. }))
        .count();
    assert_eq!(turn_finished_count, 1);
}

/// **设置项 → 引擎 → 适配器**的契约：`TurnRequest::with_max_retries` 必须原样出现在    /// 模型请求的 `options.max_retries` 里（界面上的"请求重试次数"就是靠这条路生效的）。
///
/// 不写这条就会出现"设置项存下来了、界面也显示 5，但真的发请求时还是写死的 3"——
/// 正是本仓反复出现的"看起来装上了其实没接线"。
#[tokio::test]
async fn test_max_retries_reaches_the_model_request() {
    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "ok".into() },
        StreamDelta::Done {
            stop_reason: "stop".into(),
        },
    ]]));
    let runtime = AgentRuntime::new(
        model.clone(),
        Arc::new(InMemoryToolCatalog::new()),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("system prompt")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy {
            max_steps: Some(1),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    );

    // 1) 显式设置 7 次 → 必须原样送达
    let sink = RecordingSink::new();
    let req = TurnRequest::new("t_retry_setting", mock_provider_config())
        .with_user_prompt("go")
        .with_max_retries(Some(7));
    runtime.run_turn(req, &sink, &NeverCancel).await.unwrap();

    let seen = model.requests();
    assert_eq!(seen.len(), 1, "一轮只该有一次模型请求");
    assert_eq!(
        seen[0].options.max_retries,
        Some(7),
        "界面设置的重试次数必须原样送到适配器"
    );

    // 2) 不设置 → 落到默认值（既不是 0，也不是早先写死的 3）
    let model2 = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "ok".into() },
        StreamDelta::Done {
            stop_reason: "stop".into(),
        },
    ]]));
    let runtime2 = AgentRuntime::new(
        model2.clone(),
        Arc::new(InMemoryToolCatalog::new()),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("system prompt")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy {
            max_steps: Some(1),
            max_parallel_tools: 1,
            tool_timeout: None,
        },
    );
    let sink2 = RecordingSink::new();
    runtime2
        .run_turn(
            TurnRequest::new("t_retry_default", mock_provider_config()).with_user_prompt("go"),
            &sink2,
            &NeverCancel,
        )
        .await
        .unwrap();
    assert_eq!(
        model2.requests()[0].options.max_retries,
        Some(crate::model::DEFAULT_MODEL_RETRY_COUNT),
        "未设置时应当用默认重试次数"
    );
}

/// 一个**永远不返回**的工具，必须被策略超时（`RunPolicy::tool_timeout`）切断，
/// 而不是把整轮永远挂住——这正是"工具卡住 → 智能体停止"那条症状的兜底。
struct HangingTool {
    descriptor: ToolDescriptor,
}

impl Tool for HangingTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn execute<'a>(
        &'a self,
        _call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> crate::ports::BoxFuture<'a, ToolReceipt> {
        Box::pin(async {
            // 永不返回：没有引擎侧超时就等于整轮死等
            std::future::pending::<()>().await;
            unreachable!()
        })
    }
}

/// 声明"我合法地长时间运行"的工具必须**不受**策略超时约束。
struct LongRunningHangingTool {
    descriptor: ToolDescriptor,
}

impl Tool for LongRunningHangingTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn long_running(&self) -> bool {
        true
    }
    fn execute<'a>(
        &'a self,
        _call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> crate::ports::BoxFuture<'a, ToolReceipt> {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            ToolReceipt::success("慢但合法", 0, 1)
        })
    }
}

#[tokio::test]
async fn test_tool_timeout_cuts_a_hanging_tool_and_turn_continues() {
    let clock = Arc::new(FixedClock::new(1000));
    let store = Arc::new(InMemorySessionStore::new());
    let prompt = Arc::new(FixedPrompt::new("system prompt"));
    let scope = Arc::new(MockScope::new("ws"));

    let hang = Arc::new(HangingTool {
        descriptor: make_test_descriptor(
            "hang_tool",
            ApprovalPolicy::Never,
            Termination::ContinueTurn,
        ),
    });
    // 一个"合法的慢工具"：同样耗时，但声明豁免 → 不该被超时掐掉
    let slow = Arc::new(LongRunningHangingTool {
        descriptor: make_test_descriptor(
            "slow_tool",
            ApprovalPolicy::Never,
            Termination::ContinueTurn,
        ),
    });
    let catalog = Arc::new(InMemoryToolCatalog::with_tools(vec![hang, slow]));
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "hang_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c2".into(),
                    name: "slow_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        vec![
            StreamDelta::Text {
                text: "done".into(),
            },
            StreamDelta::Done {
                stop_reason: "stop".into(),
            },
        ],
    ]));

    let rt = AgentRuntime::new(
        model,
        catalog,
        Arc::new(RecordingApprovalGate::new(true)),
        store,
        prompt,
        scope,
        clock,
        RunPolicy {
            max_steps: Some(3),
            max_parallel_tools: 1,
            tool_timeout: Some(std::time::Duration::from_millis(120)),
        },
    );

    let sink = RecordingSink::new();
    let started = std::time::Instant::now();
    let outcome = rt
        .run_turn(
            TurnRequest::new("t_tool_timeout", mock_provider_config()).with_user_prompt("go"),
            &sink,
            &NeverCancel,
        )
        .await
        .expect("超时必须被兜住，整轮照常收尾");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "挂住的工具必须被超时切断（实际用了 {elapsed:?}）"
    );
    assert_eq!(
        outcome.stop_reason,
        TurnStopReason::Completed,
        "超时后整轮应继续跑完"
    );

    let receipts: Vec<ToolReceipt> = sink
        .snapshot()
        .into_iter()
        .filter_map(|e| match e.body {
            AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt),
            _ => None,
        })
        .collect();
    let hang_receipt = receipts
        .iter()
        .find(|r| r.output.contains("超过"))
        .expect("挂住的工具必须留下一条超时回执（否则界面只有一张永远转圈的卡片）");
    assert_eq!(hang_receipt.status, ToolStatus::Timeout);
    assert!(
        hang_receipt.output.contains("hang_tool"),
        "超时回执要说清是哪个工具：{}",
        hang_receipt.output
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.status == ToolStatus::Success && r.output.contains("慢但合法")),
        "声明 long_running 的慢工具不该被策略超时掐掉"
    );
}
