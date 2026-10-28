//! 真正的插件事件运行时：有界 lane、异步工作线程、generation 与丢弃计数。
//!
//! 它包在 [`super::HookEventDispatcher`] 外面：
//!
//! - `Critical`：`event_before_*`，单 worker 串行执行，调用方等待结果；
//! - `Decorate`：`event_after_*`，单 worker，调用方等待 note/observe；
//! - `Notify`：生命周期/通知，fire-and-forget；
//! - `Stream`：文本/思考增量，独立 lane，可丢弃。
//!
//! `Critical` / `Decorate` 的观察回调不能跨 worker 借用，所以这里用一条
//! `unbounded_channel` 把 `HookObservation` 送回调用方；调用方在自己的 future 里
//! 一边等结果一边消费观测，既保持“观测由引擎发诊断”，也不需要把 `&dyn EventSink`
//! 变成 `'static`。
//!
//! generation：插件重装配时调用 [`PluginEventRuntime::bump_generation`]；旧 generation
//! 的排队事件在 worker 取到时直接作废（critical 回 `Pass`，decorate 回空，notify 丢弃），
//! 避免“停用插件后它还在拦”。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_base::domain::{EventOutcome, PluginEvent};
use agent_base::ports::hooks::HookObservation;
use agent_base::ports::{BoxFuture, EventDispatcher};
use tokio::sync::{mpsc, oneshot};

/// 默认每个 lane 的队列深度。
pub const DEFAULT_LANE_CAPACITY: usize = 256;

/// 事件运行时统计（测试/诊断用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventRuntimeStats {
    pub processed: u64,
    pub dropped_notifications: u64,
}

struct CriticalJob {
    event: PluginEvent,
    generation: u64,
    observations: mpsc::UnboundedSender<HookObservation>,
    response: oneshot::Sender<EventOutcome>,
}

struct DecorateJob {
    event: PluginEvent,
    generation: u64,
    observations: mpsc::UnboundedSender<HookObservation>,
    response: oneshot::Sender<Vec<EventOutcome>>,
}

struct NotifyJob {
    event: PluginEvent,
    generation: u64,
}

/// 插件事件运行时（每 agent runtime 一个实例）。
pub struct PluginEventRuntime {
    inner: Arc<dyn EventDispatcher>,
    generation: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
    dropped_notifications: Arc<AtomicU64>,
    critical_tx: Option<mpsc::Sender<CriticalJob>>,
    decorate_tx: Option<mpsc::Sender<DecorateJob>>,
    notify_tx: Option<mpsc::Sender<NotifyJob>>,
    stream_tx: Option<mpsc::Sender<NotifyJob>>,
}

impl PluginEventRuntime {
    /// 构造并启动四条 lane 的 worker。
    ///
    /// 没有 Tokio 运行时（纯同步测试/老调用点）时不启动 worker，所有调用直接回落
    /// 到 `inner`——行为不变，只是没有队列。
    pub fn new(inner: Arc<dyn EventDispatcher>) -> Self {
        Self::with_capacity(inner, DEFAULT_LANE_CAPACITY)
    }

    pub fn with_capacity(inner: Arc<dyn EventDispatcher>, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let generation = Arc::new(AtomicU64::new(1));
        let processed = Arc::new(AtomicU64::new(0));
        let dropped_notifications = Arc::new(AtomicU64::new(0));

        let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
            return Self {
                inner,
                generation,
                processed,
                dropped_notifications,
                critical_tx: None,
                decorate_tx: None,
                notify_tx: None,
                stream_tx: None,
            };
        };

        let (critical_tx, critical_rx) = mpsc::channel(capacity);
        let (decorate_tx, decorate_rx) = mpsc::channel(capacity);
        let (notify_tx, notify_rx) = mpsc::channel(capacity);
        let (stream_tx, stream_rx) = mpsc::channel(capacity);

        handle.spawn(critical_worker(
            inner.clone(),
            critical_rx,
            generation.clone(),
            processed.clone(),
        ));
        handle.spawn(decorate_worker(
            inner.clone(),
            decorate_rx,
            generation.clone(),
            processed.clone(),
        ));
        handle.spawn(notify_worker(
            inner.clone(),
            notify_rx,
            generation.clone(),
            processed.clone(),
        ));
        handle.spawn(notify_worker(
            inner.clone(),
            stream_rx,
            generation.clone(),
            processed.clone(),
        ));

        Self {
            inner,
            generation,
            processed,
            dropped_notifications,
            critical_tx: Some(critical_tx),
            decorate_tx: Some(decorate_tx),
            notify_tx: Some(notify_tx),
            stream_tx: Some(stream_tx),
        }
    }

    /// 当前 generation（重装配后递增）。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// 插件重装配后调用：旧 generation 的排队事件作废。
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn stats(&self) -> EventRuntimeStats {
        EventRuntimeStats {
            processed: self.processed.load(Ordering::Relaxed),
            dropped_notifications: self.dropped_notifications.load(Ordering::Relaxed),
        }
    }
}

impl EventDispatcher for PluginEventRuntime {
    fn publish(&self, event: PluginEvent) {
        let Some(tx) = (if is_stream(&event.name) {
            &self.stream_tx
        } else {
            &self.notify_tx
        }) else {
            self.inner.publish(event);
            return;
        };
        let job = NotifyJob {
            event,
            generation: self.generation(),
        };
        if tx.try_send(job).is_err() {
            self.dropped_notifications.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dispatch<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, EventOutcome> {
        let Some(tx) = &self.critical_tx else {
            return self.inner.dispatch(event, on_observation);
        };
        let tx = tx.clone();
        let generation = self.generation();
        Box::pin(async move {
            let (obs_tx, mut obs_rx) = mpsc::unbounded_channel();
            let (response_tx, mut response_rx) = oneshot::channel();
            let job = CriticalJob {
                event,
                generation,
                observations: obs_tx,
                response: response_tx,
            };
            if tx.send(job).await.is_err() {
                return EventOutcome::Pass;
            }
            let outcome = loop {
                tokio::select! {
                    Some(obs) = obs_rx.recv() => on_observation(obs),
                    result = &mut response_rx => break result.unwrap_or(EventOutcome::Pass),
                }
            };
            while let Ok(obs) = obs_rx.try_recv() {
                on_observation(obs);
            }
            outcome
        })
    }

    fn dispatch_many<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, Vec<EventOutcome>> {
        let Some(tx) = &self.decorate_tx else {
            return self.inner.dispatch_many(event, on_observation);
        };
        let tx = tx.clone();
        let generation = self.generation();
        Box::pin(async move {
            let (obs_tx, mut obs_rx) = mpsc::unbounded_channel();
            let (response_tx, mut response_rx) = oneshot::channel();
            let job = DecorateJob {
                event,
                generation,
                observations: obs_tx,
                response: response_tx,
            };
            if tx.send(job).await.is_err() {
                return Vec::new();
            }
            let outcomes = loop {
                tokio::select! {
                    Some(obs) = obs_rx.recv() => on_observation(obs),
                    result = &mut response_rx => break result.unwrap_or_default(),
                }
            };
            while let Ok(obs) = obs_rx.try_recv() {
                on_observation(obs);
            }
            outcomes
        })
    }
}

fn is_stream(name: &str) -> bool {
    matches!(name, "event_text_delta" | "event_thinking_delta")
}

async fn critical_worker(
    inner: Arc<dyn EventDispatcher>,
    mut rx: mpsc::Receiver<CriticalJob>,
    generation: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
) {
    while let Some(job) = rx.recv().await {
        if job.generation != generation.load(Ordering::SeqCst) {
            let _ = job.response.send(EventOutcome::Pass);
            continue;
        }
        let observer = move |obs| {
            let _ = job.observations.send(obs);
        };
        let outcome = inner.dispatch(job.event, &observer).await;
        processed.fetch_add(1, Ordering::Relaxed);
        let _ = job.response.send(outcome);
    }
}

async fn decorate_worker(
    inner: Arc<dyn EventDispatcher>,
    mut rx: mpsc::Receiver<DecorateJob>,
    generation: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
) {
    while let Some(job) = rx.recv().await {
        if job.generation != generation.load(Ordering::SeqCst) {
            let _ = job.response.send(Vec::new());
            continue;
        }
        let observer = move |obs| {
            let _ = job.observations.send(obs);
        };
        let outcomes = inner.dispatch_many(job.event, &observer).await;
        processed.fetch_add(1, Ordering::Relaxed);
        let _ = job.response.send(outcomes);
    }
}

async fn notify_worker(
    inner: Arc<dyn EventDispatcher>,
    mut rx: mpsc::Receiver<NotifyJob>,
    generation: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
) {
    while let Some(job) = rx.recv().await {
        if job.generation != generation.load(Ordering::SeqCst) {
            continue;
        }
        inner.publish(job.event);
        processed.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::EventSource;
    use std::sync::Mutex;

    struct MockDispatcher {
        seen: Arc<Mutex<Vec<String>>>,
        outcome: EventOutcome,
    }
    impl EventDispatcher for MockDispatcher {
        fn publish(&self, event: PluginEvent) {
            self.seen
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(event.name);
        }

        fn dispatch<'a>(
            &'a self,
            _event: PluginEvent,
            _on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
        ) -> BoxFuture<'a, EventOutcome> {
            Box::pin(async move { self.outcome.clone() })
        }

        fn dispatch_many<'a>(
            &'a self,
            _event: PluginEvent,
            _on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
        ) -> BoxFuture<'a, Vec<EventOutcome>> {
            Box::pin(async move { vec![EventOutcome::Note { text: "n".into() }] })
        }
    }

    /// 第一个 critical 作业卡在 `release` 上，用来把后一个作业稳定地压在队列里。
    struct GateDispatcher {
        calls: Arc<AtomicU64>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        outcome: EventOutcome,
    }

    impl EventDispatcher for GateDispatcher {
        fn publish(&self, _event: PluginEvent) {}

        fn dispatch<'a>(
            &'a self,
            _event: PluginEvent,
            _on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
        ) -> BoxFuture<'a, EventOutcome> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                self.release.notified().await;
                self.outcome.clone()
            })
        }

        fn dispatch_many<'a>(
            &'a self,
            _event: PluginEvent,
            _on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
        ) -> BoxFuture<'a, Vec<EventOutcome>> {
            Box::pin(async move { Vec::new() })
        }
    }

    fn event(name: &str, seq: u64) -> PluginEvent {
        PluginEvent::new(
            name,
            seq,
            0,
            "thread-1",
            EventSource::Engine,
            serde_json::json!({ "tool": "run_command" }),
        )
    }

    #[tokio::test]
    async fn critical_dispatch_returns_inner_outcome_and_notify_reaches_inner() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let inner = Arc::new(MockDispatcher {
            seen: seen.clone(),
            outcome: EventOutcome::Deny {
                reason: "blocked".into(),
            },
        });
        let runtime = PluginEventRuntime::with_capacity(inner, 4);

        let outcome = runtime
            .dispatch(event("event_before_tool_call", 1), &|_| {})
            .await;
        assert_eq!(
            outcome,
            EventOutcome::Deny {
                reason: "blocked".into()
            }
        );

        let notes = runtime
            .dispatch_many(event("event_after_tool_call", 2), &|_| {})
            .await;
        assert_eq!(notes, vec![EventOutcome::Note { text: "n".into() }]);

        runtime.publish(event("event_turn_finished", 3));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            seen.lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .contains(&"event_turn_finished".to_string()),
            "notify lane 必须把事件交给 inner"
        );
        assert!(runtime.stats().processed >= 2, "{:?}", runtime.stats());
    }

    #[test]
    fn generation_bumps_monotonically() {
        // 没有 Tokio 时构造也必须是安全的：直接回落 inner，不起 worker。
        let runtime = PluginEventRuntime::new(Arc::new(MockDispatcher {
            seen: Arc::new(Mutex::new(Vec::new())),
            outcome: EventOutcome::Pass,
        }));
        assert_eq!(runtime.generation(), 1);
        runtime.bump_generation();
        assert_eq!(runtime.generation(), 2);
    }

    #[tokio::test]
    async fn stale_generation_discards_queued_critical_job() {
        let calls = Arc::new(AtomicU64::new(0));
        let inner = Arc::new(GateDispatcher {
            calls: calls.clone(),
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            outcome: EventOutcome::Deny {
                reason: "blocked".into(),
            },
        });
        let runtime = PluginEventRuntime::with_capacity(inner.clone(), 1);
        let tx = runtime
            .critical_tx
            .as_ref()
            .expect("Tokio 运行时下应启动 critical worker");

        let (obs1_tx, _obs1_rx) = mpsc::unbounded_channel();
        let (resp1_tx, resp1_rx) = oneshot::channel();
        tx.send(CriticalJob {
            event: event("event_before_tool_call", 1),
            generation: runtime.generation(),
            observations: obs1_tx,
            response: resp1_tx,
        })
        .await
        .expect("critical lane 应接收作业");
        // 等 worker 真正进入第一个作业，后面的作业才会稳定排在队列里。
        inner.entered.notified().await;

        let (obs2_tx, _obs2_rx) = mpsc::unbounded_channel();
        let (resp2_tx, resp2_rx) = oneshot::channel();
        tx.send(CriticalJob {
            event: event("event_before_tool_call", 2),
            generation: runtime.generation(),
            observations: obs2_tx,
            response: resp2_tx,
        })
        .await
        .expect("critical lane 应接收作业");

        // 重装配：排队中的第二个作业属于旧 generation，必须作废。
        runtime.bump_generation();
        inner.release.notify_waiters();

        assert_eq!(
            resp1_rx.await.expect("第一个作业应有回执"),
            EventOutcome::Deny {
                reason: "blocked".into()
            }
        );
        assert_eq!(
            resp2_rx.await.expect("被作废的作业也要回执"),
            EventOutcome::Pass
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "旧 generation 的作业不得进入 inner"
        );
        assert_eq!(runtime.stats().processed, 1);
    }

    #[tokio::test]
    async fn notify_lane_drops_when_full_and_skips_stale_generation() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runtime = PluginEventRuntime::with_capacity(
            Arc::new(MockDispatcher {
                seen: seen.clone(),
                outcome: EventOutcome::Pass,
            }),
            1,
        );

        // 当前线程运行时里不 await，worker 没有机会取走第一条：队列深度 1 稳定可测。
        runtime.publish(event("event_turn_finished", 1));
        runtime.publish(event("event_turn_finished", 2));
        assert_eq!(runtime.stats().dropped_notifications, 1);

        runtime.bump_generation();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            seen.lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .is_empty(),
            "旧 generation 的通知不得再交给 inner"
        );

        runtime.publish(event("event_turn_started", 3));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            seen.lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .as_slice(),
            ["event_turn_started".to_string()],
            "generation 更新后新通知必须照常送达"
        );
        assert_eq!(runtime.stats().processed, 1);
    }
}

