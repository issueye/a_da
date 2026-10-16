use boa_engine::{
    builtins::promise::ResolvingFunctions,
    error::EngineError,
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
    /// 当前线程的控制台捕获出口（P1·SP-14）。
    ///
    /// 用线程局部而不是往闭包里捕获 `Arc`：console 原生是 `from_copy_closure`（无捕获），
    /// 而**每个运行时独占一个事件循环线程**——于是线程局部正好等于"每个运行时一份"，
    /// 不用为了捕获把 Boa 的闭包签名改成需要 `Trace` 的那种。
    static CONSOLE_CAPTURE: RefCell<Option<Arc<std::sync::Mutex<ConsoleCapture>>>> =
        RefCell::new(None);
}

/// 控制台单流的捕获上限（字节）。超出后只累计字节数，不再往缓冲里写。
pub const CONSOLE_CAPTURE_LIMIT: usize = 8 * 1024;

/// 控制台输出是否**同时**照旧打到进程 stdout。
///
/// 插件路径用 [`ConsoleMode::Capture`]：插件的 `console.log` 直接打宿主 stdout 就是
/// 往 `host.log` 里灌水，既不归属、也不限长。CLI（`ts_engine run/eval`）用 `Echo`——
/// 那是用户自己写的脚本，打印就是它的输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleMode {
    Echo,
    Capture,
}

/// 结构化控制台捕获（每运行时一份）。
#[derive(Default)]
pub struct ConsoleCapture {
    stdout: String,
    stderr: String,
    stdout_bytes: u64,
    stderr_bytes: u64,
    truncated: bool,
}

impl ConsoleCapture {
    fn push(&mut self, is_err: bool, line: &str) {
        let text = if is_err { &mut self.stderr } else { &mut self.stdout };
        let total = if is_err { &mut self.stderr_bytes } else { &mut self.stdout_bytes };
        *total += line.len() as u64 + 1; // +1 是换行
        if text.len() + line.len() + 1 > CONSOLE_CAPTURE_LIMIT {
            // 到顶就不再写，但**字节数继续如实累计**（截断了多少要看得出来）
            self.truncated = true;
            return;
        }
        text.push_str(line);
        text.push('\n');
    }

    /// 取走并清空（按调用归属用）。
    fn take(&mut self) -> ConsoleSnapshot {
        ConsoleSnapshot {
            stdout: std::mem::take(&mut self.stdout),
            stderr: std::mem::take(&mut self.stderr),
            stdout_bytes: std::mem::take(&mut self.stdout_bytes),
            stderr_bytes: std::mem::take(&mut self.stderr_bytes),
            truncated: std::mem::replace(&mut self.truncated, false),
        }
    }
}

/// 一次调用期间捕获到的控制台输出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleSnapshot {
    /// 已截断的 stdout 文本（超过上限的部分不在里面）
    pub stdout: String,
    pub stderr: String,
    /// **原始**字节数（截断前），与文本长度可能不一致——这正是"截断了多少"的依据
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub truncated: bool,
}

impl ConsoleSnapshot {
    pub fn is_empty(&self) -> bool {
        self.stdout_bytes == 0 && self.stderr_bytes == 0
    }
}

/// console 原生往捕获里写一行（本线程没接捕获时什么都不做）。
fn capture_console(is_err: bool, line: &str) {
    CONSOLE_CAPTURE.with(|cell| {
        if let Some(arc) = cell.borrow().as_ref() {
            if let Ok(mut guard) = arc.lock() {
                guard.push(is_err, line);
            }
        }
    });
}

type AsyncHandler = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, Result<String, String>> + Send + Sync + 'static>;

/// 沙箱运行限额（P1 · SP-12）。
///
/// # 为什么必须有
///
/// Boa 的循环迭代上限默认是 `u64::MAX`（等于**不限**），而沙箱的 `terminate` 只是往事件循环
/// 发一条消息——**同步死循环期间它根本排不上队**（消息要等当前执行返回才被处理）。
/// 结果是 `while(true){}` 把 worker 线程占死：宿主只能超时后放弃等待，那条线程再也回不来。
///
/// 有了进程内的指令上限，同步段**必然返回**（Boa 抛 `RuntimeLimitError`），超时才能变成
/// 一条可纠正的回执，而不是"线程卡住"。
///
/// 默认值取"**宽但有限**"：正常脚本碰不到，死循环必然撞上（见 [`Limits::default`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 循环迭代上限；`0` 表示不限（与 Boa 的 `u64::MAX` 同义）。
    pub loop_iterations: u64,
    /// 函数递归深度上限。
    pub recursion_depth: usize,
    /// VM 栈上限。
    pub stack_size: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 宽但**有限**：一千万次迭代在 Boa 里约等于数百毫秒到数秒——
            // 正常脚本碰不到，纯死循环必然撞上。
            loop_iterations: 10_000_000,
            // 递归与栈沿用 Boa 的默认值（这两项它本来就有上限，不用我们收紧）
            recursion_depth: 512,
            stack_size: 1024 * 10,
        }
    }
}

impl Limits {
    /// 完全不限（只给"我知道自己在干什么"的场景；默认**不用**）。
    pub fn unlimited() -> Self {
        Self {
            loop_iterations: 0,
            recursion_depth: 512,
            stack_size: 1024 * 10,
        }
    }

    /// 相对默认更紧的限额（测试与将来的紧凑预算用）。
    pub fn tight(loop_iterations: u64) -> Self {
        Self { loop_iterations, ..Self::default() }
    }
}

/// 预算超限错误的**统一标记**。
///
/// 宿主靠它把错误映射成 `Timeout` 回执，**不必去猜 Boa 的文案**（升级 Boa 时文案会变，
/// 标记不会）。`crates/agent-node` 的脚本宿主读它，`plugin-sdk` 的契约也以它为准。
pub const LIMIT_MARKER: &str = "[script-limit]";

/// 错误串是否表示"撞上了沙箱预算"。
pub fn is_limit_error(err: &str) -> bool {
    err.contains(LIMIT_MARKER)
}

/// 把 [`Limits`] 应用到 Boa 的 `Context` 上（唯一一处，避免限额散落）。
fn apply_limits(ctx: &mut Context, limits: Limits) {
    let l = ctx.runtime_limits_mut();
    if limits.loop_iterations == 0 {
        l.disable_loop_iteration_limit();
    } else {
        l.set_loop_iteration_limit(limits.loop_iterations);
    }
    l.set_recursion_limit(limits.recursion_depth);
    l.set_stack_size_limit(limits.stack_size);
}

/// 结构化识别"是不是撞了限额"：Boa 的 `EngineError::RuntimeLimit` 是引擎级错误，
/// 可以从 `JsError` 直接取出来（不用匹配文案）。
fn limit_kind(err: &boa_engine::JsError) -> Option<&'static str> {
    match err.as_engine() {
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::LoopIteration)) => {
            Some("loop_iterations")
        }
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::Recursion)) => {
            Some("recursion_depth")
        }
        Some(EngineError::RuntimeLimit(boa_engine::error::RuntimeLimitError::StackSize)) => {
            Some("stack_size")
        }
        _ => None,
    }
}

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
    /// 控制台捕获（每运行时一份；线程局部指向同一个 `Arc`）。
    console: Arc<std::sync::Mutex<ConsoleCapture>>,
}

impl PureTsRuntime {
    /// 初始化并启动专有事件循环 Actor（使用默认工作区）
    pub fn new() -> Self {
        Self::with_workspace(None)
    }

    /// 初始化并指定工作区绝对路径
    pub fn with_workspace(workspace: Option<std::path::PathBuf>) -> Self {
        Self::with_workspace_and_limits(workspace, Limits::default())
    }

    /// 初始化并指定工作区与运行限额（宿主按预算给限额时用这个）。
    pub fn with_workspace_and_limits(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
    ) -> Self {
        Self::with_environment(workspace, limits, crate::env::Needs::everything())
    }

    /// 初始化并指定工作区、限额与**环境注入清单**（P1 · SP-13）。
    ///
    /// 这是插件路径该用的构造器：`needs` 里没声明的能力**连注入都不做**
    /// （既没有 JS 包装，也没有挂到 `globalThis` 的原生函数）。
    pub fn with_environment(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
    ) -> Self {
        // 老签名保持"照旧打印"（CLI/本地脚本口径），插件路径用下面的显式版本
        Self::with_environment_and_console(workspace, limits, needs, ConsoleMode::Echo)
    }

    /// 同 [`Self::with_environment`]，但显式指定控制台模式（P1 · SP-14）。
    pub fn with_environment_and_console(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
        console_mode: ConsoleMode,
    ) -> Self {
        Self::with_environment_console_and_net(
            workspace,
            limits,
            needs,
            console_mode,
            // 宿主/运维给的策略（P4·SP-72）：环境变量读不到就用默认值
            crate::net::NetPolicy::from_env(),
        )
    }

    /// 同 [`Self::with_environment_and_console`]，但**联网策略由宿主给**（P4·SP-72）：
    /// 超时与域名白名单。默认档见 [`crate::net::NetPolicy`]。
    pub fn with_environment_console_and_net(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
        console_mode: ConsoleMode,
        net: crate::net::NetPolicy,
    ) -> Self {
        let (tx, rx) = channel::<EventLoopMsg>();
        let worker_tx = tx.clone();
        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        let ws_for_thread = workspace.clone();
        let capture = Arc::new(std::sync::Mutex::new(ConsoleCapture::default()));
        let capture_for_thread = capture.clone();
        let net_for_thread = net.clone();

        thread::Builder::new()
            .name("pure-ts-event-loop".into())
            .spawn(move || {
                let mut ctx = Context::default();
                // P1·SP-12：限额在 `Context` 建好之后立刻应用——它是"同步段必然返回"的
                // 唯一依据，晚一步就等于给死循环留了一整段时间窗。
                apply_limits(&mut ctx, limits);
                WORKER_TX.with(|cell| *cell.borrow_mut() = Some(worker_tx.clone()));
                // P1·SP-14：把捕获出口挂到本线程（console 原生按线程局部取它）
                CONSOLE_CAPTURE.with(|cell| *cell.borrow_mut() = Some(capture_for_thread));
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
                    capture_console(false, &parts.join(" "));
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
                    capture_console(false, &parts.join(" "));
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
                    capture_console(true, &parts.join(" "));
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
                    capture_console(true, &parts.join(" "));
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

        let runtime = Self {
            sender: tx,
            console: capture,
        };

        // 自动注册 Tokio 异步 fs 原生能力并附加沙箱安全防御
        let ws_r = workspace.clone();
        runtime.register_async_fn_gated(needs.fs_read, "__native_fs_read_file_async", move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_r.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                tokio::fs::read_to_string(&safe).await.map_err(|e| format!("读取失败: {e}"))
            }
        });

        let ws_w = workspace.clone();
        runtime.register_async_fn_gated(needs.fs_write, "__native_fs_write_file_async", move |args| {
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
        runtime.register_async_fn_gated(needs.fs_write, "__native_fs_mkdir_async", move |args| {
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
        runtime.register_async_fn_gated(needs.fs_read, "__native_fs_readdir_async", move |args| {
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
        runtime.register_async_fn_gated(needs.fs_read || needs.fs_write, "__native_fs_stat_async", move |args| {
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
        runtime.register_async_fn_gated(needs.fs_write, "__native_fs_rm_async", move |args| {
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

        // 自动注册 Tokio 驱动的原生异步 HTTP fetch 桥接能力（**按声明**：未声明就整条不注册）
        runtime.register_async_fn_gated(needs.net, "__native_fetch_async", move |args| {
            let url = args.get(0).cloned().unwrap_or_default();
            let policy = net_for_thread.clone();
            let opts_str = args.get(1).cloned().unwrap_or_default();
            async move {
                // 🔴 **策略先于请求**（P4 SP-72）：白名单之外的域名连连接都不该建，
                // 而且失败原因要说人话（插件作者据此知道该改哪个域名）。
                if let Err(reason) = policy.check_url(&url) {
                    return Err(format!("网络请求被宿主策略拒绝：{reason}"));
                }
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

                // 超时来自宿主策略（此前是硬编码 30s）
                let mut client_builder = reqwest::Client::builder().timeout(policy.timeout);

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

    /// 取走本运行时**累计**的控制台输出（P1·SP-14）。
    ///
    /// 取走即清空：调用方按自己的归属口径上报（插件路径按 `plugin_id`）。
    pub fn take_console(&self) -> ConsoleSnapshot {
        match self.console.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => ConsoleSnapshot::default(),
        }
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

    /// 按声明注册异步原生：`allowed == false` 时**连全局函数都不注册**。
    ///
    /// 🔴 为什么必须裁到这一层：异步原生是经 `register_global_callable` 挂到 `globalThis`
    /// 上的（`runtime/mod.rs` 的 `RegisterAsyncFn` 处理处）。只关 JS 侧的包装脚本挡不住
    /// `globalThis.__native_fetch_async(...)` 这种**直呼**——那正是"注入后再拦"的漏法。
    pub fn register_async_fn_gated<F, Fut>(&self, allowed: bool, name: &'static str, async_logic: F)
    where
        F: Fn(Vec<String>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        if allowed {
            self.register_async_fn(name, async_logic);
        }
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

    /// P4·SP-73：运行时错误的栈里要能看见**文件名与行号**。
    ///
    /// 不带源码名时 Boa 报的是 `(unknown at :4:2)`——文件是 "unknown"、行号也没法跟编辑器
    /// 对上，插件作者只能靠猜。这条用例断言"名字真的传下去了"。
    #[tokio::test]
    async fn test_runtime_error_carries_the_source_name_and_line() {
        let runtime = PureTsRuntime::with_workspace(None);
        let err = runtime
            .eval_ts(
                "function boom() {\n  return missingThing();\n}\nboom();\n",
                Some("plugins/probe.ts"),
            )
            .await
            .expect_err("引用未定义的名字必须报错");
        assert!(
            err.contains("plugins/probe.ts"),
            "错误文案里必须出现源码名（否则作者不知道是哪个文件）：{err}"
        );
        assert!(
            !err.contains("unknown at"),
            "不该再出现 `unknown at`（那等于没有位置信息）：{err}"
        );
        // 位置由 Boa 给出（调用点 / 定义点各一条），只断言"是文件里的某一行"
        let has_line = (1..=4).any(|n| err.contains(&format!("plugins/probe.ts:{n}:")));
        assert!(has_line, "要点回源码里的某一行：{err}");
    }

    /// SP-12 的核心判据：**同步死循环必须必然返回**。
    ///
    /// 为什么这条用例是 P1 的关键：`terminate` 只是往事件循环发消息，同步段期间它排不上队，
    /// 所以"取消/超时"在死循环面前完全无效——只有进程内的指令上限能让同步段回来。
    #[tokio::test]
    async fn test_loop_limit_turns_sync_infinite_loop_into_an_error() {
        let runtime = PureTsRuntime::with_workspace_and_limits(None, Limits::tight(100_000));
        let started = std::time::Instant::now();
        let res = runtime.eval_ts("while (true) {}", None).await;
        let elapsed = started.elapsed();

        let err = res.expect_err("死循环必须被限额打断，而不是永远跑不完");
        assert!(is_limit_error(&err), "错误必须带限额标记（宿主靠它映射成 Timeout）：{err}");
        assert!(err.contains("loop_iterations"), "要能看出撞的是哪一项限额：{err}");
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "必须在很短时间内返回，实际 {elapsed:?}"
        );
        runtime.terminate();
    }

    /// 递归上限同样生效（证明限额是**三项**都接上了，不是只接了循环那一项）。
    #[tokio::test]
    async fn test_recursion_limit_is_reported() {
        let runtime = PureTsRuntime::with_workspace(None);
        let res = runtime
            .eval_ts("function f(){ return f() + 1 } f()", None)
            .await;
        let err = res.expect_err("无限递归必须被递归上限打断");
        assert!(is_limit_error(&err), "递归超限也要带限额标记：{err}");
        assert!(err.contains("recursion_depth"), "{err}");
        runtime.terminate();
    }

    /// SP-13：**原生桥也必须按声明裁剪**。
    ///
    /// 🔴 这条守的是一个真实的绕过路径：异步原生是经 `register_global_callable` 挂到
    /// `globalThis` 上的，只关 JS 侧的 `fetch` 包装脚本挡不住
    /// `globalThis.__native_fetch_async(...)` 这种**直呼**。
    #[tokio::test]
    async fn test_net_needs_gates_the_native_bridge_too() {
        // 默认（插件口径）：网络关 → 包装不存在，原生桥也不存在
        let closed = PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::plugin_default());
        for expr in ["typeof fetch", "typeof globalThis.__native_fetch_async"] {
            let got = closed.eval_ts(expr, None).await.expect("eval 应当成功");
            assert_eq!(got, "undefined", "`{expr}` 在默认档下必须不存在");
        }
        closed.terminate();

        // 全开：两者都在
        let open = PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::everything());
        for expr in ["typeof fetch", "typeof globalThis.__native_fetch_async"] {
            let got = open.eval_ts(expr, None).await.expect("eval 应当成功");
            assert_eq!(got, "function", "`{expr}` 在声明了网络之后必须存在");
        }
        open.terminate();
    }

    /// SP-14：控制台输出必须**被结构化捕获**，且超限时**如实标注截断**。
    ///
    /// 判据里同时断言"原始字节数 > 文本长度"——那正是"截了多少"的依据；
    /// 只看文本的话，"只有两行"与"截得只剩两行"分不出来。
    #[tokio::test]
    async fn test_console_output_is_captured_and_truncation_is_marked() {
        let runtime = PureTsRuntime::with_environment_and_console(
            None,
            Limits::default(),
            crate::env::Needs::plugin_default(),
            ConsoleMode::Capture,
        );

        runtime
            .eval_ts(
                "console.log('hello'); console.error('oops');",
                None,
            )
            .await
            .expect("eval 应当成功");

        let snap = runtime.take_console();
        assert!(snap.stdout.contains("hello"), "stdout 必须被捕获：{snap:?}");
        assert!(snap.stderr.contains("oops"), "stderr 必须被捕获：{snap:?}");
        assert!(!snap.truncated);

        // 取走即清空（按调用归属）
        let again = runtime.take_console();
        assert!(again.is_empty(), "取走之后必须清空：{again:?}");

        // 刷屏：文本被截，但**字节数继续如实累计**
        let big = format!("for (let i = 0; i < 5000; i++) console.log('x'.repeat(40));");
        runtime.eval_ts(big, None).await.expect("eval 应当成功");
        let flooded = runtime.take_console();
        assert!(flooded.truncated, "超限必须标记截断");
        assert!(
            (flooded.stdout.len() as u64) < flooded.stdout_bytes,
            "文本必须短于原始字节数（截断的依据）：text={} bytes={}",
            flooded.stdout.len(),
            flooded.stdout_bytes
        );
        assert!(
            flooded.stdout_bytes > CONSOLE_CAPTURE_LIMIT as u64,
            "原始字节数要如实超过上限：{}",
            flooded.stdout_bytes
        );

        runtime.terminate();
    }

    /// 默认限额**必须有限**：`while(true){}` 的危险不该靠"记得传限额"来避免。
    #[test]
    fn test_default_limits_are_finite_but_wide() {
        let d = Limits::default();
        assert!(d.loop_iterations > 0, "默认不限循环 = 死循环会占死线程");
        assert!(d.loop_iterations >= 1_000_000, "默认要给宽值，不能在正常脚本上误伤");
        assert!(d.recursion_depth > 0 && d.stack_size > 0);

        // `tight` 只收循环那一项，另外两项沿用默认（避免测试里连带改语义）
        let t = Limits::tight(123);
        assert_eq!(t.loop_iterations, 123);
        assert_eq!(t.recursion_depth, d.recursion_depth);
        assert_eq!(t.stack_size, d.stack_size);

        // 0 = 不限（显式选择才生效）
        assert_eq!(Limits::unlimited().loop_iterations, 0);
    }

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

    /// P4·SP-72：白名单之外的域名**在建立连接之前**就被拒，且理由能直接给作者看。
    ///
    /// 直接打原生桥（而不是 `fetch(...)`）是为了让断言只落在**策略**这一件事上：
    /// 走 JS 包装层会引入 Promise 与 polyfill 的噪音，而这里要证明的是
    /// "策略先于请求"——真发出去就晚了。
    #[tokio::test]
    async fn test_net_policy_blocks_disallowed_domain_before_the_request() {
        let runtime = PureTsRuntime::with_environment_console_and_net(
            None,
            Limits::default(),
            crate::env::Needs::everything(),
            ConsoleMode::Capture,
            crate::net::NetPolicy::allow_only(&["allowed.example"]),
        );
        let err = runtime
            .call_async_fn(
                "__native_fetch_async",
                r#"["https://evil.example/x", "{}"]"#,
            )
            .await
            .expect_err("白名单之外的域名必须被拒");
        assert!(
            err.contains("网络请求被宿主策略拒绝"),
            "要说明是**策略**拒的（而不是网络不通）：{err}"
        );
        assert!(
            err.contains("evil.example") && err.contains("allowed.example"),
            "要说清是哪个域名、允许的是哪些：{err}"
        );
    }

    /// P4·SP-73：**JS 侧捕获到的**错误也要带得住位置。
    ///
    /// 工具路径的错误是在 JS 里 catch 之后回传的（`__dispatch_tool_call` 的 catch），
    /// 那里现在用 `err.stack` 而不是 `String(err)`——这条用例钉住的前提是
    /// **Boa 真的把 `stack` 暴露成字符串**。若哪天引擎换了实现、`stack` 没了，
    /// 报错会静默退回"只有一句话、没有位置"，而这条会红。
    #[tokio::test]
    async fn test_js_error_exposes_a_stack_string() {
        let runtime = PureTsRuntime::with_environment_and_console(
            None,
            Limits::default(),
            crate::env::Needs::everything(),
            ConsoleMode::Capture,
        );
        let _ = runtime
            .eval_ts(
                "try { undefinedName(); } catch (e) { console.log('TYPEOF=' + typeof e.stack); }\n",
                Some("plugins/probe.ts"),
            )
            .await;
        let out = runtime.take_console().stdout;
        assert!(
            out.contains("TYPEOF=string"),
            "Boa 必须把 `stack` 暴露成字符串（宿主靠它把位置带给插件作者）：{out}"
        );
    }
}

