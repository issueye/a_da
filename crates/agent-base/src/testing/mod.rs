//! 测试替身：让"引擎测试"不再需要真 WS、真磁盘、真窗口（计划 §9 的样板目标）。
//!
//! 这些替身放在基座里（而不是各 crate 的 `#[cfg(test)]`），因为适配器与产品的测试都要用同一套，
//! 且它们**必须**与端口实现同一份契约——合规套件拿它们当参照实现。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use crate::domain::AgentEvent;
use crate::ports::{AppHome, CancelToken, Clock, EventSink};

/// 确定性时钟：时间只在测试要求时前进。
pub struct FixedClock {
    now_ms: AtomicI64,
}

impl FixedClock {
    pub fn new(start_ms: i64) -> Self {
        Self { now_ms: AtomicI64::new(start_ms) }
    }

    pub fn advance(&self, delta_ms: i64) -> i64 {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst) + delta_ms
    }

    pub fn set(&self, ms: i64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new(1_700_000_000_000)
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// 录制型事件出口：把事件按顺序留下来供断言（替代"靠日志文本判断"）。
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("事件锁中毒").clone()
    }

    /// 取走已录制的事件并清空（便于分段断言）。
    pub fn take(&self) -> Vec<AgentEvent> {
        std::mem::take(&mut *self.events.lock().expect("事件锁中毒"))
    }

    /// 事件类型序列（断言顺序用）。
    pub fn kinds(&self) -> Vec<&'static str> {
        self.snapshot().into_iter().map(|e| e.body.kind()).collect()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().expect("事件锁中毒").push(event);
    }
}

/// 永不被取消的令牌。
pub struct NeverCancel;

impl CancelToken for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 由测试指定的应用目录（**调用方给路径**：基座不读环境变量，也不建目录）。
pub struct TempAppHome {
    root: PathBuf,
    config_file: Option<PathBuf>,
}

impl TempAppHome {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), config_file: None }
    }

    pub fn with_config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_file = Some(path.into());
        self
    }
}

impl AppHome for TempAppHome {
    fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn config_file(&self) -> PathBuf {
        self.config_file.clone().unwrap_or_else(|| self.root.join("config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AgentEvent, AgentEventBody};

    #[test]
    fn fixed_clock_only_moves_when_told() {
        let c = FixedClock::new(1000);
        assert_eq!(c.now_ms(), 1000);
        c.advance(250);
        assert_eq!(c.now_ms(), 1250);
        c.set(42);
        assert_eq!(c.now_ms(), 42);
    }

    #[test]
    fn recording_sink_preserves_order_and_clears_on_take() {
        let sink = RecordingSink::new();
        sink.emit(AgentEvent::new(1, 0, "t1", AgentEventBody::TurnStarted));
        sink.emit(AgentEvent::new(2, 1, "t1", AgentEventBody::TextDelta { text: "hi".into() }));
        assert_eq!(sink.kinds(), vec!["turn.started", "text.delta"]);
        assert_eq!(sink.take().len(), 2);
        assert!(sink.snapshot().is_empty());
    }

    #[test]
    fn temp_app_home_maps_kinds_without_touching_disk() {
        let home = TempAppHome::at("/tmp/x");
        assert_eq!(home.root(), Path::new("/tmp/x"));
        assert_eq!(home.dir("sessions"), PathBuf::from("/tmp/x/sessions"));
        assert_eq!(home.config_file(), PathBuf::from("/tmp/x/config.json"));
    }
}
