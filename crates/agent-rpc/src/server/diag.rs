//! 诊断遥测的**唯一投影点**（P0 埋点地基）。
//!
//! 两条来源都走这里：
//! 1. **轮次内**：引擎的 `Diagnostic` 事件经 dispatch 的事件循环到达（带轮次上下文）；
//! 2. **轮次外**：插件/技能的扫描——由用户动作触发，此刻没有轮次在跑（无轮次上下文）。
//!
//! 为什么只留一处：下面这些字段名（`family` / `point` / `origin` / `outcome` /
//! `durationMs` / `bytes` / `turnId`）就是界面过滤与展示的依据。两处各写一份必然分叉，
//! 而分叉的表现是"面板上少一列"——不会有任何测试变红，只会有人纳闷为什么过滤不出来。
//!
//! 内容字段一律截断：诊断是要**随快照跨端传输**的数据，一条不封顶的理由串就能把长文案
//! 整段搬进界面缓冲（§2.2 第 3 条律：默认只记元数据，内容按需且必须有界）。

use agent_base::domain::{DiagnosticEvent, TraceCtx};

use crate::state::AgentStore;

/// 单条诊断**内容字段**的上限（字符数）。
pub const DIAG_TEXT_LIMIT: usize = 200;

/// 单条诊断 `data` 的上限（字节）。
///
/// `data` 由各发射方自填，将来很可能有人把整段内容塞进去。这里给一道**有界**的闸：
/// 超了就只留摘要 + 预览，并**如实标注**原始体积。
pub const DIAG_DATA_LIMIT: usize = 4096;

/// 超长内容截断并**如实标注**（不要静默切掉：看的人得知道这段是被截过的）。
pub fn truncate_for_diag(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{cut}…（已截断）")
}

/// `data` 限幅（同上：截断要标注，不能静默）。
fn cap_data(data: &serde_json::Value, limit: usize) -> serde_json::Value {
    let encoded = data.to_string();
    if encoded.len() <= limit {
        return data.clone();
    }
    serde_json::json!({
        "摘要": format!("data 过大已截断（原始 {} 字节）", encoded.len()),
        "预览": encoded.chars().take(400).collect::<String>(),
    })
}

/// 把一条诊断写进诊断缓冲（`AgentStore.log`），返回给人看的那一行。
pub fn project_diagnostic(
    store: &mut AgentStore,
    diag: &DiagnosticEvent,
    trace: &TraceCtx,
) -> String {
    let origin = diag.origin.label();
    let outcome = diag.outcome.map(|o| o.label()).unwrap_or("-");
    let reason = diag
        .reason
        .as_deref()
        .map(|r| truncate_for_diag(r, DIAG_TEXT_LIMIT));

    let mut parts = vec![format!(
        "[diag] {} {} ← {}",
        diag.family.label(),
        diag.point,
        origin
    )];
    parts.push(format!("outcome={outcome}"));
    if let Some(ms) = diag.duration_ms {
        parts.push(format!("{ms}ms"));
    }
    if let Some(b) = diag.bytes {
        parts.push(format!("{b}B"));
    }
    if let Some(ref r) = reason {
        parts.push(format!("reason={r}"));
    }
    let text = parts.join(" · ");

    let payload = serde_json::json!({
        "family": diag.family.label(),
        "point": diag.point,
        "origin": origin,
        "outcome": diag.outcome.map(|o| o.label()),
        "durationMs": diag.duration_ms,
        "bytes": diag.bytes,
        "reason": reason,
        "turnId": trace.turn_id,
        "stepIndex": trace.step_index,
        "callId": trace.call_id,
        "data": cap_data(&diag.data, DIAG_DATA_LIMIT),
    });
    store.push_log("diag", text.clone(), Some(payload));
    text
}

/// 轮次外的诊断（扫描类）：没有轮次上下文，关联键如实留空。
pub fn project_out_of_turn(store: &mut AgentStore, diag: &DiagnosticEvent) -> String {
    project_diagnostic(store, diag, &TraceCtx::none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_marks_the_cut_instead_of_hiding_it() {
        assert_eq!(truncate_for_diag("短", 10), "短");
        let long = "字".repeat(30);
        let cut = truncate_for_diag(&long, 10);
        assert!(cut.starts_with(&"字".repeat(10)));
        assert!(cut.contains("已截断"), "截断必须如实标注：{cut}");
    }

    #[test]
    fn test_cap_data_bounds_the_payload_but_keeps_a_preview() {
        let small = serde_json::json!({ "a": 1 });
        assert_eq!(cap_data(&small, DIAG_DATA_LIMIT), small, "小 data 不该被动");

        let big = serde_json::json!({ "text": "x".repeat(DIAG_DATA_LIMIT * 2) });
        let capped = cap_data(&big, DIAG_DATA_LIMIT);
        assert!(capped.get("摘要").is_some(), "超限要给摘要：{capped}");
        assert!(capped.get("预览").is_some(), "超限要留预览：{capped}");
        assert!(
            capped.to_string().len() < DIAG_DATA_LIMIT,
            "限幅后必须真的变小：{}",
            capped.to_string().len()
        );
    }
}
