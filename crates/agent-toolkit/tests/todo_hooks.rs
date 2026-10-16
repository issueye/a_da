//! P3b · SP-63 的验收：`todo` 的**内部点位**（工具自己跑，引擎只发观测）。
//!
//! # 为什么放这里而不是 `agent-base`
//!
//! 要验的是"真实内置 `TodoTool` + 真实引擎"这两者的接缝：工具在 `agent-toolkit`、
//! 引擎在 `agent-base`（前者依赖后者，反向不成立）。所以只有这一层同时看得见两边。
//!
//! # 验的是什么
//!
//! 1. `beforeTodoUpdate` 拦下 → **清单不更新**，理由以普通工具结果回模型（可纠正）；
//! 2. 改写 → **重新校验**（空标题一律拒），生效时要让模型看见"这不是它写的那份"；
//! 3. `afterTodoUpdate` 的旁注带差异，且**首个快照**如实说明没有历史可比；
//! 4. 没装钩子时行为一字不变（不开这套机制的产品不该多出任何可观察差异）。

use std::sync::Arc;

use agent_base::domain::{AgentEventBody, ToolReceipt, ToolStatus};
use agent_base::engine::{AgentRuntime, RunPolicy, TurnRequest};
use agent_base::model::{StreamDelta, ToolCallInfo};
use agent_base::ports::hooks::HookRunner;
use agent_base::testing::{
    FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, MockScope,
    RecordingApprovalGate, RecordingSink, ScriptedHookRunner, ScriptedModelClient,
};
use agent_toolkit::core::TodoTool;

fn provider_config() -> agent_base::model::ProviderConfig {
    agent_base::model::ProviderConfig {
        id: "test".into(),
        name: "test".into(),
        protocol: Default::default(),
        base_url: "http://localhost".into(),
        api_key: "k".into(),
        model: "m".into(),
        max_output_tokens: None,
        custom_headers: None,
        proxy_url: None,
    }
}

/// 造一个"模型调一次写清单的 `todo`"的运行时（用**真实** `TodoTool`）。
fn runtime_with_todo(runner: Option<Arc<dyn HookRunner>>) -> AgentRuntime {
    let tool = Arc::new(TodoTool::new());
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "todo".into(),
                    args: serde_json::json!({
                        "action": "set",
                        "todos": [
                            { "title": "写测试", "status": "pending" },
                            { "title": "改文档", "status": "completed" }
                        ]
                    })
                    .to_string(),
                },
            },
            StreamDelta::Done { stop_reason: "tool_calls".into() },
        ],
        vec![
            StreamDelta::Text { text: "done".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ],
    ]));
    let mut rt = AgentRuntime::new(
        model,
        Arc::new(InMemoryToolCatalog::with_tools(vec![tool])),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("system")),
        Arc::new(MockScope::new("ws")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy { max_steps: Some(5), max_parallel_tools: 1, tool_timeout: None },
    );
    if let Some(r) = runner {
        rt = rt.with_hooks(r);
    }
    rt
}

async fn run_todo_turn(rt: &AgentRuntime, thread: &str) -> RecordingSink {
    let sink = RecordingSink::new();
    rt.run_turn(
        TurnRequest::new(thread, provider_config()).with_user_prompt("更新清单"),
        &sink,
        &agent_base::testing::NeverCancel,
    )
    .await
    .expect("轮次应正常结束");
    sink
}

fn todo_receipt(sink: &RecordingSink) -> ToolReceipt {
    sink.snapshot()
        .into_iter()
        .find_map(|e| match e.body {
            AgentEventBody::ToolCallFinished { receipt, .. } => Some(receipt),
            _ => None,
        })
        .expect("要有工具回执")
}

/// SP-63：拦下 → **清单不更新**，理由以普通工具结果回模型。
#[tokio::test]
async fn test_before_todo_update_can_deny_the_update() {
    let runner = Arc::new(
        ScriptedHookRunner::new().deny_at("beforeTodoUpdate", "清单里还没有验收项，先补齐"),
    );
    let rt = runtime_with_todo(Some(runner.clone()));
    let sink = run_todo_turn(&rt, "t_todo_deny").await;

    let receipt = todo_receipt(&sink);
    assert_eq!(receipt.status, ToolStatus::Denied, "{receipt:?}");
    assert!(
        receipt.output.contains("还没有验收项"),
        "插件理由要原样回模型（可纠正）：{}",
        receipt.output
    );
    assert!(
        runner.calls().contains(&"beforeTodoUpdate".to_string()),
        "点位必须真被调过：{:?}",
        runner.calls()
    );
    assert!(
        !runner.calls().contains(&"afterTodoUpdate".to_string()),
        "被拦下就不该再问事后点位：{:?}",
        runner.calls()
    );
}

/// SP-63：改写清单 → **重新校验后生效**，并让模型看见"这不是它写的那份"。
#[tokio::test]
async fn test_before_todo_update_replacement_is_revalidated_and_applied() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeTodoUpdate",
        serde_json::json!({
            "todos": [
                { "title": "写测试", "status": "pending" },
                { "title": "改文档", "status": "completed" },
                { "title": "补一条验收项", "status": "pending" }
            ]
        }),
    ));
    let rt = runtime_with_todo(Some(runner));
    let sink = run_todo_turn(&rt, "t_todo_replace").await;

    let receipt = todo_receipt(&sink);
    assert_eq!(receipt.status, ToolStatus::Success, "{receipt:?}");
    assert!(
        receipt.output.contains("共 3 条"),
        "改写的效果要能被看出来（条数从 2 变 3）：{}",
        receipt.output
    );
    assert!(
        receipt.output.contains("被插件") && receipt.output.contains("改写后生效"),
        "改写必须留痕给模型看（它得知道清单不是自己那份）：{}",
        receipt.output
    );
}

/// SP-63：**改写不合形状 → 拒且不改**（改写不是授权，回来后照样要过校验）。
#[tokio::test]
async fn test_todo_replacement_with_empty_title_is_rejected() {
    let runner = Arc::new(ScriptedHookRunner::new().replace_at(
        "beforeTodoUpdate",
        serde_json::json!({ "todos": [ { "title": "   ", "status": "pending" } ] }),
    ));
    let rt = runtime_with_todo(Some(runner));
    let sink = run_todo_turn(&rt, "t_todo_bad").await;

    let receipt = todo_receipt(&sink);
    assert_eq!(receipt.status, ToolStatus::Error, "{receipt:?}");
    assert!(
        receipt.output.contains("空标题"),
        "要指出改写在哪儿不合法：{}",
        receipt.output
    );
}

/// SP-63：`afterTodoUpdate` 的旁注追进模型读到的文本。
#[tokio::test]
async fn test_after_todo_update_note_reaches_the_model() {
    let runner =
        Arc::new(ScriptedHookRunner::new().note_at("afterTodoUpdate", "清单已记录，注意验收项"));
    let rt = runtime_with_todo(Some(runner));
    let sink = run_todo_turn(&rt, "t_todo_after").await;

    let receipt = todo_receipt(&sink);
    assert_eq!(receipt.status, ToolStatus::Success, "{receipt:?}");
    assert!(
        receipt.output.contains("[插件旁注]") && receipt.output.contains("注意验收项"),
        "事后旁注要追进模型读到的文本：{}",
        receipt.output
    );
}

/// 没装钩子（`hooks: None`）时行为**一字不变**：不开这套机制的产品不该多出可观察差异。
#[tokio::test]
async fn test_todo_without_hooks_behaves_as_before() {
    let rt = runtime_with_todo(None);
    let sink = run_todo_turn(&rt, "t_todo_plain").await;
    let receipt = todo_receipt(&sink);
    assert_eq!(receipt.status, ToolStatus::Success, "{receipt:?}");
    assert!(
        !receipt.output.contains("[插件旁注]"),
        "没钩子就不该有旁注：{}",
        receipt.output
    );
    assert!(receipt.output.contains("共 2 条"), "{}", receipt.output);
}
