use crate::compiler::oxc_strip_types;
use boa_engine::{
    builtins::promise::ResolvingFunctions,
    object::builtins::JsPromise,
    Context, JsValue, NativeFunction, Source,
};
use futures_util::future::BoxFuture;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use tokio::sync::oneshot;

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// 仅限专有事件循环单线程内部持有的未决 Promise 解析器映射表（零跨线程移动，完美避开 !Send）
    static PENDING_PROMISES: RefCell<HashMap<u64, ResolvingFunctions>> = RefCell::new(HashMap::new());
    /// 异步工具调用挂起的返回通道映射表
    static PENDING_TOOL_CALLS: RefCell<HashMap<u64, oneshot::Sender<Result<String, String>>>> = RefCell::new(HashMap::new());
    /// 当前事件循环单线程持有的内部消息发送通道
    static WORKER_TX: RefCell<Option<Sender<EventLoopMsg>>> = RefCell::new(None);
    /// 当前事件循环持有的异步处理逻辑
    static ASYNC_HANDLERS: RefCell<HashMap<&'static str, AsyncHandler>> = RefCell::new(HashMap::new());
    /// 当前关联的 Tokio 运行时调度句柄
    static TOKIO_HANDLE: RefCell<Option<tokio::runtime::Handle>> = RefCell::new(None);
}

type AsyncHandler = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, Result<String, String>> + Send + Sync + 'static>;

/// 专有事件循环 Actor 的消息载荷（保证 100% Send）
pub enum EventLoopMsg {
    /// 执行 TypeScript 代码并异步返回结果字符串
    Execute {
        source_code: String,
        filename: Option<String>,
        response_tx: oneshot::Sender<Result<String, String>>,
    },
    /// Tokio 异步任务完成后的纯数据结果回传
    AsyncJobDone {
        job_id: u64,
        result: Result<String, String>,
    },
    /// 注册一个全局异步原生函数
    RegisterAsyncFn {
        name: &'static str,
        handler: AsyncHandler,
    },
    /// 调用全局异步 JavaScript/TypeScript 工具函数并等待异步完成
    CallAsyncFunction {
        fn_name: String,
        args_json: String,
        response_tx: oneshot::Sender<Result<String, String>>,
    },
    /// 终止专有事件循环
    Terminate,
}

/// 纯 Rust 实现的 TypeScript 微内核运行时
#[derive(Clone)]
pub struct PureTsRuntime {
    sender: Sender<EventLoopMsg>,
}

impl PureTsRuntime {
    /// 初始化并启动专有事件循环 Actor
    pub fn new() -> Self {
        let (tx, rx) = channel::<EventLoopMsg>();
        let worker_tx = tx.clone();
        let tokio_handle = tokio::runtime::Handle::try_current().ok();

        thread::Builder::new()
            .name("pure-ts-event-loop".into())
            .spawn(move || {
                let mut ctx = Context::default();
                WORKER_TX.with(|cell| *cell.borrow_mut() = Some(worker_tx.clone()));
                TOKIO_HANDLE.with(|cell| *cell.borrow_mut() = tokio_handle);

                // 注入基础全局能力，例如 console.log
                let console_log = NativeFunction::from_copy_closure(|_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    tracing::info!("[TS Console] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });
                
                let console_obj = boa_engine::object::ObjectInitializer::new(&mut ctx)
                    .function(console_log, boa_engine::js_string!("log"), 0)
                    .build();
                ctx.register_global_property(
                    boa_engine::js_string!("console"),
                    console_obj,
                    boa_engine::property::Attribute::all(),
                ).ok();

                // 注入异步工具完成时的原生回调函数
                let tool_return = NativeFunction::from_copy_closure(|_this, args, ctx| {
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
                            __native_tool_return(callId, String(err), true);
                        }
                    };
                "#;
                let _ = ctx.eval(Source::from_bytes(bridge_bootstrap));

                while let Ok(msg) = rx.recv() {
                    match msg {
                        EventLoopMsg::Execute { source_code, filename, response_tx } => {
                            // 1. OXC 内存中微秒级类型擦除 (Strip Types)
                            let js_code = match oxc_strip_types(&source_code, filename.as_deref()) {
                                Ok(code) => code,
                                Err(err) => {
                                    let _ = response_tx.send(Err(err));
                                    continue;
                                }
                            };

                            // 2. Boa 引擎执行纯 JS
                            let eval_res = ctx.eval(Source::from_bytes(&js_code));
                            
                            // 3. 立即排空本次执行产生的微任务队列
                            let _ = ctx.run_jobs();

                            let response = eval_res
                                .map(|val| val.to_string(&mut ctx).unwrap_or_default().to_std_string_escaped())
                                .map_err(|err| format!("JS 执行异常: {err}"));

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

                            let _ = ctx.eval(Source::from_bytes(&js_invocation));
                            let _ = ctx.run_jobs();
                        }
                        EventLoopMsg::Terminate => break,
                    }
                }
            })
            .expect("创建纯 Rust TS 事件循环线程失败");

        Self { sender: tx }
    }

    /// 执行一段 TypeScript 代码并等待最终字符串返回
    pub async fn eval_ts(&self, ts_code: impl Into<String>, filename: Option<&str>) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(EventLoopMsg::Execute {
            source_code: ts_code.into(),
            filename: filename.map(|s| s.to_string()),
            response_tx: tx,
        }).map_err(|e| e.to_string())?;

        rx.await.map_err(|e| format!("事件循环挂起无响应: {e}"))?
    }

    /// 向微内核注册一个返回 Promise 的 Tokio 异步原生函数
    pub fn register_async_fn<F, Fut>(&self, name: &'static str, async_logic: F)
    where
        F: Fn(Vec<String>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let handler: AsyncHandler = Arc::new(move |args| Box::pin(async_logic(args)));
        let _ = self.sender.send(EventLoopMsg::RegisterAsyncFn { name, handler });
    }

    /// 调用在微内核中注册的全局异步 TypeScript/JavaScript 工具函数
    pub async fn call_async_fn(&self, fn_name: impl Into<String>, args_json: impl Into<String>) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(EventLoopMsg::CallAsyncFunction {
            fn_name: fn_name.into(),
            args_json: args_json.into(),
            response_tx: tx,
        }).map_err(|e| e.to_string())?;

        rx.await.map_err(|e| format!("异步工具执行响应被中断: {e}"))?
    }
}

impl Default for PureTsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pure_ts_runtime_eval_sync() {
        let runtime = PureTsRuntime::new();
        let ts_code = r#"
            interface MathOp {
                a: number;
                b: number;
            }
            const op: MathOp = { a: 15, b: 27 };
            const sum = (x: number, y: number): number => x + y;
            `Result: ${sum(op.a, op.b)}`;
        "#;

        let result = runtime.eval_ts(ts_code, Some("calc.ts")).await.expect("执行失败");
        assert_eq!(result, "Result: 42");
    }

    #[tokio::test]
    async fn test_pure_ts_runtime_async_await_syntax() {
        let runtime = PureTsRuntime::new();
        let ts_code = r#"
            async function compute(): Promise<number> {
                return 100 + 200;
            }
            async function run(): Promise<string> {
                const val = await compute();
                return `Computed: ${val}`;
            }
            run();
        "#;

        let result = runtime.eval_ts(ts_code, Some("async.ts")).await.expect("执行失败");
        assert!(result.contains("Promise") || result.contains("Computed"));
    }

    #[tokio::test]
    async fn test_pure_ts_runtime_tokio_bridge() {
        let runtime = PureTsRuntime::new();

        // 注册一个 Tokio 驱动的原生异步函数
        runtime.register_async_fn("nativeSleepEcho", |args| async move {
            let msg = args.get(0).cloned().unwrap_or_default();
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            Ok(format!("ECHO_{msg}"))
        });

        let ts_code = r#"
            // @ts-ignore
            globalThis.bridgeOutput = "pending";
            async function testBridge(): Promise<void> {
                // @ts-ignore
                const reply = await nativeSleepEcho("Antigravity");
                // @ts-ignore
                globalThis.bridgeOutput = `Got: ${reply}`;
            }
            testBridge();
        "#;

        let result = runtime.eval_ts(ts_code, Some("bridge.ts")).await.expect("执行失败");
        assert!(result.contains("Promise") || result.contains("undefined"));

        // 稍作等待，让 Tokio 异步任务完成并触发 resolve 与 microtask 执行
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let check_res = runtime.eval_ts("globalThis.bridgeOutput;", None).await.expect("查询状态失败");
        assert_eq!(check_res, "Got: ECHO_Antigravity");
    }

    #[tokio::test]
    async fn test_mvp_e2e_ts_tool_execution() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_mvp_e2e_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let target_file = temp_dir.join("hello.txt");
        std::fs::write(&target_file, "Antigravity Pure Rust TS Runtime MVP!").unwrap();

        let runtime = PureTsRuntime::new();

        // 1. 注册沙箱底层异步读取文件的原生函数
        let base_dir = temp_dir.clone();
        runtime.register_async_fn("__native_read_file", move |args| {
            let rel_path = args.get(0).cloned().unwrap_or_default();
            let abs_path = base_dir.join(rel_path);
            async move {
                match tokio::fs::read_to_string(&abs_path).await {
                    Ok(text) => Ok(text),
                    Err(e) => Err(format!("读取文件失败: {e}")),
                }
            }
        });

        // 2. 注入使用真实 TypeScript 语法编写的插件工具代码
        let ts_tool_code = r#"
            interface ReadFileArgs {
                path: string;
            }

            interface ReadFileResult {
                file: string;
                content: string;
                bytes: number;
            }

            // @ts-ignore
            globalThis.ts_read_file = async function(args: ReadFileArgs): Promise<string> {
                // @ts-ignore
                const raw: string = await __native_read_file(args.path);
                const result: ReadFileResult = {
                    file: args.path,
                    content: raw,
                    bytes: raw.length,
                };
                return JSON.stringify(result);
            };
        "#;

        runtime.eval_ts(ts_tool_code, Some("read_file.ts")).await.expect("TS 工具加载失败");

        // 3. 模拟 Agent 模型发出的 ToolCall 派发执行
        let tool_args = serde_json::json!({ "path": "hello.txt" }).to_string();
        let tool_result_str = runtime.call_async_fn("ts_read_file", tool_args).await.expect("TS 工具执行失败");

        // 4. 验证执行结果
        let parsed_result: serde_json::Value = serde_json::from_str(&tool_result_str).expect("工具输出必须为合法 JSON");
        assert_eq!(parsed_result["file"], "hello.txt");
        assert_eq!(parsed_result["content"], "Antigravity Pure Rust TS Runtime MVP!");
        assert_eq!(parsed_result["bytes"], 37);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
