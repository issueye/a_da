//! `plugins/ask-user` 的端到端契约：工具写问题 → 界面动作读取 → 作答动作唤醒工具。

use std::path::PathBuf;
use std::time::Duration;

use agent_node::plugins::PluginSandbox;

fn repo_plugin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/ask-user/index.ts")
}

fn temp_ws(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "a_da_ask_user_{tag}_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("建临时工作区失败");
    dir
}

async fn list_questions(ws: &std::path::Path) -> Vec<serde_json::Value> {
    let result = PluginSandbox::call_action_budgeted(
        &repo_plugin(),
        "list",
        serde_json::json!({ "threadId": "thread-1" }),
        serde_json::json!({ "workspace": ws.to_string_lossy() }),
        "read",
        ws,
        10,
        ada_ts_engine::Needs::plugin_default(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("list action should run");
    result
        .get("data")
        .and_then(|v| v.get("questions"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn ask_user_plugin_round_trips_through_the_ui_action() {
    let plugin = repo_plugin();
    let ws = temp_ws("roundtrip");

    let inspected = PluginSandbox::inspect_plugin_full(
        &plugin,
        &ws,
        ada_ts_engine::Needs::plugin_default(),
    )
    .await
    .expect("ask-user plugin should inspect");
    assert_eq!(inspected.tools.len(), 1, "插件应提供一个工具");
    assert_eq!(inspected.tools[0].name, "ask_user");
    assert!(inspected.tools[0].long_running, "提问必须声明 longRunning");
    let action_names: Vec<&str> = inspected.actions.iter().map(|a| a.name.as_str()).collect();
    assert!(action_names.contains(&"list"));
    assert!(action_names.contains(&"answer"));

    let ws_for_tool = ws.clone();
    let tool = tokio::spawn(async move {
        PluginSandbox::call_tool_budgeted(
            &repo_plugin(),
            "ask_user",
            serde_json::json!({
                "question": "选哪个方案？",
                "choices": [
                    { "id": "c1", "label": "方案 A" },
                    { "id": "c2", "label": "方案 B" }
                ],
                "allow_text": true
            }),
            &ws_for_tool,
            Some("thread-1"),
            30,
            ada_ts_engine::Needs::plugin_default(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("ask_user tool should wait and then return")
    });

    let mut question = None;
    for _ in 0..50 {
        let pending = list_questions(&ws).await;
        if let Some(item) = pending.first() {
            question = Some(item.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let question = question.expect("待答提问应通过 list 动作出现");
    let question_id = question["id"].as_str().expect("question id").to_string();
    assert_eq!(question["items"][0]["question"], "选哪个方案？");

    let answered = PluginSandbox::call_action_budgeted(
        &repo_plugin(),
        "answer",
        serde_json::json!({
            "question_id": question_id,
            "thread_id": "thread-1",
            "choice__q1": "c2",
            "text__q1": "按 B 做"
        }),
        serde_json::json!({ "workspace": ws.to_string_lossy() }),
        "write",
        &ws,
        10,
        ada_ts_engine::Needs::plugin_default(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("answer action should run");
    assert_eq!(answered["ok"], true, "{answered}");

    let receipt = tokio::time::timeout(Duration::from_secs(5), tool)
        .await
        .expect("作答后工具应被唤醒")
        .expect("tool task should not panic");
    assert!(receipt.ok, "工具应成功返回：{receipt:?}");
    assert!(receipt.output.contains("方案 B"), "{}", receipt.output);
    assert!(receipt.output.contains("按 B 做"), "{}", receipt.output);

    assert!(
        list_questions(&ws).await.is_empty(),
        "作答后 list 不应再返回待答项"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn ask_user_plugin_reports_invalid_args_instead_of_hanging() {
    let ws = temp_ws("invalid");
    let receipt = PluginSandbox::call_tool_budgeted(
        &repo_plugin(),
        "ask_user",
        serde_json::json!({ "question": "" }),
        &ws,
        Some("thread-1"),
        10,
        ada_ts_engine::Needs::plugin_default(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("tool should return a receipt");
    assert!(!receipt.ok);
    assert!(receipt.output.contains("缺少 question"), "{}", receipt.output);
    let _ = std::fs::remove_dir_all(&ws);
}
