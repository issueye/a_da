//! `EventDispatcher` 的生产适配器：把统一 `event_*` 事件折回现有 `HookRuntime`。
//!
//! 这是迁移期的桥，不是最终总线：
//! - `dispatch` 用于 `event_before_*` / `event_after_*`，把结果映射成 [`EventOutcome`]；
//! - `publish` 用于 `event_*` 通知，异步跑一遍 after 钩子并丢弃结果；
//! - 引擎迁移到 dispatcher 后，本适配器可以删掉，由真正的 plugin event bus 取代。
//!
//! 事件名归一化仍只在 `event_names` 一处，避免“注册时一套、调用时另一套”。

use std::sync::Arc;

use agent_base::domain::{EventOutcome, PluginEvent};
use agent_base::ports::hooks::{
    AfterNote, AfterRequest, BeforeOutcome, BeforeRequest, HookCtx, HookObservation, HookRunner,
};
use agent_base::ports::{BoxFuture, CancelToken, EventDispatcher};

use super::event_names;

/// 生产适配器：统一事件 → 现有钩子运行时。
pub struct HookEventDispatcher {
    runtime: Arc<dyn HookRunner>,
}

impl HookEventDispatcher {
    pub fn new(runtime: Arc<dyn HookRunner>) -> Self {
        Self { runtime }
    }
}

impl EventDispatcher for HookEventDispatcher {
    fn publish(&self, event: PluginEvent) {
        let runtime = self.runtime.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            let _ = run_after(runtime.as_ref(), &event, &|_| {}).await;
        });
    }

    fn dispatch<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, EventOutcome> {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            if event.name.starts_with("event_after_") {
                let notes = run_after(runtime.as_ref(), &event, on_observation).await;
                return notes
                    .into_iter()
                    .find_map(|note| match note {
                        AfterNote::Note { text } => Some(EventOutcome::Note { text }),
                        AfterNote::Observe { .. } => None,
                    })
                    .unwrap_or(EventOutcome::Pass);
            }

            let point = event_names::normalize_point(&event.name);
            let request = BeforeRequest {
                subject: subject_of(&event),
                payload: event.payload.clone(),
                point,
            };
            match runtime
                .before(request, &hook_ctx(&event, on_observation))
                .await
            {
                BeforeOutcome::Pass => EventOutcome::Pass,
                BeforeOutcome::Deny { reason } => EventOutcome::Deny { reason },
                BeforeOutcome::Replace { args, .. } => EventOutcome::Replace { payload: args },
                BeforeOutcome::AbortTurn { reason } => EventOutcome::AbortTurn { reason },
            }
        })
    }

    fn dispatch_many<'a>(
        &'a self,
        event: PluginEvent,
        on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> BoxFuture<'a, Vec<EventOutcome>> {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            if event.name.starts_with("event_after_") {
                let notes = run_after(runtime.as_ref(), &event, on_observation).await;
                return notes
                    .into_iter()
                    .map(|note| match note {
                        AfterNote::Note { text } => EventOutcome::Note { text },
                        AfterNote::Observe { data } => EventOutcome::Observe { data },
                    })
                    .collect();
            }
            // before 事件天然只有一个定稿结果；调用方应使用 dispatch，这里保持兼容。
            vec![self.dispatch(event, on_observation).await]
        })
    }
}

async fn run_after(
    runtime: &dyn HookRunner,
    event: &PluginEvent,
    on_observation: &(dyn Fn(HookObservation) + Send + Sync),
) -> Vec<AfterNote> {
    let request = AfterRequest {
        subject: subject_of(event),
        payload: event.payload.clone(),
        point: event_names::normalize_point(&event.name),
    };
    runtime
        .after(request, &hook_ctx(event, on_observation))
        .await
}

fn subject_of(event: &PluginEvent) -> Option<String> {
    event
        .payload
        .get("tool")
        .or_else(|| event.payload.get("subject"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn hook_ctx<'a>(
    event: &'a PluginEvent,
    on_observation: &'a (dyn Fn(HookObservation) + Send + Sync),
) -> HookCtx<'a> {
    HookCtx {
        thread_id: &event.thread_id,
        turn_id: event.turn_id.as_deref().unwrap_or(""),
        call_id: event.call_id.as_deref(),
        cancel: &NeverCancel,
        on_event: on_observation,
    }
}

struct NeverCancel;

impl CancelToken for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookRuntime;
    use agent_base::domain::{EventSource, PluginEvent};

    #[tokio::test]
    async fn no_subscriber_is_pass_and_notify_does_not_block() {
        let dispatcher = HookEventDispatcher::new(Arc::new(HookRuntime::disabled()));
        let event = PluginEvent::new(
            "event_before_tool_call",
            1,
            0,
            "thread-1",
            EventSource::Engine,
            serde_json::json!({ "tool": "run_command" }),
        );
        assert_eq!(
            dispatcher.dispatch(event, &|_| {}).await,
            EventOutcome::Pass
        );
        dispatcher.publish(PluginEvent::new(
            "event_turn_finished",
            2,
            0,
            "thread-1",
            EventSource::Engine,
            serde_json::json!({}),
        ));
    }
}
