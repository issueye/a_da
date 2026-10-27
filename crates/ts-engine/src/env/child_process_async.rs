//! `child_process` 的**异步执行器**（`spawn` 的宿主侧）。
//!
//! # 职责边界
//!
//! 这里只负责"把进程真的跑起来，并把它的输出与退出**如实**送出去"：
//!
//! - 进程本身用 `tokio::process`，不阻塞事件循环；
//! - `stdout` / `stderr` 各跑一个 reader task，读到一块就推一块（**真流式**，不是攒完再给）；
//! - 事件经由 [`EventSink`] 投出去。这个 sink 由 `runtime` 侧提供（内部把事件包成
//!   `EventLoopMsg::ChildEvent`），所以本模块**不知道** `EventLoopMsg` 的存在——
//!   `env` 不反向依赖 `runtime`，两边各自可测。
//! - `ChildProcess` 的 JS 语义（事件顺序、`exec`/`execFile` 的回调、`maxBuffer` 累积）
//!   在 [`super::child_process`] 的包装层，不在 Rust 里。
//!
//! # 事件顺序
//!
//! `spawn` → `stdout`/`stderr`（0..n 块）→ `stdout-end`/`stderr-end` → `exit` → `close`。
//!
//! 与 Node 一致：`exit` 在进程退出时立刻发，`close` 要等 **stdio 全关**之后。
//! 两者之间的空档是真实存在的（孙进程可能还攥着管道），所以 `close` 之后才是
//! "可以认为它彻底结束了"。
//!
//! # 超时
//!
//! `timeoutMs` 由这里负责（而不是 JS 侧的 `setTimeout`）：JS 的定时器走
//! `__native_sleep_async`，会挂在未决 Promise 上，`wait_idle()` 就得陪着它等满整个超时时间——
//! 一个 30s 超时、1s 跑完的命令会让空闲判定白等 29s。放进 Tokio 的 `select!` 里就没这问题。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use super::child_process::{
    SpawnSpec, kill_process_tree, shell_program_and_args, signal_of, stdio_for,
};

/// 单次读取的块大小：与同步版 reader 取同一个值，免得同一份输出在两条路径上
/// 切出的 `data` 块差一个数量级（插件按块做解析时最容易踩）。
const READ_CHUNK: usize = 8 * 1024;
/// 进程退出后等 stdio EOF 的宽限。
///
/// 为什么必须有：管道可能被**孙进程**继承着，父进程已经死了也读不到 EOF。
/// 没有这个宽限，`close` 事件会永远不来（Node 同样会等，但它等的是 stream 自己
/// 关闭；我们这里是同一件事的超时版本）。
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// 回传给 JS 的一种子进程事件。
///
/// 输出块一律走 **base64**：`String::from_utf8_lossy` 会把无效字节换成 `�`，
/// 而那正是二进制输出（图片、gzip）里最常见的内容——丢一个字节，插件就解不开。
#[derive(Debug, Clone)]
pub enum ChildEvent {
    Spawn {
        pid: u32,
    },
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    StdoutEnd,
    StderrEnd,
    Exit {
        code: Option<i32>,
        signal: Option<String>,
        timed_out: bool,
    },
    Close {
        code: Option<i32>,
        signal: Option<String>,
        timed_out: bool,
    },
}

impl ChildEvent {
    /// 事件的 wire 形态（交给 JS 侧 `__child_process_dispatch` 消费）。
    pub fn to_json(&self) -> Value {
        match self {
            ChildEvent::Spawn { pid } => json!({ "type": "spawn", "pid": pid }),
            ChildEvent::Stdout(chunk) => json!({ "type": "stdout", "data": encode_base64(chunk) }),
            ChildEvent::Stderr(chunk) => json!({ "type": "stderr", "data": encode_base64(chunk) }),
            ChildEvent::StdoutEnd => json!({ "type": "stdout-end" }),
            ChildEvent::StderrEnd => json!({ "type": "stderr-end" }),
            ChildEvent::Exit {
                code,
                signal,
                timed_out,
            } => json!({
                "type": "exit",
                "code": code,
                "signal": signal,
                "timedOut": timed_out,
            }),
            ChildEvent::Close {
                code,
                signal,
                timed_out,
            } => json!({
                "type": "close",
                "code": code,
                "signal": signal,
                "timedOut": timed_out,
            }),
        }
    }
}

/// 事件出口：`(client_id, event)`。
///
/// `client_id` 由 **JS 侧**生成（不是这里的序号）：最早那批 `data` 可能先于
/// `spawn()` 的 Promise 到达，只有 JS 自己发的标识才能在那之前就建好路由表。
pub type EventSink = Arc<dyn Fn(&str, ChildEvent) + Send + Sync + 'static>;

/// 一个还活着的子进程的控制面（stdin 与 kill）。
struct ChildHandle {
    pid: u32,
    kill_tx: mpsc::UnboundedSender<String>,
    stdin: Arc<AsyncMutex<Option<ChildStdin>>>,
}

fn registry() -> &'static Mutex<HashMap<u64, ChildHandle>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, ChildHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_lock() -> MutexGuard<'static, HashMap<u64, ChildHandle>> {
    // 毒锁不致命：这里的临界区只有 HashMap 的增删查，没有"改到一半"的业务状态。
    registry().lock().unwrap_or_else(|poison| poison.into_inner())
}

/// 序号是**进程内全局**的（不是每个运行时一份）：注册表本身也是进程级的，
/// 两个运行时各自从 1 数起就会撞 key，撞了的结果是 A 的 `kill` 打到 B 的进程上。
static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);

/// 启动一个异步子进程，立即返回它的句柄 id。
///
/// 这个函数不做 `await`：`Command::spawn()` 本身就是非阻塞的，而后续的等待、读流、
/// 收尾都在 Tokio 任务里跑。于是 JS 侧拿到 id 的速度不受子进程寿命影响。
pub fn spawn(spec_json: &str, client_id: &str, sink: EventSink) -> Result<u64, String> {
    let raw: Value = serde_json::from_str(spec_json)
        .map_err(|error| format!("child_process 参数不是合法 JSON: {error}"))?;
    let spec = SpawnSpec::parse(&raw)?;
    let handle_rt = tokio::runtime::Handle::try_current()
        .map_err(|_| "异步 child_process 需要 Tokio 运行时（事件循环没挂上来）".to_string())?;

    let mut child = build_command(&spec)
        .spawn()
        .map_err(|error| format!("启动进程失败: {error}"))?;
    let pid = child.id().unwrap_or_default();
    let stdin = Arc::new(AsyncMutex::new(child.stdin.take()));
    let (kill_tx, kill_rx) = mpsc::unbounded_channel::<String>();
    let handle_id = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
    registry_lock().insert(
        handle_id,
        ChildHandle {
            pid,
            kill_tx,
            stdin,
        },
    );

    let client = client_id.to_string();
    sink(&client, ChildEvent::Spawn { pid });

    // 非 `pipe` 的流根本不会被读（`inherit` 直接接宿主、`ignore` 是空设备），
    // 但 JS 侧的 `stdout` 对象还在。立即给它一个 `end`，否则
    // `await new Promise(r => child.stdout.on('end', r))` 会永远等下去。
    if spec.stdout_mode != "pipe" {
        sink(&client, ChildEvent::StdoutEnd);
    }
    if spec.stderr_mode != "pipe" {
        sink(&client, ChildEvent::StderrEnd);
    }

    // stdio 各一个 reader；`pipes` 记住要等几个 EOF 才发 `close`。
    let (eof_tx, eof_rx) = mpsc::channel::<()>(4);
    let mut pipes = 0usize;
    if let Some(reader) = child.stdout.take() {
        pipes += 1;
        spawn_reader(
            reader,
            sink.clone(),
            client.clone(),
            StreamKind::Stdout,
            eof_tx.clone(),
        );
    }
    if let Some(reader) = child.stderr.take() {
        pipes += 1;
        spawn_reader(
            reader,
            sink.clone(),
            client.clone(),
            StreamKind::Stderr,
            eof_tx.clone(),
        );
    }
    drop(eof_tx);

    let deadline = (spec.timeout_ms > 0).then(|| {
        tokio::time::Instant::now() + Duration::from_millis(spec.timeout_ms)
    });
    let sink_for_task = sink.clone();
    handle_rt.spawn(async move {
        supervise(
            child,
            pid,
            handle_id,
            kill_rx,
            deadline,
            pipes,
            eof_rx,
            client,
            sink_for_task,
        )
        .await;
    });

    Ok(handle_id)
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    mut child: tokio::process::Child,
    pid: u32,
    handle_id: u64,
    mut kill_rx: mpsc::UnboundedReceiver<String>,
    mut deadline: Option<tokio::time::Instant>,
    pipes: usize,
    mut eof_rx: mpsc::Receiver<()>,
    client: String,
    sink: EventSink,
) {
    let mut timed_out = false;

    // 三个可能先发生的事：宿主 kill、到点、进程自己退出。
    //
    // `biased` 让"收到 kill"优先于"进程恰好退出"被检查——先杀树再收尸，
    // 否则容易漏掉树里的孙进程（父进程一退，孙进程就成了孤儿，再也找不回来）。
    let status = loop {
        tokio::select! {
            biased;
            Some(_signal) = kill_rx.recv() => {
                // 注意：这里**不能**调 `child.start_kill()`——`child.wait()` 的 future
                // 正抱着 `&mut child`，再加一处可变的借用会直接编译不过。
                // 而按 pid 杀整棵树本来就把子进程包含在内（unix 的 `kill -KILL -pgid`、
                // Windows 的 `taskkill /T`），所以不碰 `child` 也不会漏。
                kill_process_tree(pid);
            }
            _ = sleep_until(deadline), if deadline.is_some() => {
                timed_out = true;
                // 置空：绝对时刻已经过去，不清掉就会在下一轮 select 里立刻再次命中（空转）
                deadline = None;
                kill_process_tree(pid);
            }
            status = child.wait() => break status,
        }
    };

    let (code, signal) = match status {
        Ok(status) => (status.code(), signal_of(&status)),
        Err(_) => (None, None),
    };
    sink(
        &client,
        ChildEvent::Exit {
            code,
            signal: signal.clone(),
            timed_out,
        },
    );

    // 等 stdio 排空（宽限内）：真正的 `close` 必须晚于最后一块 `data`。
    for _ in 0..pipes {
        let _ = tokio::time::timeout(PIPE_DRAIN_GRACE, eof_rx.recv()).await;
    }

    sink(
        &client,
        ChildEvent::Close {
            code,
            signal,
            timed_out,
        },
    );
    // 收尸：句柄表里留着一个死进程的 pid 没有任何用处，还会让 `kill` 打到
    // 一个可能已被系统复用的 pid 上。
    registry_lock().remove(&handle_id);
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        // 没有超时 = 永不就绪（调用点用 `if deadline.is_some()` 兜住，这里只是补齐类型）
        None => std::future::pending().await,
    }
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

fn spawn_reader<R>(
    mut reader: R,
    sink: EventSink,
    client: String,
    kind: StreamKind,
    eof: mpsc::Sender<()>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buffer[..n].to_vec();
                    sink(
                        &client,
                        match kind {
                            StreamKind::Stdout => ChildEvent::Stdout(chunk),
                            StreamKind::Stderr => ChildEvent::Stderr(chunk),
                        },
                    );
                }
                Err(_) => break,
            }
        }
        sink(
            &client,
            match kind {
                StreamKind::Stdout => ChildEvent::StdoutEnd,
                StreamKind::Stderr => ChildEvent::StderrEnd,
            },
        );
        let _ = eof.send(()).await;
    });
}

fn build_command(spec: &SpawnSpec) -> Command {
    let mut command = if spec.shell {
        let (program, args) = shell_program_and_args(&spec.shell_name());
        let mut command = Command::new(program);
        command.args(args);
        command
    } else {
        let mut command = Command::new(&spec.file);
        command.args(&spec.args);
        command
    };
    command
        .current_dir(&spec.cwd)
        .stdin(stdio_for(&spec.stdin_mode))
        .stdout(stdio_for(&spec.stdout_mode))
        .stderr(stdio_for(&spec.stderr_mode))
        .env("A_DA_AGENT", "1");
    if let Some(env) = &spec.env {
        command.env_clear();
        for (key, value) in env {
            if let Some(value) = value.as_str() {
                command.env(key, value);
            }
        }
        command.env("A_DA_AGENT", "1");
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    command
}

fn stdin_of(handle_id: u64) -> Result<Arc<AsyncMutex<Option<ChildStdin>>>, String> {
    registry_lock()
        .get(&handle_id)
        .map(|handle| handle.stdin.clone())
        .ok_or_else(|| format!("子进程 {handle_id} 不存在或已退出"))
}

/// 往子进程 stdin 写一块数据（base64 传入，二进制安全）。
pub async fn write_stdin_base64(handle_id: u64, data_base64: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|error| format!("stdin 数据不是合法 base64: {error}"))?;
    let stdin = stdin_of(handle_id)?;
    let mut guard = stdin.lock().await;
    match guard.as_mut() {
        Some(handle) => {
            handle
                .write_all(&bytes)
                .await
                .map_err(|error| format!("写入子进程 stdin 失败: {error}"))?;
            handle
                .flush()
                .await
                .map_err(|error| format!("刷新子进程 stdin 失败: {error}"))
        }
        None => Err("子进程的 stdin 已关闭".to_string()),
    }
}

/// 关闭子进程 stdin（`child.stdin.end()`）：对端子进程会读到 EOF。
pub async fn close_stdin(handle_id: u64) -> Result<(), String> {
    let stdin = stdin_of(handle_id)?;
    // take 掉即 drop 掉即关闭写端——这里不需要也可能不该再写一个"EOF 标记字节"。
    stdin.lock().await.take();
    Ok(())
}

/// 给子进程发一个终止信号。
///
/// 真正的杀树动作在 supervisor 里做（那边才持有 `Child` 的 &mut，能 `start_kill` +
/// `wait` 收尸）。这里只负责把"要 kill"这件事传过去，因此是**同步且非阻塞**的——
/// JS 的 `kill()` 必须能立刻返回 `true`。
pub fn kill(handle_id: u64, signal: &str) -> bool {
    let sender = registry_lock()
        .get(&handle_id)
        .map(|handle| handle.kill_tx.clone());
    match sender {
        Some(sender) => sender.send(signal.to_string()).is_ok(),
        None => false,
    }
}

/// 当前还活着的子进程数。
///
/// 给 `PureTsRuntime::wait_children` 用：定时轮询这个数就是“还有没有子进程没收完”。
pub fn active_count() -> usize {
    registry_lock().len()
}

/// 运行时终止时把这台机器上所有还活着的子进程连根杀掉。
///
/// 不做这件事的后果很具体：插件跑一个 `npm install`，用户关掉会话 →
/// 宿主进程没了、子进程还在跑，占着端口与 CPU，而用户完全不知道是哪个插件干的。
pub fn kill_all() {
    let handles: Vec<(u32, mpsc::UnboundedSender<String>)> = registry_lock()
        .iter()
        .map(|(_, handle)| (handle.pid, handle.kill_tx.clone()))
        .collect();
    for (pid, sender) in handles {
        kill_process_tree(pid);
        let _ = sender.send("SIGKILL".to_string());
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn collector() -> (EventSink, Arc<StdMutex<Vec<(String, ChildEvent)>>>) {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let log_for_sink = log.clone();
        let sink: EventSink = Arc::new(move |client: &str, event: ChildEvent| {
            log_for_sink
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((client.to_string(), event));
        });
        (sink, log)
    }

    /// 事件形态是 JS 侧的契约：类型名与字段名改了，包装层就静默收不到东西。
    #[test]
    fn test_event_json_matches_the_js_contract() {
        assert_eq!(
            ChildEvent::Spawn { pid: 7 }.to_json(),
            json!({ "type": "spawn", "pid": 7 })
        );
        // 二进制输出必须无损：base64 而不是 lossy 字符串
        assert_eq!(
            ChildEvent::Stdout(vec![0xff, 0x00, 0x41]).to_json()["data"],
            json!("/wBB"),
            "无效 UTF-8 字节不能被替换成 `�`"
        );
        assert_eq!(ChildEvent::StdoutEnd.to_json()["type"], "stdout-end");
        assert_eq!(
            ChildEvent::Exit {
                code: Some(2),
                signal: None,
                timed_out: true
            }
            .to_json(),
            json!({ "type": "exit", "code": 2, "signal": null, "timedOut": true })
        );
    }

    /// 空 spec 必须给一条人话错误，而不是 panic 或者启动一个空命令。
    #[test]
    fn test_spawn_rejects_an_empty_file() {
        let (sink, _log) = collector();
        let err = spawn(r#"{"file": ""}"#, "c1", sink).expect_err("空 file 必须被拒");
        assert!(err.contains("file 不能为空"), "{err}");
    }

    #[test]
    fn test_kill_unknown_handle_is_false_not_a_panic() {
        assert!(!kill(u64::MAX, "SIGTERM"));
    }

    /// 静态检查：`kill_all` 在没有活跃子进程时是安全的。
    #[test]
    fn test_kill_all_without_children_is_a_noop() {
        kill_all();
    }

    /// 端到端：真进程 + 真流式输出 + 真事件顺序。
    ///
    /// 这条钉的是三件事：`stdout` 的字节真的到了（不丢不换）、`exit` 带着退出码、
    /// `close` 一定排在最后。顺序错了，JS 侧的 `exec` 回调就会在输出收齐之前触发。
    #[tokio::test]
    async fn test_spawn_streams_output_and_reports_exit_then_close() {
        if cfg!(windows) {
            // 用例依赖 POSIX 的 `sh -c printf`；不为 Windows 再造一份（宁缺勿假绿）。
            return;
        }
        let (sink, log) = collector();
        let id = spawn(
            r#"{"file": "/bin/sh", "args": ["-c", "printf hello; printf oops >&2; exit 3"]}"#,
            "c1",
            sink,
        )
        .expect("spawn 应当成功");
        assert!(id > 0);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let seen_close = log
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .iter()
                .any(|(_, event)| matches!(event, ChildEvent::Close { .. }));
            if seen_close {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "等 close 超时：{:#?}",
                log.lock().unwrap_or_else(|poison| poison.into_inner())
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let events = log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert!(
            matches!(events.first(), Some((_, ChildEvent::Spawn { .. }))),
            "第一个事件必须是 spawn：{events:#?}"
        );
        let stdout: Vec<u8> = events
            .iter()
            .filter_map(|(_, event)| match event {
                ChildEvent::Stdout(chunk) => Some(chunk.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(String::from_utf8_lossy(&stdout), "hello");
        let stderr: Vec<u8> = events
            .iter()
            .filter_map(|(_, event)| match event {
                ChildEvent::Stderr(chunk) => Some(chunk.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(String::from_utf8_lossy(&stderr), "oops");
        assert!(
            events
                .iter()
                .any(|(_, event)| matches!(event, ChildEvent::Exit { code: Some(3), .. })),
            "exit 必须带上真实退出码：{events:#?}"
        );
        assert!(
            matches!(
                events.last(),
                Some((
                    _,
                    ChildEvent::Close {
                        code: Some(3),
                        timed_out: false,
                        ..
                    }
                ))
            ),
            "close 必须最后到、且与 exit 同码：{events:#?}"
        );
    }

    /// 超时由宿主负责：到点要真的杀掉，并把 `timedOut` 带回去（JS 侧靠它区分
    /// “命令失败”与“我们杀的”）。
    #[tokio::test]
    async fn test_timeout_kills_the_child_and_is_reported() {
        if cfg!(windows) {
            return;
        }
        let (sink, log) = collector();
        spawn(
            r#"{"file": "/bin/sh", "args": ["-c", "sleep 30"], "timeoutMs": 150}"#,
            "c2",
            sink,
        )
        .expect("spawn 应当成功");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let closed = log
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .iter()
                .any(|(_, event)| matches!(event, ChildEvent::Close { .. }));
            if closed {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "超时的进程必须被杀掉，不能等到它自己睡醒"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let events = log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert!(
            events.iter().any(|(_, event)| matches!(
                event,
                ChildEvent::Close {
                    timed_out: true,
                    ..
                }
            )),
            "close 必须带着 timedOut：{events:#?}"
        );
    }
}
