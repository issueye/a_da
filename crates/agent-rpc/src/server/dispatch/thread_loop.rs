use super::helpers::*;
use super::*;

impl Dispatcher {
    pub(super) async fn spawn_thread_loop(
        &self,
        thread_id: String,
        first_prompt: Option<String>,
        first_images: Option<Vec<String>>,
        _provider_config: ProviderConfig,
    ) {
        let target_tid_clone_for_insert = thread_id.clone();
        let store_clone = Arc::clone(&self.store);
        let broadcaster_clone = self.broadcaster.clone();
        let abort_senders_clone = Arc::clone(&self.abort_senders);
        let running_tasks_clone = Arc::clone(&self.running_tasks);
        let target_tid_clone = thread_id;
        // W3-T2：把注入的真引擎（若有）与动态工作区工厂带进 drain 循环
        let engine_clone = self.engine.clone();
        let engine_factory_clone = self.engine_factory.clone();
        let workspace_engines_clone = Arc::clone(&self.workspace_engines);
        let pm_engine_clone = self.pm_engine.clone();
        let pm_engine_factory_clone = self.pm_engine_factory.clone();
        let pm_workspace_engines_clone = Arc::clone(&self.pm_workspace_engines);
        let session_mgr_clone = self.session_mgr.clone();

        let runner_task = tokio::spawn(async move {
            let mut current_prompt: Option<String> = first_prompt;
            // 与 `current_prompt` 同生命周期：本轮带哪几张图。排队指令出队时一并换成它自带的那组，
            // 否则第二条指令会"继承"第一条的图片（图片被当成会话级状态而不是消息级附件）。
            let mut current_images: Option<Vec<String>> = first_images;
            // 本次执行链里已经自动压缩过几次（防"压了还撞顶"无限循环）
            let mut auto_compact_rounds = 0u32;
            loop {
                let (abort_tx, abort_rx) = watch::channel(false);
                abort_senders_clone
                    .lock()
                    .await
                    .insert(target_tid_clone.clone(), abort_tx);

                // 容量给足：模型流式时 `TextDelta` 事件密集，通道小会让关键事件（审批/收尾）
                // 更容易撞上"满"而被丢。关键事件另有不可丢的退避重试（见 DirectEventSink）。
                let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(1024);
                let turn_started = std::time::Instant::now();
                let tid = target_tid_clone.clone();
                let prompt = current_prompt.take();
                let images = current_images.take();
                // 压缩重试要复用同一组图片（见下面的 `current_images = retry_images`）
                let retry_images = images.clone();
                let p_cfg = {
                    let s = store_clone.read().await;
                    s.provider.clone()
                };
                // 本轮是否是"撞顶停的"（引擎通过专门事件告知，见下面的事件臂）
                let mut context_exceeded: Option<(u64, u64)> = None;
                // 本轮是否是"模型请求失败停的"（重试已耗尽）→ 断开对话，不再消费排队指令
                let mut hard_failure: Option<String> = None;

                // 每轮根据当前会话绑定的 Agent / 模式 / 所属工作区 分发对应引擎：
                let (thread_agent_id, thread_mode, thread_ws, context_window, retry_count) = {
                    let s = store_clone.read().await;
                    let t = s.threads.iter().find(|t| t.id == tid);
                    let agent_id = t.and_then(|t| t.agent_id.clone());
                    let mode = t.and_then(|t| t.mode);
                    let ws = thread_workspace(&s, &tid);
                    // 上下文窗口：界面配置里的真值（运行时可变），交给引擎做撞顶治理
                    let cw = s.config.context_window;
                    // 请求重试次数：同样取界面配置的真值（设置项「请求重试次数」，默认 5）
                    let retries = agent_base::model::normalize_model_retry_count(
                        s.config.retry_count as usize,
                    );
                    (
                        agent_id,
                        mode,
                        ws,
                        if cw > 0 { Some(cw) } else { None },
                        retries,
                    )
                };
                let is_pm = thread_agent_id.as_deref() == Some("ada-pm")
                    || thread_agent_id.as_deref() == Some("pm-assistant")
                    || thread_mode == Some(AgentMode::Pm);
                let engine_for_turn = if is_pm {
                    let pm_res = resolve_engine_from_cache(
                        &thread_ws,
                        &pm_workspace_engines_clone,
                        &pm_engine_factory_clone,
                        &pm_engine_clone,
                    )
                    .await;
                    if pm_res.is_some() {
                        pm_res
                    } else {
                        resolve_engine_from_cache(
                            &thread_ws,
                            &workspace_engines_clone,
                            &engine_factory_clone,
                            &engine_clone,
                        )
                        .await
                    }
                } else {
                    resolve_engine_from_cache(
                        &thread_ws,
                        &workspace_engines_clone,
                        &engine_factory_clone,
                        &engine_clone,
                    )
                    .await
                };

                // 失败卡片要用的句柄：这一支**不产生任何 AgentEvent**（引擎根本没起来），
                // 所以提示只能由宿主自己补，否则用户那边就是"发了消息毫无反应"。
                let fail_mgr = session_mgr_clone.clone();
                let fail_store = std::sync::Arc::clone(&store_clone);
                let fail_tid = tid.clone();
                let loop_handle = tokio::spawn(async move {
                    // W3-T2：统一入口——按是否注入引擎决定执行。
                    if let Err(e) = run_agent_turn(
                        engine_for_turn,
                        &tid,
                        prompt.as_deref(),
                        images,
                        p_cfg,
                        context_window,
                        Some(retry_count),
                        event_tx,
                        Some(abort_rx),
                    )
                    .await
                    {
                        tracing::warn!("轮次执行失败: {e}");
                        // 2026-10-18 真事故：`create_plugin` 声明了 `RollbackPolicy::None`，
                        // 引擎装配期契约违约 → 引擎没注入 → 每轮在这里静默失败，
                        // 界面上是「1 轮 / 0 tok / 0s」，用户只看到自己的提问孤零零挂着。
                        // 这类失败不会有 AgentEvent，所以提示卡必须在这里补。
                        let notice = format!("本轮未能开始：{e}");
                        persist_failure_notice(&fail_mgr, &fail_tid, &notice);
                        let mut store = fail_store.write().await;
                        store.add_notice(&fail_tid, &notice, "error");
                        store.push_log("error", notice, None);
                    }
                });

                while let Some(event) = event_rx.recv().await {
                    let mut store = store_clone.write().await;
                    match event.body {
                        AgentEventBody::ThinkingDelta { text } => {
                            store.append_thinking_delta(&target_tid_clone, &text);
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.mark_dirty();
                            }
                        }
                        AgentEventBody::TextDelta { text } => {
                            store.append_assistant_delta(&target_tid_clone, &text);
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.mark_dirty();
                            }
                        }
                        AgentEventBody::ToolCallStarted {
                            call_id,
                            name,
                            args,
                        } => {
                            let args_str = args.to_string();
                            store.start_tool_call(&target_tid_clone, &call_id, &name, &args_str);
                            let is_subagent = name == "invoke_subagent";
                            // 这次派活**之前**该父会话已有的子会话：下面靠它认出
                            // "本次调用新建的那一个"。缺了它就只能"任取一个子会话"，
                            // 第二次派活会把上一次已结束的子会话重新点亮成运行中。
                            let known_children: Vec<String> = store
                                .threads
                                .iter()
                                .filter(|t| {
                                    t.parent_id.as_deref() == Some(target_tid_clone.as_str())
                                        && t.is_subagent == Some(true)
                                })
                                .map(|t| t.id.clone())
                                .collect();
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }

                            // 子智能体启动时：后台异步检测落盘会话并及时绑定至工具卡片与 running_thread_ids
                            if is_subagent {
                                let store_monitor = store_clone.clone();
                                let bc_monitor = broadcaster_clone.clone();
                                let sess_mgr_monitor = session_mgr_clone.clone();
                                let parent_tid_monitor = target_tid_clone.clone();
                                let call_id_monitor = call_id.clone();
                                let known_children_monitor = known_children;
                                tokio::spawn(async move {
                                    for _ in 0..60 {
                                        tokio::time::sleep(tokio::time::Duration::from_millis(100))
                                            .await;
                                        // 两阶段：解析在锁外（子会话进度只从磁盘回流，
                                        // 这里几乎每拍都要解析），写锁只做合并。
                                        let force =
                                            store_monitor.read().await.needs_full_disk_scan();
                                        if let Some(disk_threads) = crate::state::load_changed_threads(
                                            &sess_mgr_monitor,
                                            force,
                                        ) {
                                            store_monitor
                                                .write()
                                                .await
                                                .merge_disk_threads(disk_threads);
                                        }
                                        let mut st = store_monitor.write().await;
                                        // 已经被**别的运行中委派卡片**认领过的子会话不再重复认领
                                        let bound_children: Vec<String> = st
                                            .threads
                                            .iter()
                                            .find(|t| t.id == parent_tid_monitor)
                                            .map(|t| {
                                                t.items
                                                    .iter()
                                                    .filter_map(|it| match it.as_ref() {
                                                        agent_proto::Item::Tool {
                                                            name,
                                                            status,
                                                            call_id,
                                                            details,
                                                            ..
                                                        } if name == "invoke_subagent"
                                                            && status == "running"
                                                            && call_id != &call_id_monitor =>
                                                        {
                                                            details.as_ref().and_then(|d| {
                                                                d.get("threadId")
                                                                    .or_else(|| {
                                                                        d.get("subagent_thread_id")
                                                                    })
                                                                    .and_then(|v| v.as_str())
                                                                    .map(|s| s.to_string())
                                                            })
                                                        }
                                                        _ => None,
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        let found_child = st.pick_new_subagent_child(
                                            &parent_tid_monitor,
                                            &known_children_monitor,
                                            &bound_children,
                                        );

                                        if let Some(child_id) = found_child {
                                            let mut updated = false;
                                            if !st.running_thread_ids.contains(&child_id) {
                                                st.running_thread_ids.push(child_id.clone());
                                                updated = true;
                                            }
                                            if let Some(parent_t) =
                                                st.get_thread_mut(&parent_tid_monitor)
                                            {
                                                for item in parent_t.items.iter_mut().rev() {
                                                    // 只认这一张正在跑的 invoke_subagent 卡：
                                                    // 命中才 copy-on-write，别克隆其它共享卡片。
                                                    let hit = matches!(
                                                        item.as_ref(),
                                                        agent_proto::Item::Tool { call_id, status, .. }
                                                            if call_id == &call_id_monitor
                                                                && status == "running"
                                                    );
                                                    if !hit {
                                                        continue;
                                                    }
                                                    if let agent_proto::Item::Tool {
                                                        details,
                                                        ..
                                                    } = Arc::make_mut(item)
                                                    {
                                                        let mut det = details.take().unwrap_or_else(
                                                            || serde_json::json!({}),
                                                        );
                                                        if let Some(obj) = det.as_object_mut() {
                                                            if !obj.contains_key("threadId") {
                                                                obj.insert(
                                                                    "threadId".to_string(),
                                                                    serde_json::json!(child_id),
                                                                );
                                                                obj.insert(
                                                                    "subagent_thread_id".to_string(),
                                                                    serde_json::json!(child_id),
                                                                );
                                                                updated = true;
                                                            }
                                                        }
                                                        *details = Some(det);
                                                        break;
                                                    }
                                                }
                                            }
                                            drop(st);
                                            if updated {
                                                if let Some(ref bc) = bc_monitor {
                                                    bc.broadcast_immediate().await;
                                                }
                                                break;
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        AgentEventBody::ToolCallFinished {
                            call_id,
                            name,
                            receipt,
                        } => {
                            // 本次调用对应的子会话 id：回执里带的就是它（见 `SubagentTool`）
                            let finished_child = receipt.details.as_ref().and_then(|d| {
                                d.get("subagent_thread_id")
                                    .or_else(|| d.get("threadId"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            });
                            store.finish_tool_call(
                                &target_tid_clone,
                                &call_id,
                                receipt.ok(),
                                Some(receipt.output.clone()),
                                Some(receipt.duration_ms()),
                                Some(receipt.started_at),
                                Some(receipt.finished_at),
                                receipt.details.clone(),
                            );
                            if name == "invoke_subagent" {
                                // 子会话的最终状态只从磁盘回流：先放掉写锁做两阶段同步，
                                // 再回来解除运行态（全量解析是秒级活儿，不能压在锁里）。
                                drop(store);
                                let force = store_clone.read().await.needs_full_disk_scan();
                                if let Some(disk_threads) =
                                    crate::state::load_changed_threads(&session_mgr_clone, force)
                                {
                                    store_clone
                                        .write()
                                        .await
                                        .merge_disk_threads(disk_threads);
                                }
                                store = store_clone.write().await;
                                match finished_child {
                                    // 精确解除：只摘掉**这次**派活那个子会话的运行态。
                                    // 按"父会话名下所有子会话"一刀切会误伤并行派活的另一个子会话。
                                    Some(cid) => store.running_thread_ids.retain(|tid| tid != &cid),
                                    // 回执里没有子会话 id（没进入执行/被拒）→ 退回按父会话清理
                                    None => {
                                        let child_ids: Vec<String> = store
                                            .threads
                                            .iter()
                                            .filter(|t| {
                                                t.parent_id.as_deref() == Some(&target_tid_clone)
                                                    && t.is_subagent == Some(true)
                                            })
                                            .map(|t| t.id.clone())
                                            .collect();
                                        store
                                            .running_thread_ids
                                            .retain(|tid| !child_ids.contains(tid));
                                    }
                                }
                            }
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.broadcast_immediate().await;
                            }
                        }
                        AgentEventBody::UsageReported { usage, duration_ms } => {
                            let turn_duration_ms = turn_started.elapsed().as_millis() as u64;
                            store.set_assistant_stats(
                                &target_tid_clone,
                                Some(usage),
                                duration_ms,
                                turn_duration_ms,
                            );
                            drop(store);
                            if let Some(ref bc) = broadcaster_clone {
                                bc.mark_dirty();
                            }
                        }
                        AgentEventBody::Diagnostic(diag) => {
                            // P0 埋点地基：诊断遥测**不进会话流、不进模型上下文**，
                            // 只进诊断缓冲（界面 DebugPanel）与 tracing。
                            //
                            // 与 Notice 的分工：Notice 是"给用户看的一条提示"
                            // （要落盘、要跨端可见），Diagnostic 是"给排查用的元数据"
                            // （不落盘，只此进程有效）。
                            //
                            // 投影走 `server::diag` 的**唯一一处**：轮次外的扫描类诊断
                            // 也从那里进同一个缓冲，两处各写一份格式化必然分叉。
                            let text = crate::server::diag::project_diagnostic(
                                &mut store,
                                &diag,
                                &event.trace,
                            );
                            drop(store);
                            tracing::debug!(target: "a_da::diag", "{text}");
                            if let Some(ref bc) = broadcaster_clone {
                                bc.mark_dirty();
                            }
                        }
                        AgentEventBody::TurnFinished { stop } => {
                            // 上下文撞顶：记下来，收尾后**自动压缩并续跑**（见下面的分支）
                            if let TurnStopReason::ContextWindowExceeded {
                                prompt_tokens,
                                window,
                            } = stop
                            {
                                store.push_log(
                                            "warn",
                                            format!("上下文撞顶（{prompt_tokens}/{window} tokens）：将自动压缩后继续"),
                                            None,
                                        );
                                drop(store);
                                context_exceeded = Some((prompt_tokens, window));
                                break;
                            }
                            drop(store);
                            break;
                        }
                        AgentEventBody::Failed { message } => {
                            // 失败信息走 **Notice 卡片**，绝不写进助手正文：
                            // ① 落进正文会把一段连贯回答从中间劈开（截图里的形态）；
                            // ② 正文会被回传给模型，等于让模型以为"我上一轮说过这句报错"。
                            // Notice 的落盘记录在 `fold_messages` 里被明确跳过，
                            // 所以它**只给人看**、永不进上下文。
                            //
                            // 「已断开」这句由**宿主**写，而不是只做界面提示：
                            // 任何客户端（桌面 / Web / CLI）都该看到"这轮到此为止"，
                            // 而不是各端各自猜。
                            let notice_text = format!(
                                "请求异常：{message}\n\
                                         重试已耗尽，本次对话已断开；排队中的指令仍保留。"
                            );
                            // 落盘**在改内存之前**（与重试清理同一顺序）：
                            // 后台每 1.5s 一次的 `sync_threads_from_disk` 要拿 store
                            // 写锁，所以还握着写锁时先把磁盘写好，就不存在
                            // "同步读到旧磁盘内容、把这张卡片抹掉"的窗口。
                            persist_failure_notice(
                                &session_mgr_clone,
                                &target_tid_clone,
                                &notice_text,
                            );
                            store.add_notice(&target_tid_clone, &notice_text, "error");
                            store.push_log("error", format!("Agent 执行异常: {message}"), None);
                            drop(store);
                            // 模型请求的重试次数已在适配器里用完（次数由设置项决定）
                            // → 记下来，收尾时**断开本轮对话**而不是接着跑排队指令
                            hard_failure = Some(message);
                            break;
                        }
                        AgentEventBody::TurnStarted => {
                            drop(store);
                        }
                        AgentEventBody::SubagentStarted {
                            thread_id,
                            subagent,
                        } => {
                            tracing::debug!(child_thread_id = %thread_id, subagent = %subagent, "子智能体已启动");
                            drop(store);
                        }
                        AgentEventBody::SubagentFinished {
                            thread_id,
                            ok,
                            summary,
                        } => {
                            tracing::debug!(child_thread_id = %thread_id, ok = ok, summary = %summary, "子智能体已结束");
                            drop(store);
                        }
                    }
                }

                let _ = loop_handle.await;

                // 收尾并闭合本轮卡片状态
                {
                    let mut store = store_clone.write().await;
                    store.end_thinking(&target_tid_clone);
                    if let Some(t) = store.get_thread_mut(&target_tid_clone) {
                        for item in t.items.iter_mut() {
                            if !matches!(item.as_ref(), Item::Assistant { .. }) {
                                continue;
                            }
                            if let Item::Assistant { streaming, .. } = Arc::make_mut(item) {
                                *streaming = None;
                            }
                        }
                        if let Some(Item::Thinking { text, .. }) =
                            t.items.last().map(|item| item.as_ref())
                        {
                            if text.trim().is_empty() {
                                t.items.pop();
                            }
                        }
                    }
                }

                // 如果该会话已被主动停止（用户点击了中止），则不再消费队列
                {
                    let store = store_clone.read().await;
                    if !store.is_thread_running(&target_tid_clone) {
                        break;
                    }
                }

                // 模型请求失败（重试次数已耗尽）→ **断开本次对话**：
                // 排队指令原样留在队列里，等用户改配置/网络后手动重试，
                // 而不是拿注定失败的请求把队列一条条撞完。
                if let Some(msg) = hard_failure.take() {
                    let mut store = store_clone.write().await;
                    store.push_log(
                        "error",
                        format!("模型请求失败（重试已耗尽），本次对话已断开: {msg}"),
                        None,
                    );
                    drop(store);
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
                    break;
                }

                // 上下文撞顶 → **自动压缩并续跑**（而不是丢给用户一句"请手动压缩"）。
                // 压过一次仍然撞顶就如实收尾，绝不无限压。
                if let Some((pt, win)) = context_exceeded.take() {
                    if auto_compact_rounds >= 1 {
                        let mut store = store_clone.write().await;
                        store.append_assistant_delta(
                            &target_tid_clone,
                            &format!(
                                "\n\n**上下文已达窗口上限**（{pt}/{win} tokens），压缩后仍然超限。\
                                         请把任务拆小，或换更大上下文窗口的模型。"
                            ),
                        );
                        drop(store);
                        if let Some(ref bc) = broadcaster_clone {
                            bc.broadcast_immediate().await;
                        }
                        break;
                    }
                    auto_compact_rounds += 1;
                    tracing::info!("上下文撞顶（{pt}/{win} tokens），自动压缩后继续");
                    let summary =
                        compact_thread_now(&store_clone, &session_mgr_clone, &target_tid_clone)
                            .await;
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
                    current_prompt = Some(match summary {
                        Some(s) => format!(
                            "【上下文已自动压缩】以下是压缩后的历史摘要，请基于它继续完成原任务：\n\n{s}"
                        ),
                        None => {
                            "【上下文已自动压缩】请基于压缩后的上下文继续完成原任务。".to_string()
                        }
                    });
                    // 压缩后的重试仍是**同一轮**：用户附的图片要跟着一起重发，
                    // 否则"发图 + 上下文撞顶"就变成图片被静默吞掉。
                    current_images = retry_images;
                    continue;
                }

                // 检查会话队列中是否还有待执行的排队指令
                let next_item = {
                    let mut store = store_clone.write().await;
                    store.pop_next_queued(&target_tid_clone)
                };

                if let Some(queued) = next_item {
                    // 出队指令**自带**它的图片：本轮换用队列里那一组，别继承上一条指令的
                    // （图片是消息级附件，不是会话级状态）。
                    let queued_images = queued.images.clone();
                    let mut store = store_clone.write().await;
                    store.add_user_message_with_images(
                        &target_tid_clone,
                        &queued.text,
                        queued.images,
                    );
                    drop(store);
                    if let Some(ref bc) = broadcaster_clone {
                        bc.broadcast_immediate().await;
                    }
                    current_prompt = Some(queued.text);
                    current_images = queued_images;
                } else {
                    // 对话队列已清空，退出 drain 循环
                    break;
                }
            }

            // 整个执行链（含排队消息）全部完成，统一收尾
            abort_senders_clone.lock().await.remove(&target_tid_clone);
            running_tasks_clone.lock().await.remove(&target_tid_clone);
            let mut store = store_clone.write().await;
            store.finish_turn(&target_tid_clone);
            drop(store);
            if let Some(ref bc) = broadcaster_clone {
                bc.broadcast_immediate().await;
            }
        });

        self.running_tasks
            .lock()
            .await
            .insert(target_tid_clone_for_insert, runner_task);
    }
}
