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
    /// 等待所有未决任务排空的回调通道队列
    static PENDING_IDLE_WAITERS: RefCell<Vec<oneshot::Sender<()>>> = RefCell::new(Vec::new());
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
    /// 等待当前所有未决 Promise 与微任务全部完成（达到空闲状态）
    WaitIdle {
        response_tx: oneshot::Sender<()>,
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
    /// 初始化并启动专有事件循环 Actor（使用默认工作区）
    pub fn new() -> Self {
        Self::with_workspace(None)
    }

    /// 初始化并指定工作区绝对路径
    pub fn with_workspace(workspace: Option<std::path::PathBuf>) -> Self {
        let (tx, rx) = channel::<EventLoopMsg>();
        let worker_tx = tx.clone();
        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        let ws_for_thread = workspace.clone();

        thread::Builder::new()
            .name("pure-ts-event-loop".into())
            .spawn(move || {
                let mut ctx = Context::default();
                WORKER_TX.with(|cell| *cell.borrow_mut() = Some(worker_tx.clone()));
                TOKIO_HANDLE.with(|cell| *cell.borrow_mut() = tokio_handle);

                // 自动装配 P0 基础运行底座 (process, path, Buffer, EventEmitter, require)
                if let Err(err) = crate::env::inject_p0_environment(&mut ctx, ws_for_thread.as_deref()) {
                    tracing::error!("P0 基础运行底座注入失败: {err}");
                }

                // 注入基础全局能力，例如 console (log, error, warn, info)
                let console_log = NativeFunction::from_copy_closure(|_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    println!("{}", parts.join(" "));
                    tracing::info!("[TS Console] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_info = NativeFunction::from_copy_closure(|_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    println!("{}", parts.join(" "));
                    tracing::info!("[TS Console] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_error = NativeFunction::from_copy_closure(|_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    eprintln!("{}", parts.join(" "));
                    tracing::error!("[TS Console Error] {}", parts.join(" "));
                    Ok(JsValue::undefined())
                });

                let console_warn = NativeFunction::from_copy_closure(|_this, args, ctx| {
                    let parts: Vec<String> = args
                        .iter()
                        .map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped())
                        .collect();
                    eprintln!("{}", parts.join(" "));
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
                            // 1. OXC 内存中微秒级类型擦除与模块转换
                            let js_code = match crate::compiler::transpile_ts_module(&source_code, filename.as_deref()) {
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

                            let _ = ctx.eval(Source::from_bytes(&js_invocation));
                            let _ = ctx.run_jobs();
                        }
                        EventLoopMsg::WaitIdle { response_tx } => {
                            let is_empty = PENDING_PROMISES.with(|cell| cell.borrow().is_empty());
                            if is_empty {
                                let _ = response_tx.send(());
                            } else {
                                PENDING_IDLE_WAITERS.with(|cell| cell.borrow_mut().push(response_tx));
                            }
                        }
                        EventLoopMsg::Terminate => break,
                    }
                }
            })
            .expect("创建纯 Rust TS 事件循环线程失败");

        let runtime = Self { sender: tx };

        // 自动注册 Tokio 异步 fs 原生能力并附加沙箱安全防御
        let ws_r = workspace.clone();
        runtime.register_async_fn("__native_fs_read_file_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_r.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                tokio::fs::read_to_string(&safe).await.map_err(|e| format!("读取失败: {e}"))
            }
        });

        let ws_w = workspace.clone();
        runtime.register_async_fn("__native_fs_write_file_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let content = args.get(1).cloned().unwrap_or_default();
            let base = ws_w.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                if let Some(parent) = safe.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                tokio::fs::write(&safe, content).await.map_err(|e| format!("写入失败: {e}"))?;
                Ok("ok".to_string())
            }
        });

        let ws_mkdir = workspace.clone();
        runtime.register_async_fn("__native_fs_mkdir_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_mkdir.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                tokio::fs::create_dir_all(&safe).await.map_err(|e| format!("创建目录失败: {e}"))?;
                Ok("ok".to_string())
            }
        });

        let ws_readdir = workspace.clone();
        runtime.register_async_fn("__native_fs_readdir_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_readdir.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                let mut entries = Vec::new();
                let mut dir = tokio::fs::read_dir(&safe).await.map_err(|e| format!("读取目录失败: {e}"))?;
                while let Ok(Some(entry)) = dir.next_entry().await {
                    if let Ok(name) = entry.file_name().into_string() {
                        entries.push(name);
                    }
                }
                Ok(serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()))
            }
        });

        let ws_stat = workspace.clone();
        runtime.register_async_fn("__native_fs_stat_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_stat.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                let meta = tokio::fs::metadata(&safe).await.map_err(|e| format!("获取元数据失败: {e}"))?;
                let size = meta.len();
                let is_file = meta.is_file();
                let is_dir = meta.is_dir();
                let mtime_ms = meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                Ok(serde_json::json!({
                    "size": size,
                    "isFile": is_file,
                    "isDirectory": is_dir,
                    "mtimeMs": mtime_ms
                }).to_string())
            }
        });

        let ws_rm = workspace.clone();
        runtime.register_async_fn("__native_fs_rm_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_rm.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                if safe.is_dir() {
                    tokio::fs::remove_dir_all(&safe).await.map_err(|e| format!("删除目录失败: {e}"))?;
                } else if safe.exists() {
                    tokio::fs::remove_file(&safe).await.map_err(|e| format!("删除文件失败: {e}"))?;
                }
                Ok("ok".to_string())
            }
        });

        // 自动注册 Tokio 驱动的原生异步 HTTP fetch 桥接能力
        runtime.register_async_fn("__native_fetch_async", move |args| {
            let url = args.get(0).cloned().unwrap_or_default();
            let opts_str = args.get(1).cloned().unwrap_or_default();
            async move {
                let (method, headers, body_str, proxy_str) = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&opts_str) {
                    let m = val.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_string();
                    let mut h_map = std::collections::HashMap::new();
                    if let Some(h_obj) = val.get("headers").and_then(|v| v.as_object()) {
                        for (k, v) in h_obj {
                            if let Some(vs) = v.as_str() {
                                h_map.insert(k.clone(), vs.to_string());
                            }
                        }
                    }
                    let b = val.get("body").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let p = val.get("proxy").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                    (m, h_map, b, p)
                } else {
                    ("GET".to_string(), std::collections::HashMap::new(), None, None)
                };

                // 获取代理设置：优先使用请求显式指定的 proxy，其次读取环境变量
                let effective_proxy = proxy_str.or_else(|| {
                    std::env::var("HTTPS_PROXY")
                        .or_else(|_| std::env::var("https_proxy"))
                        .or_else(|_| std::env::var("HTTP_PROXY"))
                        .or_else(|_| std::env::var("http_proxy"))
                        .or_else(|_| std::env::var("ALL_PROXY"))
                        .or_else(|_| std::env::var("all_proxy"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                });

                let mut client_builder = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30));

                if let Some(ref proxy_url) = effective_proxy {
                    match reqwest::Proxy::all(proxy_url) {
                        Ok(p) => {
                            client_builder = client_builder.proxy(p);
                        }
                        Err(e) => {
                            tracing::warn!("配置 HTTP 代理 '{}' 失败: {}", proxy_url, e);
                        }
                    }
                }

                let client = client_builder
                    .build()
                    .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;

                let http_method = match method.to_uppercase().as_str() {
                    "GET" => reqwest::Method::GET,
                    "POST" => reqwest::Method::POST,
                    "PUT" => reqwest::Method::PUT,
                    "DELETE" => reqwest::Method::DELETE,
                    "PATCH" => reqwest::Method::PATCH,
                    "HEAD" => reqwest::Method::HEAD,
                    _ => reqwest::Method::GET,
                };

                let mut req = client.request(http_method, &url);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                if let Some(b) = body_str {
                    req = req.body(b);
                }

                let resp = req.send().await.map_err(|e| format!("HTTP 请求失败: {e}"))?;
                let status = resp.status().as_u16();
                let status_text = resp.status().canonical_reason().unwrap_or("").to_string();

                let mut resp_headers = std::collections::HashMap::new();
                for (k, v) in resp.headers() {
                    if let Ok(val_str) = v.to_str() {
                        resp_headers.insert(k.as_str().to_lowercase(), val_str.to_string());
                    }
                }

                let body_text = resp.text().await.map_err(|e| format!("读取响应文本失败: {e}"))?;

                let out_json = serde_json::json!({
                    "status": status,
                    "statusText": status_text,
                    "ok": (200..300).contains(&status),
                    "headers": resp_headers,
                    "body": body_text
                });

                Ok(out_json.to_string())
            }
        });

        // 自动注册 Tokio 驱动的原生异步 sleep 桥接能力（赋能 setTimeout / setInterval）
        runtime.register_async_fn("__native_sleep_async", move |args| {
            let ms_str = args.get(0).cloned().unwrap_or_else(|| "0".to_string());
            let ms = ms_str.parse::<u64>().unwrap_or(0);
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Ok("ok".to_string())
            }
        });

        runtime
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

    /// 等待当前所有未决异步任务排空（空闲）
    pub async fn wait_idle(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(EventLoopMsg::WaitIdle { response_tx: tx })
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| format!("等待空闲被中断: {e}"))
    }

    /// 设置微内核中 process.argv 命令行参数列表
    pub async fn set_process_argv(&self, argv: Vec<String>) -> Result<(), String> {
        let json = serde_json::to_string(&argv).map_err(|e| e.to_string())?;
        self.eval_ts(format!("if (typeof process !== 'undefined') {{ process.argv = {json}; }}"), None).await?;
        Ok(())
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

    /// 终止专有事件循环并释放后台线程资源
    pub fn terminate(&self) {
        let _ = self.sender.send(EventLoopMsg::Terminate);
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

    #[tokio::test]
    async fn test_pure_ts_runtime_p0_apis() {
        let ws = std::path::PathBuf::from("E:/codes/mock_project");
        let runtime = PureTsRuntime::with_workspace(Some(ws));

        let ts_code = r#"
            // 验证 process 全局对象
            const cwd = process.cwd();
            const isWin = process.platform === "win32";

            // 验证 path 模块
            const fullPath = path.join(cwd, "src", "index.ts");
            const ext = path.extname(fullPath);

            // 验证 Buffer
            const buf = Buffer.from("Antigravity Pure Rust", "utf-8");
            const bufStr = buf.toString("utf-8");

            // 验证 EventEmitter
            const ee = new EventEmitter();
            let triggered = false;
            ee.on("event", (val: boolean) => {
                triggered = val;
            });
            ee.emit("event", true);

            // 验证 require 虚拟模块
            const pathReq = require("node:path");
            const sameExt = pathReq.extname("sample.tsx");

            // 验证 node:crypto
            const cryptoReq = require("node:crypto");
            const cryptoHash = cryptoReq.createHash("sha256").update("PureTsRuntime").digest("hex");

            JSON.stringify({
                cwd,
                isWin,
                ext,
                bufStr,
                triggered,
                sameExt,
                cryptoHash
            });
        "#;

        let result = runtime.eval_ts(ts_code, Some("p0_test.ts")).await.expect("执行 P0 测试失败");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("结果应为合法 JSON");

        assert!(parsed["cwd"].as_str().unwrap().contains("mock_project"));
        assert_eq!(parsed["ext"], ".ts");
        assert_eq!(parsed["bufStr"], "Antigravity Pure Rust");
        assert_eq!(parsed["triggered"], true);
        assert_eq!(parsed["sameExt"], ".tsx");
        assert_eq!(parsed["cryptoHash"], "d9cd7b8c78b9481c8c22505db00a81514b5e772258069babbb0a9b9792c98510");
    }

    #[tokio::test]
    async fn test_pure_ts_runtime_async_fs_and_sandbox() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_async_fs_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let runtime = PureTsRuntime::with_workspace(Some(temp_dir.clone()));

        let ts_code = r##"
            // @ts-ignore
            globalThis.asyncFsTest = async function(): Promise<string> {
                const fsPromises = require("node:fs/promises");
                
                // 1. 异步写入文件
                await fsPromises.writeFile("async_note.md", "Hello Tokio Async FS");

                // 2. 异步读取文件
                const readBack = await fsPromises.readFile("async_note.md", "utf-8");

                // 3. 异步沙箱越界拦截验证
                let sandboxBlocked = false;
                try {
                    await fsPromises.readFile("../../../../../etc_shadow.txt", "utf-8");
                } catch (err) {
                    sandboxBlocked = true;
                }

                return JSON.stringify({
                    readBack,
                    sandboxBlocked
                });
            };
        "##;

        runtime.eval_ts(ts_code, Some("async_fs.ts")).await.expect("加载 async fs 测试脚本失败");

        let res_json_str = runtime.call_async_fn("asyncFsTest", "{}").await.expect("执行 asyncFsTest 失败");
        let parsed: serde_json::Value = serde_json::from_str(&res_json_str).expect("返回值必须为合法 JSON");

        assert_eq!(parsed["readBack"], "Hello Tokio Async FS");
        assert_eq!(parsed["sandboxBlocked"], true);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_pure_ts_runtime_global_fetch() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // 启动后台本地 Mock HTTP 服务器
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"message":"Hello from Mock Server!","code":200}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        let runtime = PureTsRuntime::new();
        let url = format!("http://127.0.0.1:{port}/api/greet");

        let ts_code = format!(r#"
            // @ts-ignore
            globalThis.fetchTest = async function(): Promise<string> {{
                const res = await fetch("{url}");
                const status = res.status;
                const ok = res.ok;
                const text = await res.text();
                const json = JSON.parse(text);
                return JSON.stringify({{
                    status,
                    ok,
                    text,
                    code: json.code
                }});
            }};
        "#);

        runtime.eval_ts(ts_code, Some("fetch_test.ts")).await.expect("加载 fetch 测试脚本失败");

        let res_json_str = runtime.call_async_fn("fetchTest", "{}").await.expect("调用 fetchTest 失败");
        let parsed: serde_json::Value = serde_json::from_str(&res_json_str).expect("解析 JSON 失败");

        assert_eq!(parsed["status"], 200);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["code"], 200);
        assert!(parsed["text"].as_str().unwrap().contains("Hello from Mock Server!"));
    }

    #[tokio::test]
    async fn test_pure_ts_runtime_timers_and_wait_idle() {
        let runtime = PureTsRuntime::new();

        let ts_code = r#"
            // @ts-ignore
            globalThis.timerState = {
                timeoutFired: false,
                cancelledFired: false,
                promiseValue: null
            };

            // 1. 设置正常触发的 setTimeout
            setTimeout(() => {
                // @ts-ignore
                globalThis.timerState.timeoutFired = true;
            }, 30);

            // 2. 设置被取消的 setTimeout
            // @ts-ignore
            const cancelId = setTimeout(() => {
                // @ts-ignore
                globalThis.timerState.cancelledFired = true;
            }, 50);
            clearTimeout(cancelId);

            // 3. 验证 node:timers/promises
            const timersPromises = require("node:timers/promises");
            timersPromises.setTimeout(40, "AntigravityTimerOK").then((val: string) => {
                // @ts-ignore
                globalThis.timerState.promiseValue = val;
            });
        "#;

        runtime.eval_ts(ts_code, Some("timers_test.ts")).await.expect("执行定时器脚本失败");

        // 等待所有未决定时器宏任务排空
        runtime.wait_idle().await.expect("等待空闲失败");

        let check_res = runtime.eval_ts("JSON.stringify(globalThis.timerState)", None).await.expect("查询状态失败");
        let parsed: serde_json::Value = serde_json::from_str(&check_res).expect("JSON 解析失败");

        assert_eq!(parsed["timeoutFired"], true);
        assert_eq!(parsed["cancelledFired"], false);
        assert_eq!(parsed["promiseValue"], "AntigravityTimerOK");
    }
}

