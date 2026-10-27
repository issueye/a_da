use super::*;

impl Dispatcher {
    /// 跑一个**轮外**事前点位（P3a·SP-55）。
    ///
    /// 观测**先攒后发**：诊断要写 `store`（async），而端口给的回调是同步的（`Fn`）。
    /// 攒起来在调用返回后投影，代价是"发出时刻"晚了一点——这里刻意接受，
    /// 因为轮外点位本来就没有轮内顺序可谈，而 `duration_ms` 是**运行时实测**的、不是编的。
    /// 轮内点位（引擎）用的是同步 sink，所以那边是真·实时逐条发。
    pub(super) async fn run_out_of_turn_before(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
    ) -> agent_base::ports::BeforeOutcome {
        let Some(runner) = &self.hook_runner else {
            return agent_base::ports::BeforeOutcome::Pass;
        };
        let collected: std::sync::Mutex<Vec<agent_base::ports::HookObservation>> =
            std::sync::Mutex::new(Vec::new());
        let on_event = |obs: agent_base::ports::HookObservation| {
            if let Ok(mut v) = collected.lock() {
                v.push(obs);
            }
        };
        let ctx = agent_base::ports::HookCtx {
            thread_id: "",
            turn_id: "",
            call_id: None,
            cancel: &NeverCancelled,
            on_event: &on_event,
        };
        let outcome = runner
            .before(
                agent_base::ports::BeforeRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await;
        self.project_hook_observations(&collected).await;
        outcome
    }

    /// 跑一个**轮外**事后点位，返回旁注文本（P3a·SP-55）。
    pub(super) async fn run_out_of_turn_after(
        &self,
        point: &str,
        subject: Option<String>,
        payload: serde_json::Value,
    ) -> Vec<String> {
        let Some(runner) = &self.hook_runner else {
            return Vec::new();
        };
        let collected: std::sync::Mutex<Vec<agent_base::ports::HookObservation>> =
            std::sync::Mutex::new(Vec::new());
        let on_event = |obs: agent_base::ports::HookObservation| {
            if let Ok(mut v) = collected.lock() {
                v.push(obs);
            }
        };
        let ctx = agent_base::ports::HookCtx {
            thread_id: "",
            turn_id: "",
            call_id: None,
            cancel: &NeverCancelled,
            on_event: &on_event,
        };
        let notes = runner
            .after(
                agent_base::ports::AfterRequest {
                    point: point.to_string(),
                    subject,
                    payload,
                },
                &ctx,
            )
            .await;
        self.project_hook_observations(&collected).await;
        notes
            .into_iter()
            .filter_map(|n| match n {
                agent_base::ports::AfterNote::Note { text } if !text.trim().is_empty() => {
                    Some(text)
                }
                _ => None,
            })
            .collect()
    }

    /// 把攒下来的观测投影成诊断（**转换点只有一处**：`ports::hooks` 里那个函数）。
    pub(super) async fn project_hook_observations(
        &self,
        collected: &std::sync::Mutex<Vec<agent_base::ports::HookObservation>>,
    ) {
        let obs = match collected.lock() {
            Ok(v) => v.clone(),
            Err(_) => return,
        };
        if obs.is_empty() {
            return;
        }
        // 🔴 **不能在这里阻塞重入**：调用方可能正握着 store 写锁（历史上的
        // `thread.delete` 就是），此时再 `write().await` 等于同一任务等自己手里的锁，
        // 任务永不恢复、guard 永不释放，整台 agent 的 store 从此锁死。
        // 观测是诊断、不是事实：先试一次（绝大多数路径没有竞争）；拿不到就交给后台
        // 任务在锁释放后补投影，晚一拍不影响任何判定。
        match self.store.try_write() {
            Ok(mut store_w) => {
                for o in &obs {
                    crate::server::diag::project_out_of_turn(
                        &mut store_w,
                        &agent_base::ports::hooks::observation_to_diagnostic(o),
                    );
                }
            }
            Err(_) => {
                // 没有运行时（极少数同步驱动的调用点）就放弃这次投影：诊断可以丢，
                // 阻塞调用方换取一条日志不值得。
                let Ok(handle) = tokio::runtime::Handle::try_current() else {
                    tracing::debug!(
                        "[hooks] store 写锁被占用且没有异步运行时，跳过 {} 条轮外观测",
                        obs.len()
                    );
                    return;
                };
                let store = Arc::clone(&self.store);
                handle.spawn(async move {
                    let mut store_w = store.write().await;
                    for o in &obs {
                        crate::server::diag::project_out_of_turn(
                            &mut store_w,
                            &agent_base::ports::hooks::observation_to_diagnostic(o),
                        );
                    }
                });
            }
        }
    }
}
