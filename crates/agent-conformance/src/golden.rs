//! GOLDEN 回放夹具（W3-T5 / 计划 M0-T3 补课）。
//!
//! # 为什么需要它
//!
//! W3-T2 要把产品从 legacy 主循环切到 `AgentRuntime::run_turn`，而计划对这一步的
//! 硬要求是「**每切一处都要能用 golden 回放证明事件序列等价**」。
//! 在切换之前先有这把尺子，切换才有判据；否则只能靠"跑起来看着像对的"。
//!
//! # 夹具的形态
//!
//! 一份夹具 = **脚本化模型 + 产品声明 + 冻结的事件序列**：
//!
//! ```json
//! {
//!   "name": "text_only",
//!   "spec": { ...agent.spec.json 的内容... },
//!   "threadId": "golden_text_only",
//!   "provider": { ... },
//!   "script": [ [ {"type":"text","text":"..."}, {"type":"done","stopReason":"stop"} ] ],
//!   "expected": {
//!     "stepsTaken": 1,
//!     "stopReason": "completed",
//!     "events": [ { "seq": 1, "kind": "turn.started" }, ... ]
//!   }
//! }
//! ```
//!
//! # 冻结的粒度（刻意如此）
//!
//! | 字段 | 是否冻结 | 理由 |
//! |---|---|---|
//! | `seq` | ✅ | INV-6：单调递增是硬契约，必须钉住 |
//! | `kind` | ✅ | 事件种类序列就是"等价"的定义 |
//! | 文本 / 工具名 / 停止原因 | ✅ | 用户可见的实质内容 |
//! | `at_ms` | ❌ | 墙钟时间，冻结它等于让夹具对运行时刻敏感 |
//! | `total_duration_ms` | ❌ | 同上 |
//!
//! `at_ms` 虽不冻结，但会**断言非递减**（时间倒流是 bug）。
//!
//! # 夹具发现
//!
//! [`load_fixtures`] **扫描目录**（`CARGO_MANIFEST_DIR/golden/*.json`），
//! 不维护文件清单——新增夹具不需要改代码，也就不会有"加了夹具忘了登记"的静默失效。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta};
use agent_base::testing::{
    FixedClock, FixedPrompt, InMemorySessionStore, MockScope, NeverCancel, RecordingApprovalGate,
    RecordingSink, ScriptedModelClient,
};
use agent_runtime::{AgentSpec, ProductBuilder};
use serde::{Deserialize, Serialize};

/// 一份 golden 夹具。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoldenFixture {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 产品声明（`agent.spec.json` 的结构）。
    pub spec: serde_json::Value,
    pub thread_id: String,
    pub provider: ProviderConfig,
    /// 每一轮模型调用的增量脚本。
    pub script: Vec<Vec<StreamDelta>>,
    pub expected: GoldenExpectation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoldenExpectation {
    pub steps_taken: u32,
    pub stop_reason: String,
    pub events: Vec<ExpectedEvent>,
}

/// 事件的可冻结投影（见模块文档的粒度表）。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedEvent {
    pub seq: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
}

/// 一次回放的结果。
pub struct GoldenReport {
    pub steps_taken: u32,
    pub stop_reason: String,
    pub events: Vec<ExpectedEvent>,
    /// 未投影的原始事件（供时间单调性等附加断言使用）。
    pub raw: Vec<AgentEvent>,
}

/// 停止原因 → 稳定字符串（夹具里用它，避免把 Rust 枚举名冻进 JSON）。
pub fn stop_reason_name(stop: &TurnStopReason) -> String {
    match stop {
        TurnStopReason::Completed => "completed",
        TurnStopReason::Aborted => "aborted",
        TurnStopReason::ModelError => "model_error",
        TurnStopReason::BudgetExhausted { .. } => "budget_exhausted",
        TurnStopReason::Denied => "denied",
    }
    .to_string()
}

/// 领域事件 → 可冻结投影。
pub fn project_event(event: &AgentEvent) -> ExpectedEvent {
    let mut out = ExpectedEvent {
        seq: event.seq,
        kind: event.body.kind().to_string(),
        text: None,
        tool: None,
        stop: None,
    };
    match &event.body {
        AgentEventBody::TextDelta { text } | AgentEventBody::ThinkingDelta { text } => {
            out.text = Some(text.clone());
        }
        AgentEventBody::ToolCallStarted { name, .. }
        | AgentEventBody::ToolCallFinished { name, .. } => {
            out.tool = Some(name.clone());
        }
        AgentEventBody::TurnFinished { stop } => {
            out.stop = Some(stop_reason_name(stop));
        }
        _ => {}
    }
    out
}

/// 回放一份夹具（**真实引擎** `AgentRuntime::run_turn`，只把模型换成脚本）。
pub async fn replay(fixture: &GoldenFixture) -> Result<GoldenReport, String> {
    let spec_json = serde_json::to_string(&fixture.spec).map_err(|e| e.to_string())?;
    let spec = AgentSpec::from_json_str(&spec_json).map_err(|e| e.to_string())?;

    // 工具按声明装配（真源）；夹具只声明 `core` 这类无副作用的工具包。
    let tools = agent_toolkit::tools_for_toolkits(&spec.toolkits, Path::new("."))
        .map_err(|e| format!("按声明装配工具包失败：{e}"))?;

    let model = Arc::new(ScriptedModelClient::new(fixture.script.clone()));
    let builder = ProductBuilder::new(spec)
        .with_tools(tools)
        .with_model(model)
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("golden 系统提示词")))
        .with_scope(Arc::new(MockScope::new("golden_scope")))
        .with_clock(Arc::new(FixedClock::new(1_700_000_000_000)));

    let runtime = builder.build().map_err(|e| e.to_string())?;

    let sink = RecordingSink::new();
    let req = TurnRequest::new(fixture.thread_id.clone(), fixture.provider.clone());
    let outcome = runtime
        .run_turn(req, &sink, &NeverCancel)
        .await
        .map_err(|e| format!("回放执行失败：{e}"))?;

    let raw = sink.snapshot();
    Ok(GoldenReport {
        steps_taken: outcome.steps_taken,
        stop_reason: stop_reason_name(&outcome.stop_reason),
        events: raw.iter().map(project_event).collect(),
        raw,
    })
}

/// 把回放结果与冻结期望逐字段比对。
pub fn compare(fixture: &GoldenFixture, report: &GoldenReport) -> Result<(), String> {
    let mut problems = Vec::new();

    if report.steps_taken != fixture.expected.steps_taken {
        problems.push(format!(
            "步数不一致：期望 {}，实际 {}",
            fixture.expected.steps_taken, report.steps_taken
        ));
    }
    if report.stop_reason != fixture.expected.stop_reason {
        problems.push(format!(
            "停止原因不一致：期望 {}，实际 {}",
            fixture.expected.stop_reason, report.stop_reason
        ));
    }
    if report.events != fixture.expected.events {
        problems.push(format!(
            "事件序列不一致：\n  期望 {:#?}\n  实际 {:#?}",
            fixture.expected.events, report.events
        ));
    }

    // 附加不变量：时间不得倒流（at_ms 不冻结，但单调性要成立）
    for pair in report.raw.windows(2) {
        if pair[1].at_ms < pair[0].at_ms {
            problems.push(format!(
                "事件时间倒流：seq={} at_ms={} 之后出现 seq={} at_ms={}",
                pair[0].seq, pair[0].at_ms, pair[1].seq, pair[1].at_ms
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// golden 夹具目录。
pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

/// 扫描目录取出全部夹具（`(路径, 原始 JSON)`）。**不维护文件清单**。
pub fn load_fixtures() -> Result<Vec<(PathBuf, String)>, String> {
    let dir = golden_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| format!("读不到 golden 目录 {}：{e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("读不到夹具 {}：{e}", path.display()))?;
        out.push((path, raw));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    if out.is_empty() {
        return Err(format!("golden 目录 {} 里没有任何 .json 夹具", dir.display()));
    }
    Ok(out)
}

/// 解析一份夹具 JSON。
pub fn parse_fixture(raw: &str) -> Result<GoldenFixture, String> {
    serde_json::from_str(raw).map_err(|e| format!("夹具 JSON 解析失败：{e}"))
}

/// 回放并比对一份夹具。
pub async fn assert_fixture(raw: &str) -> Result<(), String> {
    let fixture = parse_fixture(raw)?;
    let report = replay(&fixture).await?;
    compare(&fixture, &report).map_err(|e| format!("夹具 `{}` 回放不一致：\n{e}", fixture.name))
}

/// 回放并比对**目录下全部**夹具，返回夹具数量。
pub async fn assert_all_fixtures() -> Result<usize, String> {
    let fixtures = load_fixtures()?;
    for (path, raw) in &fixtures {
        assert_fixture(raw)
            .await
            .map_err(|e| format!("{}：{e}", path.display()))?;
    }
    Ok(fixtures.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_SPEC: &str = r#"{
        "id": "ada-golden",
        "archetype": "coding",
        "identity": { "name": "夹具产品", "persona": "system.md", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 8, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#;

    fn provider() -> ProviderConfig {
        ProviderConfig {
            id: "golden".into(),
            name: "golden".into(),
            protocol: Default::default(),
            base_url: "http://localhost".into(),
            api_key: "k".into(),
            model: "m".into(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        }
    }

    /// W3-T5 出口判据：**目录下全部夹具**都要回放一致。
    ///
    /// 新增夹具 = 放一个 `.json` 进 `golden/`，这条测试自动覆盖它（无需改代码）。
    #[tokio::test]
    async fn test_all_golden_fixtures_replay() {
        let count = assert_all_fixtures().await.expect("全部 golden 夹具必须回放一致");
        assert!(count >= 3, "至少应有 3 份夹具，实际 {count}");
    }

    /// 事件序列**确实被冻结**了：把期望改掉一位，比对必须报错。
    ///
    /// 这条防的是"夹具形同虚设"——如果 compare 永远返回 Ok，上面那条测试毫无意义。
    #[tokio::test]
    async fn test_tampered_expectation_is_detected() {
        let (_, raw) = load_fixtures()
            .expect("夹具可读")
            .into_iter()
            .next()
            .expect("至少一份夹具");
        let mut fixture = parse_fixture(&raw).expect("夹具可解析");

        // 篡改：少一个事件
        fixture.expected.events.pop();
        let report = replay(&fixture).await.expect("回放必须成功");
        assert!(
            compare(&fixture, &report).is_err(),
            "篡改期望后比对必须失败（否则夹具没有约束力）"
        );

        // 篡改：步数
        let mut fixture2 = parse_fixture(&raw).expect("夹具可解析");
        fixture2.expected.steps_taken += 99;
        let report2 = replay(&fixture2).await.expect("回放必须成功");
        assert!(compare(&fixture2, &report2).is_err(), "步数篡改必须被检出");
    }

    /// 目录扫描是唯一的夹具发现方式：不存在"清单里漏登记"的可能。
    #[test]
    fn test_fixtures_are_discovered_by_scanning_not_by_a_list() {
        let fixtures = load_fixtures().expect("扫描夹具目录");
        assert!(fixtures.len() >= 3);
        assert!(fixtures.iter().all(|(p, _)| p.extension().unwrap() == "json"));
    }

    /// 停止原因映射是稳定的（夹具里冻的是这些字符串）。
    #[test]
    fn test_stop_reason_names_are_stable() {
        assert_eq!(stop_reason_name(&TurnStopReason::Completed), "completed");
        assert_eq!(stop_reason_name(&TurnStopReason::Aborted), "aborted");
        assert_eq!(stop_reason_name(&TurnStopReason::ModelError), "model_error");
        assert_eq!(stop_reason_name(&TurnStopReason::Denied), "denied");
        assert_eq!(
            stop_reason_name(&TurnStopReason::BudgetExhausted { limit_steps: 3 }),
            "budget_exhausted"
        );
    }

    /// 维护工具：`A_DA_GOLDEN_BLESS=1` 时用**当前引擎的真实输出**重写夹具的 `expected`。
    ///
    /// 刻意做成需要显式环境变量：默认绝不自动改写冻结期望——
    /// 否则"夹具"会退化成"把当前行为抄一遍"，失去约束力。
    /// 用法：`A_DA_GOLDEN_BLESS=1 cargo test -p agent-conformance --lib golden::tests::test_bless -- --nocapture`
    #[tokio::test]
    async fn test_bless_fixtures_when_requested() {
        if std::env::var("A_DA_GOLDEN_BLESS").is_err() {
            return;
        }
        for (path, raw) in load_fixtures().expect("夹具可读") {
            let mut fixture = parse_fixture(&raw).expect("夹具可解析");
            let report = replay(&fixture).await.expect("回放必须成功");
            fixture.expected = GoldenExpectation {
                steps_taken: report.steps_taken,
                stop_reason: report.stop_reason,
                events: report.events,
            };
            let text = serde_json::to_string_pretty(&fixture).expect("序列化夹具") + "\n";
            std::fs::write(&path, text).expect("写回夹具");
            println!("blessed {}", path.display());
        }
    }

    /// 纯文本夹具的期望**是手工可读的**：至少要有 turn.started / text.delta / turn.finished。
    #[tokio::test]
    async fn test_text_only_fixture_shape() {        let fixture = GoldenFixture {
            name: "inline".into(),
            description: "内联最小夹具".into(),
            spec: serde_json::from_str(MINIMAL_SPEC).unwrap(),
            thread_id: "inline_thread".into(),
            provider: provider(),
            script: vec![vec![
                StreamDelta::Text { text: "你好".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ]],
            expected: GoldenExpectation { steps_taken: 0, stop_reason: String::new(), events: vec![] },
        };
        let report = replay(&fixture).await.expect("回放必须成功");
        assert_eq!(report.steps_taken, 1);
        assert_eq!(report.stop_reason, "completed");
        let kinds: Vec<&str> = report.events.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"turn.started"), "{kinds:?}");
        assert!(kinds.contains(&"text.delta"), "{kinds:?}");
        assert!(kinds.contains(&"turn.finished"), "{kinds:?}");
    }
}
