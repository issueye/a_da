use boa_engine::{Context, JsValue, NativeFunction, Source, object::builtins::JsPromise};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::thread;

use super::PureTsRuntime;
use super::console::{ConsoleCapture, ConsoleMode, install_capture};
use super::limits::{LIMIT_MARKER, Limits, apply_limits, limit_kind};
use super::messages::EventLoopMsg;
use super::state::{
    ASYNC_HANDLERS, NEXT_JOB_ID, PENDING_IDLE_WAITERS, PENDING_PROMISES, PENDING_TOOL_CALLS,
    TOKIO_HANDLE, WORKER_TX,
};

/// 启动专有事件循环线程，返回可直接使用的运行时。
pub(super) fn build(
    workspace: Option<std::path::PathBuf>,
    limits: Limits,
    needs: crate::env::Needs,
    console_mode: ConsoleMode,
    net: crate::net::NetPolicy,
) -> PureTsRuntime {
    let (tx, rx) = channel::<EventLoopMsg>();
    let worker_tx = tx.clone();
    let tokio_handle = tokio::runtime::Handle::try_current().ok();
    let ws_for_thread = workspace.clone();
    let capture = Arc::new(std::sync::Mutex::new(ConsoleCapture::default()));
    let capture_for_thread = capture.clone();

    thread::Builder::new()
            .name("pure-ts-event-loop".into())
            .spawn(move || {
                let mut ctx = Context::default();
                // P1·SP-12：限额在 `Context` 建好之后立刻应用——它是"同步段必然返回"的
                // 唯一依据，晚一步就等于给死循环留了一整段时间窗。
                apply_limits(&mut ctx, limits);
                WORKER_TX.with(|cell| *cell.borrow_mut() = Some(worker_tx.clone()));
                // P1·SP-14：把捕获出口挂到本线程（console 原生按线程局部取它）
                install_capture(capture_for_thread);
                TOKIO_HANDLE.with(|cell| *cell.borrow_mut() = tokio_handle);

                // 自动装配 P0 基础运行底座（**按声明**：SP-13 的注入清单）
                if let Err(err) = crate::env::inject_environment(&mut ctx, ws_for_thread.as_deref(), needs) {
                    tracing::error!("P0 基础运行底座注入失败: {err}");
                }

                // 注入基础全局能力，例如 console (log, error, warn, info)
                let console_log = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    // P1·SP-14：先捕获（结构化、可截断、可归属），再决定要不要照旧打印。
                    // 插件路径不打印：直接打宿主 stdout 等于往 host.log 灌水。
                    super::console::capture_console(false, &parts.join(" "));
                    if matches!(console_mode, ConsoleMode::Echo) {
                        println!("{}", parts.join(" "));
                    }
                    tracing::info!("[TS Console] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_info = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    // P1·SP-14：先捕获（结构化、可截断、可归属），再决定要不要照旧打印。
                    // 插件路径不打印：直接打宿主 stdout 等于往 host.log 灌水。
                    super::console::capture_console(false, &parts.join(" "));
                    if matches!(console_mode, ConsoleMode::Echo) {
                        println!("{}", parts.join(" "));
                    }
                    tracing::info!("[TS Console] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_error = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    super::console::capture_console(true, &parts.join(" "));
                    if matches!(console_mode, ConsoleMode::Echo) {
                        eprintln!("{}", parts.join(" "));
                    }
                    tracing::error!("[TS Console Error] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_warn = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    super::console::capture_console(true, &parts.join(" "));
                    if matches!(console_mode, ConsoleMode::Echo) {
                        eprintln!("{}", parts.join(" "));
                    }
                    tracing::warn!("[TS Console Warn] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_obj = boa_engine::object::ObjectInitializer::new(&mut ctx)
                    .function(console_log, boa_engine::js_string!("log"), 0)
                    .function(console_info, boa_engine::js_string!("info"), 0)
                    .function(console_error, boa_engine::js_string!("error"), 0)
                    .function(console_warn, boa_engine::js_string!("warn"), 0)
                    .build();
                ctx.register_global_property(
                    boa_engine::js_string!("console"),
                    console_obj,
                    boa_engine::property::Attribute::all(),
                ).ok();

                // 注入异步工具完成时的原生回调函数
                let tool_return = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                    let call_id = args.get(0).and_then(|v| v.to_u32(ctx).ok()).unwrap_or(0) as u64;
                    let result_str = args.get(1).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
                    let is_error = args.get(2).map(|v| v.to_boolean()).unwrap_or(false);

                    let maybe_sender = PENDING_TOOL_CALLS.with(|cell| cell.borrow_mut().remove(&call_id));
                    if let Some(tx) = maybe_sender {
                        let res = if is_error {
                            Err(result_str)
                        } else {
                            Ok(result_str)
                        };
                        let _ = tx.send(res);
                    }
                    Ok(JsValue::undefined())
                });
                ctx.register_global_callable(boa_engine::js_string!("__native_tool_return"), 3, tool_return).ok();

                // 注入异步工具调度与错误捕获桥接器
                let bridge_bootstrap = r#"
                    globalThis.__dispatch_tool_call = async function(callId, fnName, argsJson) {
                        try {
                            let args = null;
                            try {
                                args = JSON.parse(argsJson);
                            } catch (_) {
                                args = argsJson;
                            }
                            const targetFn = globalThis[fnName];
                            if (typeof targetFn !== 'function') {
                                __native_tool_return(callId, "函数 " + fnName + " 未在全局定义", true);
                                return;
                            }
                            const res = await targetFn(args);
                            const outStr = typeof res === 'string' ? res : JSON.stringify(res);
                            __native_tool_return(callId, outStr, false);
                        } catch (err) {
                            // 🔴 带上 `stack`（P4·SP-73）：`String(err)` 只有一句消息，
                            // 没有文件与行号——插件作者只能靠猜。
                            const detail = err && err.stack ? String(err.stack) : String(err);
                            __native_tool_return(callId, detail, true);
                        }
                    };
                "#;
                let _ = ctx.eval(Source::from_bytes(bridge_bootstrap));

                while let Ok(msg) = rx.recv() {
                    match msg {
                        EventLoopMsg::Execute { source_code, filename, response_tx } => {
                            // 1. OXC 内存中微秒级类型擦除与模块转换
                            let js_code = match crate::compiler::transpile_ts_module(&source_code, filename.as_deref()) {
                                Ok(code) => code,
                                Err(err) => {
                                    let _ = response_tx.send(Err(err));
                                    continue;
                                }
                            };

                            // 2. Boa 引擎执行纯 JS
                            // P4·SP-73：把**源码名**带上（Boa 的栈回溯用它定位）。
                            // 不带的话报错长这样：`ReferenceError: x is not defined (unknown at :4:2)`
                            // ——文件是 "unknown"、行号也没法跟编辑器对上，作者只能靠猜。
                            let eval_res = match filename.as_deref() {
                                Some(name) => ctx.eval(
                                    Source::from_bytes(&js_code)
                                        .with_path(std::path::Path::new(name)),
                                ),
                                None => ctx.eval(Source::from_bytes(&js_code)),
                            };

                            // 3. 立即排空本次执行产生的微任务队列
                            let _ = ctx.run_jobs();

                            let response = eval_res
                                .map(|val| val.to_string(&mut ctx).unwrap_or_default().to_std_string_escaped())
                                .map_err(|err| {
                                    // 撞限额与"脚本自己抛错"必须能分开：前者是可纠正的
                                    // （调用方映射成 Timeout 回执），后者是脚本的错。
                                    match limit_kind(&err) {
                                        Some(kind) => {
                                            format!("{LIMIT_MARKER} {kind} exceed; JS 执行异常: {err}")
                                        }
                                        None => format!("JS 执行异常: {err}"),
                                    }
                                });

                            let _ = response_tx.send(response);
                        }
                        EventLoopMsg::AsyncJobDone { job_id, result } => {
                            // 从当前线程的线程本地字典中安全取出对应的 resolvers
                            let maybe_resolvers = PENDING_PROMISES.with(|cell| cell.borrow_mut().remove(&job_id));

                            if let Some(resolvers) = maybe_resolvers {
                                match result {
                                    Ok(val) => {
                                        let js_val = JsValue::from(boa_engine::js_string!(val.as_str()));
                                        resolvers.resolve.call(&JsValue::undefined(), &[js_val], &mut ctx).ok();
                                    }
                                    Err(err) => {
                                        let js_err = JsValue::from(boa_engine::js_string!(err.as_str()));
                                        resolvers.reject.call(&JsValue::undefined(), &[js_err], &mut ctx).ok();
                                    }
                                }
                                // 关键时序：每次宏任务完成 resolve/reject 之后，必须立即排空微任务
                                let _ = ctx.run_jobs();
                            }

                            // 检查所有异步任务是否已完全排空，若是则唤醒等待空闲的线程
                            let is_empty = PENDING_PROMISES.with(|cell| cell.borrow().is_empty());
                            if is_empty {
                                let waiters = PENDING_IDLE_WAITERS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
                                for w in waiters {
                                    let _ = w.send(());
                                }
                            }
                        }
                        EventLoopMsg::RegisterAsyncFn { name, handler } => {
                            ASYNC_HANDLERS.with(|cell| {
                                cell.borrow_mut().insert(name, handler);
                            });

                            let native = NativeFunction::from_copy_closure(move |_this, args, ctx| {
                                let job_id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
                                let string_args: Vec<String> = args
                                    .iter()
                                    .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                                    .collect();

                                let promise = JsPromise::new(
                                    |resolvers, _context| {
                                        // 直接将 resolvers 存入当前线程内部的 thread_local 字典，零跨线程逃逸！
                                        PENDING_PROMISES.with(|cell| {
                                            cell.borrow_mut().insert(job_id, resolvers.clone());
                                        });
                                        Ok(JsValue::undefined())
                                    },
                                    ctx,
                                )
                                .map_err(|e| e)?;

                                let async_h = ASYNC_HANDLERS.with(|h| h.borrow().get(name).cloned());
                                let async_tx = WORKER_TX.with(|tx| tx.borrow().clone());
                                let rt_opt = TOKIO_HANDLE.with(|cell| cell.borrow().clone());

                                if let (Some(h), Some(tx), Some(rt)) = (async_h, async_tx, rt_opt) {
                                    rt.spawn(async move {
                                        let res = h(string_args).await;
                                        let _ = tx.send(EventLoopMsg::AsyncJobDone {
                                            job_id,
                                            result: res,
                                        });
                                    });
                                }

                                Ok(promise.into())
                            });

                            ctx.register_global_callable(boa_engine::js_string!(name), 1, native).ok();
                        }
                        EventLoopMsg::CallAsyncFunction { fn_name, args_json, response_tx } => {
                            let call_id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
                            PENDING_TOOL_CALLS.with(|cell| cell.borrow_mut().insert(call_id, response_tx));

                            let js_invocation = format!(
                                "globalThis.__dispatch_tool_call({}, {}, {});",
                                call_id,
                                serde_json::to_string(&fn_name).unwrap_or_else(|_| "\"\"".to_string()),
                                serde_json::to_string(&args_json).unwrap_or_else(|_| "\"\"".to_string())
                            );

                            // 🔴 **不许吞这次 eval 的错误**（老实现是 `let _ =`）。
                            //
                            // 撞上进程内限额时，`__dispatch_tool_call` 会中途抛错：
                            // 那个 Promise **永远不会 resolve/reject**，于是调用方只能白等到
                            // 墙钟超时——表现正是"工具卡片卡住"。把错误立刻回给等待方，
                            // 既能立刻返回，又能让上层把"撞限额"映射成一条可纠正的回执。
                            match ctx.eval(Source::from_bytes(&js_invocation)) {
                                Ok(_) => {
                                    let _ = ctx.run_jobs();
                                }
                                Err(err) => {
                                    let msg = match limit_kind(&err) {
                                        Some(kind) => {
                                            format!("{LIMIT_MARKER} {kind} exceed; JS 执行异常: {err}")
                                        }
                                        None => format!("JS 执行异常: {err}"),
                                    };
                                    if let Some(tx) =
                                        PENDING_TOOL_CALLS.with(|cell| cell.borrow_mut().remove(&call_id))
                                    {
                                        let _ = tx.send(Err(msg));
                                    }
                                }
                            }
                        }
                        EventLoopMsg::ChildEvent { client_id, event } => {
                            // 子进程事件是**增量**到达的：每块输出都经这里投给 JS 侧，
                            // 于是插件能在进程还在跑的时候就处理数据（真流式）。
                            dispatch_child_event(&mut ctx, &client_id, &event);
                        }
                        EventLoopMsg::WaitIdle { response_tx } => {
                            let is_empty = PENDING_PROMISES.with(|cell| cell.borrow().is_empty());
                            if is_empty {
                                let _ = response_tx.send(());
                            } else {
                                PENDING_IDLE_WAITERS.with(|cell| cell.borrow_mut().push(response_tx));
                            }
                        }
                        EventLoopMsg::Terminate => {
                            // 🔴 子进程不能活过运行时的账：插件跑起来的 `npm run dev`
                            // 一类东西留在系统里，用户既看不见、也不知道是哪个插件的。
                            crate::env::child_process_async::kill_all();
                            break;
                        }
                    }
                }

                // 🔴 另一个出口同样要收尾：宿主没调 `terminate()` 就把运行时扔了，
                // 于是 `rx.recv()` 返回 Err 直接跳出循环。子进程不能因为
                // “没人记得调清理” 就活下来。`kill_all` 幂等，多跑一次无害。
                crate::env::child_process_async::kill_all();
            })
            .expect("创建纯 Rust TS 事件循环线程失败");

    let event_loop_tx = tx.clone();
    let runtime = PureTsRuntime {
        sender: tx,
        console: capture,
    };

    super::natives::register(&runtime, workspace, needs, net, event_loop_tx);
    runtime
}

/// 把一条子进程事件投递给 JS 侧的 `__child_process_dispatch`。
///
/// # 为什么用 `eval` 拼一段调用，而不是缓存 `JsObject` 直接 `call`
///
/// JS 函数对象的生命周期归 Boa 的 GC 管：把它吊在线程局部里跨消息复用，
/// 要么漏 root（被回收后调用就炸）要么漏解绑（泄漏）。而这个 `eval` 只有一行、
/// 开销与既有的 `CallAsyncFunction` 同量级，换来的是"没有悬空句柄"。
fn dispatch_child_event(
    ctx: &mut boa_engine::Context,
    client_id: &str,
    event: &crate::env::child_process_async::ChildEvent,
) {
    let payload = serde_json::to_string(&event.to_json()).unwrap_or_else(|_| "null".to_string());
    let client = serde_json::to_string(client_id).unwrap_or_else(|_| "\"\"".to_string());
    let script = format!(
        "(function(){{ var dispatch = globalThis.__child_process_dispatch; \
         if (typeof dispatch === 'function') {{ dispatch(String({client}), {payload}); }} }})();"
    );
    // U+2028 / U+2029 在 JS 里是**行终止符**（在 JSON 里不是）：不转义就会把这段脚本
    // 从中间断开、变成一个语法错误。它们完全可能出现在子进程输出里（任何 UTF-8 文本）。
    let script = script
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    if let Err(error) = ctx.eval(Source::from_bytes(&script)) {
        tracing::warn!("child_process 事件分发失败: {error}");
    }
    // 与 AsyncJobDone 同一时序：每批数据落地后立刻排空微任务，
    // 否则 `on('data')` 里的 `await` 要等到下一次无关事件才被唤醒。
    let _ = ctx.run_jobs();
}
