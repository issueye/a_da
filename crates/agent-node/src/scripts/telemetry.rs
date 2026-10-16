//! 脚本生命周期的**遥测出口**（P4 · SP-70 抽出）：两种宿主实现共用。
//!
//! # 为什么要抽出来
//!
//! 沙箱档与子进程档都要发 `script.invoked` / `script.finished`（一进一出、带耗时、
//! 控制台**只报元数据**）。各写一份的话，迟早出现"换个档位，诊断就少一个字段"
//! ——那是最难察觉的漂移：界面上看不出少了什么，只是面板变得不好用。
//!
//! 出口本身是**宿主级**的（构造时接一次，见 `agent-base::ports::scripts` 的说明）：
//! 端口不把出口塞进每次调用的上下文，因为调用方大多凑不出来（`seq` 与轮次上下文在引擎手里）。

use agent_base::domain::{DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_SCRIPT_FINISHED,
    POINT_SCRIPT_INVOKED};

use crate::scripts::DiagOutlet;

/// 一次脚本调用的观测器（一进一出成对）。
pub struct ScriptTelemetry {
    diag: Option<DiagOutlet>,
    origin: Origin,
    export: String,
}

impl ScriptTelemetry {
    pub fn new(diag: Option<DiagOutlet>, origin: Origin, export: &str) -> Self {
        Self {
            diag,
            origin,
            export: export.to_string(),
        }
    }

    fn report(&self, point: &str, outcome: Option<DiagOutcome>, ms: Option<u64>, extra: serde_json::Value) {
        let mut ev = DiagnosticEvent::new(DiagFamily::Script, point, self.origin.clone());
        if let Some(o) = outcome {
            ev = ev.with_outcome(o);
        }
        if let Some(m) = ms {
            ev = ev.with_duration_ms(m);
        }
        ev = ev.with_data(extra);
        match &self.diag {
            Some(outlet) => outlet(ev),
            // 没接出口就退回日志：**宁可只进日志，也不要伪造事件信封**
            // （`seq` 与轮次上下文在引擎手里，宿主伪造不出来）。
            None => tracing::debug!(
                target: "a_da::diag",
                "[diag] script {} ← {:?} outcome={:?} {}ms",
                ev.point,
                ev.origin,
                ev.outcome,
                ev.duration_ms.unwrap_or(0)
            ),
        }
    }

    /// 进入一次调用。
    pub fn entered(&self) {
        self.report(
            POINT_SCRIPT_INVOKED,
            None,
            None,
            serde_json::json!({ "export": self.export }),
        );
    }

    /// 收尾（**失败路径也必报**，否则面板上只留下"开始"，看不出它怎么收场的）。
    ///
    /// `console` 只报**元数据**（字节数 + 是否截断）：诊断缓冲要跨端传输，
    /// 正文塞进去会把它刷爆；控制台正文进日志。
    pub fn finished(&self, outcome: DiagOutcome, ms: u64, console: Option<serde_json::Value>) {
        let mut data = serde_json::json!({ "export": self.export });
        let mut bytes = 0u64;
        if let Some(c) = console {
            bytes = c.get("stdoutBytes").and_then(|v| v.as_u64()).unwrap_or(0)
                + c.get("stderrBytes").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(obj) = data.as_object_mut() {
                obj.insert("console".to_string(), c);
            }
        }
        self.report(POINT_SCRIPT_FINISHED, Some(outcome), Some(ms), data);
        let _ = bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::DiagCollector;
    use std::sync::Arc;

    /// 一进一出、带耗时与 export：面板据此回答"这次脚本怎么收场的"。
    #[test]
    fn test_pairs_entered_and_finished_with_metadata() {
        let collector = Arc::new(DiagCollector::new());
        let sink = collector.clone();
        let t = ScriptTelemetry::new(
            Some(Arc::new(move |d| sink.push(d))),
            Origin::Plugin {
                id: "demo".to_string(),
            },
            "web_search",
        );
        t.entered();
        t.finished(DiagOutcome::Passed, 42, None);

        let events = collector.snapshot();
        let points: Vec<&str> = events.iter().map(|e| e.point.as_str()).collect();
        assert_eq!(points, vec![POINT_SCRIPT_INVOKED, POINT_SCRIPT_FINISHED]);
        assert_eq!(events[1].duration_ms, Some(42), "耗时必须如实带上");
        assert_eq!(
            events[1].data.get("export").and_then(|v| v.as_str()),
            Some("web_search")
        );
        assert!(
            matches!(&events[1].origin, Origin::Plugin { id } if id == "demo"),
            "来源要按插件归因：{:?}",
            events[1].origin
        );
    }

    /// 没接出口时**不伪造事件**（退回日志），且不 panic。
    #[test]
    fn test_without_outlet_falls_back_to_logging() {
        let t = ScriptTelemetry::new(None, Origin::Engine, "x");
        t.entered();
        t.finished(DiagOutcome::Failed, 1, None);
    }
}
