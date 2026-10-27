//! `child_process` 的同步子集（`execFileSync` / `spawnSync` / `execSync`）与
//! 异步子集的 **JS 外壳**（`spawn` / `exec` / `execFile`）。
//!
//! 这不是 Node 的完整实现，而是一条受控的宿主能力：
//! - 只有 `@ada-needs exec` 才注入模块（声明面，不是运行期拦截）：
//!   JS 侧既没有 `globalThis.childProcess`，也没有 `__native_child_process_*_async` 原生桥；
//! - `cwd` 默认是工作区，`cwd` 选项可以指向任意目录（先保证功能，安全边界后议）；
//! - 超时 / 输出超过 `maxBuffer` 时杀**整棵进程树**，并返回结构化错误；
//! - **异步在跑**：`spawn` 返回真正流式的 `ChildProcess`（`stdout`/`stderr` 逐块 `data`，
//!   stdin 可写，`kill` 杀进程树）。执行器在 [`super::child_process_async`]，
//!   事件经 `EventLoopMsg::ChildEvent` 回到 JS 事件循环；
//! - `fork` 明确报不支持（需要 IPC channel，不伪装成可用）。
//!
//! ⚠️ `exec` 等价于任意代码执行。当前阶段优先功能实现；插件信任边界由安装者
//! 负责，后续会再补审批/策略。

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use boa_engine::{js_error, js_string, Context, JsValue, NativeFunction, Source};
use serde_json::{json, Value};

const DEFAULT_MAX_BUFFER: usize = 1024 * 1024;
const READER_GRACE: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

thread_local! {
    static CHILD_WORKSPACE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// 注册 `child_process` 原生桥与 JS 封装。
pub fn register_child_process_native_and_script(
    ctx: &mut Context,
    workspace: Option<&Path>,
) -> Result<(), String> {
    CHILD_WORKSPACE.with(|cell| {
        *cell.borrow_mut() = workspace.map(Path::to_path_buf);
    });

    let native_run = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let spec = arg_string(args, 0, ctx);
        let result = run_child(&spec).map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::from(js_string!(result.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_child_process_run"), 1, native_run)
        .ok();

    let wrapper = r#"
        (function() {
            function decode(value, encoding) {
                if (value === null || value === undefined) return null;
                if (encoding === 'buffer') {
                    return Buffer.from(value.__ada_buffer || '', 'base64');
                }
                return String(value);
            }

            function encodingOf(options) {
                const raw = options && options.encoding;
                if (raw === null || raw === 'buffer') return 'buffer';
                if (raw === undefined) return 'utf8';
                const text = String(raw).toLowerCase();
                if (text === 'utf8' || text === 'utf-8') return 'utf8';
                throw new Error('child_process 只支持 utf8 或 buffer 编码（收到 ' + raw + '）');
            }

            function stdioMode(stdio, index, fallback) {
                if (stdio === undefined) return fallback;
                if (stdio === 'ignore' || stdio === 'null') return 'ignore';
                if (stdio === 'pipe') return 'pipe';
                if (stdio === 'inherit') return 'inherit';
                if (Array.isArray(stdio)) {
                    const value = stdio[index];
                    if (value === undefined) return fallback;
                    if (value === 'ignore' || value === 'null') return 'ignore';
                    if (value === 'pipe') return 'pipe';
                    if (value === 'inherit') return 'inherit';
                }
                throw new Error('child_process 的 stdio 只支持 pipe / ignore / inherit');
            }

            function encodeInput(input) {
                if (input === undefined) return null;
                if (typeof input === 'string') return { kind: 'text', data: input };
                if (typeof Buffer !== 'undefined' &&
                    (Buffer.isBuffer(input) || input instanceof Uint8Array)) {
                    return { kind: 'buffer', data: Buffer.from(input).toString('base64') };
                }
                return { kind: 'text', data: String(input) };
            }

            function makeSpec(file, args, options, shell) {
                options = options || {};
                if (!Array.isArray(args)) {
                    throw new TypeError('child_process 的 args 必须是数组');
                }
                const encoding = encodingOf(options);
                const env = {};
                if (options.env && typeof options.env === 'object') {
                    const keys = Object.keys(options.env);
                    for (let i = 0; i < keys.length; i++) {
                        const value = options.env[keys[i]];
                        if (value !== undefined && value !== null) {
                            env[keys[i]] = String(value);
                        }
                    }
                }
                const input = encodeInput(options.input);
                let stdinMode = stdioMode(options.stdio, 0, 'pipe');
                if (input !== null) stdinMode = 'pipe';
                return JSON.stringify({
                    file: String(file),
                    args: args.map(function(value) { return String(value); }),
                    cwd: options.cwd === undefined || options.cwd === null ? null : String(options.cwd),
                    env: options.env && typeof options.env === 'object' ? env : null,
                    timeoutMs: typeof options.timeout === 'number' ? Math.max(0, options.timeout) : 0,
                    maxBuffer: typeof options.maxBuffer === 'number' ? Math.max(0, options.maxBuffer) : 1048576,
                    encoding: encoding,
                    shell: shell === true || options.shell === true,
                    shellWhole: shell === true,
                    stdinMode: stdinMode,
                    stdoutMode: stdioMode(options.stdio, 1, 'pipe'),
                    stderrMode: stdioMode(options.stdio, 2, 'pipe'),
                    input: input,
                });
            }

            function run(file, args, options, shell) {
                return JSON.parse(__native_child_process_run(makeSpec(file, args, options, shell)));
            }

            function makeError(result, encoding) {
                let message = result.spawnError || 'child process failed';
                let code;
                if (result.spawnError) {
                    code = 'ENOENT';
                } else if (result.timedOut) {
                    message = 'child process timed out';
                    code = 'ETIMEDOUT';
                } else if (result.overflow) {
                    message = 'child process maxBuffer exceeded';
                    code = 'ENOBUFS';
                }
                const error = new Error(message);
                if (code) error.code = code;
                error.pid = result.pid === undefined ? undefined : result.pid;
                error.status = result.status === undefined ? null : result.status;
                error.signal = result.signal === undefined ? null : result.signal;
                error.stdout = decode(result.stdout, encoding);
                error.stderr = decode(result.stderr, encoding);
                return error;
            }

            function spawnSync(file, args, options) {
                const encoding = encodingOf(options);
                const result = run(file, args || [], options, false);
                const stdout = decode(result.stdout, encoding);
                const stderr = decode(result.stderr, encoding);
                const out = {
                    pid: result.pid,
                    output: [null, stdout, stderr],
                    stdout: stdout,
                    stderr: stderr,
                    status: result.status === undefined ? null : result.status,
                    signal: result.signal === undefined ? null : result.signal,
                };
                if (result.spawnError || result.timedOut || result.overflow) {
                    out.error = makeError(result, encoding);
                }
                return out;
            }

            function execFileSync(file, args, options) {
                const encoding = encodingOf(options);
                const result = run(file, args || [], options, false);
                if (result.spawnError || result.timedOut || result.overflow || result.status !== 0) {
                    throw makeError(result, encoding);
                }
                return decode(result.stdout, encoding);
            }

            function execSync(command, options) {
                const encoding = encodingOf(options);
                const result = run(command, [], options, true);
                if (result.spawnError || result.timedOut || result.overflow || result.status !== 0) {
                    throw makeError(result, encoding);
                }
                return decode(result.stdout, encoding);
            }

            // ── 异步子集 ─────────────────────────────────────────────────────────
            //
            // 与同步版的根本差别：这里**不阻塞** JS。进程还在跑的时候，输出就已经
            // 一块一块地推过来了（`stdout.on('data', ...)`），这才是异步真正的价值。
            //
            // 事件回流的顺序：spawn → stdout/stderr(data…) → *-end → exit → close。
            // 路由用 `clientId`（本文件生成）而不是宿主句柄号：最早那批 data 可能
            // **先于** spawn 的 Promise 到达，那时句柄号还没有影子。
            const children = Object.create(null);
            let nextChildSeq = 0;
            const ASYNC_HINT =
                '异步 child_process 未注入：它需要 // @ada-needs exec（且事件循环要挂 Tokio）';

            function nativeChild(name) {
                const fn = globalThis[name];
                if (typeof fn !== 'function') throw new Error(ASYNC_HINT);
                return fn;
            }

            function chunkToBase64(chunk) {
                if (typeof chunk === 'string') {
                    return Buffer.from(chunk, 'utf8').toString('base64');
                }
                if (typeof chunk === 'number') {
                    return Buffer.from(String(chunk), 'utf8').toString('base64');
                }
                if (typeof Buffer !== 'undefined' && Buffer.isBuffer && Buffer.isBuffer(chunk)) {
                    return chunk.toString('base64');
                }
                return Buffer.from(chunk).toString('base64');
            }

            // Node 的 stdout/stderr 是 Readable。这里实现插件真正会用到的三个动作：
            // on('data')、on('end')、setEncoding()。pipe() 明确抛错——半实现的 pipe
            // 比没有更糟（它会让 `a.pipe(b)` 看起来成功、数据却哪儿也没去）。
            class ChildStream extends EventEmitter {
                constructor() {
                    super();
                    this._encoding = null;
                    this._closed = false;
                    this.readable = true;
                    this.readableEnded = false;
                }

                setEncoding(encoding) {
                    this._encoding = String(encoding || 'utf8').toLowerCase();
                    return this;
                }

                _onChunk(base64) {
                    if (this._closed) return;
                    const buffer = Buffer.from(base64, 'base64');
                    this.emit('data', this._encoding === null ? buffer : buffer.toString(this._encoding));
                }

                _onEnd() {
                    if (this._closed) return;
                    this._closed = true;
                    this.readable = false;
                    this.readableEnded = true;
                    this.emit('end');
                }

                destroy() { this._onEnd(); }
                pause() { return this; }
                resume() { return this; }
                pipe() {
                    throw new Error('child_process 的流暂不支持 pipe()；请用 on("data") 自行收集');
                }
            }

            class ChildProcess extends EventEmitter {
                constructor(spawnfile, spawnargs) {
                    super();
                    this.pid = undefined;
                    this.spawnfile = spawnfile;
                    this.spawnargs = spawnargs;
                    this.exitCode = null;
                    this.signalCode = null;
                    this.killed = false;
                    this.spawnError = null;
                    this.stdout = new ChildStream();
                    this.stderr = new ChildStream();
                    this.stdin = null;
                    this._clientId = 'cp' + (++nextChildSeq);
                    this._handleId = null;
                    this._pendingKill = null;
                    this._spawnEmitted = false;
                    this._timedOut = false;
                    this._stdinPump = null;
                }

                // 没人接的错误不能静默甩掉（Node 在这里会直接抛），也不能把事件循环炸掉：
                // 记到 `spawnError` 上 + 打一条 console.error，插件日志里看得见。
                emit(event) {
                    if (event === 'error' && this.listenerCount('error') === 0) {
                        this.spawnError = arguments[1] || new Error(String(arguments[0]));
                        if (typeof console !== 'undefined' && console.error) {
                            console.error('[child_process] ' + (this.spawnError.message || String(this.spawnError)));
                        }
                        return false;
                    }
                    return EventEmitter.prototype.emit.apply(this, arguments);
                }

                _emitSpawnOnce() {
                    if (this._spawnEmitted) return;
                    // 'spawn' 的语义是"进程真的起来了"：句柄号必须已经可用，
                    // 否则监听者在这个回调里就 kill 不了它。
                    if (this._handleId === null) return;
                    this._spawnEmitted = true;
                    this.emit('spawn');
                }

                kill(signal) {
                    const target = signal === undefined ? 'SIGTERM' : String(signal);
                    this.killed = true;
                    if (this._handleId === null) {
                        // spawn 的回执还没到：记下来，等它对上再补发（否则这次 kill 会凭空消失）
                        this._pendingKill = target;
                        return true;
                    }
                    try {
                        nativeChild('__native_child_process_kill_async')(String(this._handleId), target)
                            .catch(function () {});
                    } catch (_) {
                        return false;
                    }
                    return true;
                }

                ref() { return this; }
                unref() { return this; }
            }

            // stdin：连着宿主侧的一条写通道。
            //
            // 队列 + 单飞（一次只允许一个 write 在途）是必要的：`write()` 在 JS 里是
            // 同步返回 true 的，而底层是异步写；不排队的话，连续两次 write 的落地顺序
            // 就没有保证，粘包式的协议会直接错位。
            function attachStdin(child) {
                const stream = new EventEmitter();
                stream.writable = true;
                stream.writableEnded = false;

                const queue = [];
                let busy = false;

                function pump() {
                    if (busy || queue.length === 0) return;
                    if (child._handleId === null) return; // 等 spawn 回执后再由它唤醒
                    const item = queue[0];
                    busy = true;
                    const id = String(child._handleId);
                    const call = item.kind === 'end'
                        ? nativeChild('__native_child_process_end_stdin_async')(id)
                        : nativeChild('__native_child_process_write_stdin_async')(id, item.data);
                    call.then(function () {
                        queue.shift();
                        busy = false;
                        if (typeof item.cb === 'function') item.cb();
                        pump();
                    }).catch(function (err) {
                        queue.shift();
                        busy = false;
                        if (typeof item.cb === 'function') item.cb(err);
                        else stream.emit('error', err);
                        pump();
                    });
                }

                child._stdinPump = pump;

                stream.write = function (chunk, cb) {
                    if (stream.writableEnded) {
                        const err = new Error('write after end');
                        if (typeof cb === 'function') { cb(err); return false; }
                        throw err;
                    }
                    queue.push({ kind: 'write', data: chunkToBase64(chunk), cb: cb });
                    pump();
                    return true;
                };

                stream.end = function (chunk, cb) {
                    if (chunk !== undefined && chunk !== null && typeof chunk !== 'function') {
                        stream.write(chunk);
                    }
                    const done = typeof chunk === 'function' ? chunk : cb;
                    stream.writableEnded = true;
                    stream.writable = false;
                    queue.push({ kind: 'end', data: null, cb: done });
                    pump();
                    return stream;
                };

                stream.destroy = function () {
                    stream.writableEnded = true;
                    stream.writable = false;
                };
                return stream;
            }

            function spawnChild(file, args, options, shellWhole) {
                const opts = options || {};
                const argv = Array.isArray(args) ? args : [];
                const child = new ChildProcess(String(file), argv.map(function (v) { return String(v); }));
                // 与 Node 一致：只有 `stdio[0] === 'pipe'` 才有可写的 `stdin`，
                // 其他档位下它是 `null`（而不是一个永远写不进去的对象）。
                child.stdin = stdioMode(opts.stdio, 0, 'pipe') === 'pipe'
                    ? attachStdin(child)
                    : null;
                children[child._clientId] = child;

                let pending;
                try {
                    pending = nativeChild('__native_child_process_spawn_async')(
                        makeSpec(file, argv, opts, shellWhole),
                        child._clientId
                    );
                } catch (err) {
                    delete children[child._clientId];
                    throw err;
                }

                pending.then(function (raw) {
                    child._handleId = Number(raw);
                    if (child._pendingKill !== null) {
                        const signal = child._pendingKill;
                        child._pendingKill = null;
                        child.kill(signal);
                    }
                    if (typeof child._stdinPump === 'function') child._stdinPump();
                    child._emitSpawnOnce();
                }).catch(function (raw) {
                    delete children[child._clientId];
                    const error = new Error(String(raw));
                    error.code = 'ENOENT';
                    error.spawnfile = child.spawnfile;
                    child.emit('error', error);
                });

                return child;
            }

            function spawn(file, args, options) {
                // Node 允许 spawn(file, options)：args 那一格直接给选项对象
                if (!Array.isArray(args) && args && typeof args === 'object') {
                    options = args;
                    args = [];
                }
                return spawnChild(file, args || [], options, false);
            }

            globalThis.__child_process_dispatch = function (clientId, payload) {
                const child = children[clientId];
                if (!child || !payload) return;
                switch (payload.type) {
                    case 'spawn':
                        child.pid = payload.pid;
                        child._emitSpawnOnce();
                        break;
                    case 'stdout':
                        child.stdout._onChunk(payload.data);
                        break;
                    case 'stderr':
                        child.stderr._onChunk(payload.data);
                        break;
                    case 'stdout-end':
                        child.stdout._onEnd();
                        break;
                    case 'stderr-end':
                        child.stderr._onEnd();
                        break;
                    case 'exit':
                        child.exitCode = payload.code === undefined ? null : payload.code;
                        child.signalCode = payload.signal === undefined ? null : payload.signal;
                        child._timedOut = !!payload.timedOut;
                        child.emit('exit', child.exitCode, child.signalCode);
                        break;
                    case 'close':
                        child.exitCode = payload.code === undefined ? null : payload.code;
                        child.signalCode = payload.signal === undefined ? null : payload.signal;
                        child._timedOut = child._timedOut || !!payload.timedOut;
                        child.emit('close', child.exitCode, child.signalCode);
                        delete children[clientId];
                        break;
                    default:
                        break;
                }
            };

            function joinChunks(chunks, encoding) {
                let total = 0;
                for (let i = 0; i < chunks.length; i++) total += chunks[i].length;
                const merged = Buffer.alloc(total);
                let offset = 0;
                for (let i = 0; i < chunks.length; i++) {
                    merged.set(chunks[i], offset);
                    offset += chunks[i].length;
                }
                return encoding === 'buffer' ? merged : merged.toString('utf8');
            }

            // exec / execFile：把流攒起来，在 close 时按 Node 的签名回调。
            //   callback(error, stdout, stderr)
            function collectOutput(child, options, encoding, callback) {
                const maxBuffer = typeof options.maxBuffer === 'number'
                    ? Math.max(0, options.maxBuffer)
                    : 1048576;
                const chunksOut = [];
                const chunksErr = [];
                let size = 0;
                let overflowed = false;
                let finished = false;

                function drain(chunks, chunk) {
                    if (overflowed) return;
                    size += chunk.length;
                    if (size > maxBuffer) {
                        overflowed = true;
                        // 超限就立刻杀：结果是注定的错误，继续攒只是把内存推向 OOM
                        child.kill('SIGKILL');
                        return;
                    }
                    chunks.push(chunk);
                }

                child.stdout.on('data', function (chunk) { drain(chunksOut, chunk); });
                child.stderr.on('data', function (chunk) { drain(chunksErr, chunk); });

                function finish(code, signal) {
                    if (finished) return;
                    finished = true;
                    if (typeof callback !== 'function') return;
                    const stdout = joinChunks(chunksOut, encoding);
                    const stderr = joinChunks(chunksErr, encoding);
                    let error = null;
                    if (overflowed) {
                        error = new Error('maxBuffer exceeded');
                        error.code = 'ENOBUFS';
                    } else if (child._timedOut) {
                        error = new Error('child process timed out');
                        error.code = 'ETIMEDOUT';
                    } else if (code !== 0 || signal !== null) {
                        error = new Error('Command failed: ' + child.spawnfile);
                        error.code = code === null ? null : code;
                    }
                    if (error) {
                        error.pid = child.pid;
                        error.killed = child.killed;
                        error.signal = signal === undefined ? null : signal;
                        error.stdout = stdout;
                        error.stderr = stderr;
                    }
                    callback(error, stdout, stderr);
                }

                child.on('close', finish);
                child.on('error', function (err) {
                    if (finished) return;
                    finished = true;
                    if (typeof callback === 'function') {
                        callback(err, joinChunks(chunksOut, encoding), joinChunks(chunksErr, encoding));
                    }
                });
            }

            function exec(command, options, callback) {
                if (typeof options === 'function') {
                    callback = options;
                    options = {};
                }
                const opts = options || {};
                const encoding = encodingOf(opts);
                // exec 的语义本来就是"过一遍 shell"，所以整串当命令交给 shell
                const child = spawnChild(String(command), [], opts, true);
                collectOutput(child, opts, encoding, callback);
                return child;
            }

            function execFile(file, args, options, callback) {
                if (typeof args === 'function') {
                    callback = args;
                    args = [];
                    options = {};
                } else if (!Array.isArray(args) && args && typeof args === 'object') {
                    callback = typeof options === 'function' ? options : callback;
                    options = args;
                    args = [];
                }
                if (typeof options === 'function') {
                    callback = options;
                    options = {};
                }
                const opts = options || {};
                const encoding = encodingOf(opts);
                const child = spawnChild(file, args || [], opts, false);
                collectOutput(child, opts, encoding, callback);
                return child;
            }

            function unsupported(name, reason) {
                return function () {
                    throw new Error('TS 微内核暂不支持 child_process.' + name + '；' + reason);
                };
            }

            globalThis.childProcess = {
                spawnSync: spawnSync,
                execFileSync: execFileSync,
                execSync: execSync,
                spawn: spawn,
                exec: exec,
                execFile: execFile,
                // fork 需要 IPC channel（父子进程间的结构化消息通道），这是另一条能力，
                // 不拿"能起进程"凑数——凑出来的 fork 只会让人以为消息能通。
                fork: unsupported(
                    'fork',
                    '它需要 IPC channel；请用 spawn 起独立进程，用 stdin/stdout 通信。'
                ),
            };
        })();
    "#;
    ctx.eval(Source::from_bytes(wrapper))
        .map_err(|e| format!("注入 child_process 封装层失败: {e}"))?;
    Ok(())
}

fn arg_string(args: &[JsValue], index: usize, ctx: &mut Context) -> String {
    args.get(index)
        .map(|value| {
            value
                .to_string(ctx)
                .unwrap_or_default()
                .to_std_string_escaped()
        })
        .unwrap_or_default()
}

fn workspace() -> PathBuf {
    CHILD_WORKSPACE.with(|cell| {
        cell.borrow()
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    })
}

/// 一次子进程调用的完整参数。
///
/// 🔴 同步与异步**共用同一份解析**：两条口径各写一遍，迟早会在 `shell`、`cwd`、
/// `stdio` 这些细节上分叉，而这类分叉只会在用户的脚本里以"同步能跑、异步跑不了"
/// 的形式出现——排查成本远高于这里的几行抽象。
pub(crate) struct SpawnSpec {
    pub file: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Option<serde_json::Map<String, Value>>,
    pub shell: bool,
    pub shell_whole: bool,
    pub stdin_mode: String,
    pub stdout_mode: String,
    pub stderr_mode: String,
    pub timeout_ms: u64,
    pub max_buffer: usize,
    /// `utf8` 或 `buffer`（同步按它决定返回串还是带标记的二进制）。
    pub encoding: String,
    /// 同步 API 的 `input` 选项（异步走 stdin 流，不走这里）。
    pub input: Option<Vec<u8>>,
}

impl SpawnSpec {
    pub(crate) fn parse(spec: &Value) -> Result<Self, String> {
        let file = spec
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "child_process 的 file 不能为空".to_string())?
            .to_string();
        if file.contains('\0') {
            return Err("child_process 的 file 含 NUL".to_string());
        }
        let args: Vec<String> = spec
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| match item {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if args.iter().any(|arg| arg.contains('\0')) {
            return Err("child_process 的 argv 含 NUL".to_string());
        }

        let workspace = workspace();
        let cwd = match spec.get("cwd").and_then(Value::as_str).map(str::trim) {
            Some(raw) if !raw.is_empty() => {
                let path = Path::new(raw);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    workspace.join(path)
                }
            }
            _ => workspace,
        };
        let max_buffer = match spec.get("maxBuffer").and_then(Value::as_u64) {
            // 0 = 不设上限（功能优先；调用方自己负责内存）。
            Some(0) => usize::MAX,
            Some(value) => value.min(usize::MAX as u64) as usize,
            None => DEFAULT_MAX_BUFFER,
        };
        Ok(Self {
            file,
            args,
            cwd,
            env: spec.get("env").and_then(Value::as_object).cloned(),
            shell: spec.get("shell").and_then(Value::as_bool).unwrap_or(false),
            shell_whole: spec
                .get("shellWhole")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            stdin_mode: spec
                .get("stdinMode")
                .and_then(Value::as_str)
                .unwrap_or("pipe")
                .to_string(),
            stdout_mode: spec
                .get("stdoutMode")
                .and_then(Value::as_str)
                .unwrap_or("pipe")
                .to_string(),
            stderr_mode: spec
                .get("stderrMode")
                .and_then(Value::as_str)
                .unwrap_or("pipe")
                .to_string(),
            timeout_ms: spec.get("timeoutMs").and_then(Value::as_u64).unwrap_or(0),
            max_buffer,
            encoding: match spec.get("encoding").and_then(Value::as_str) {
                Some("buffer") => "buffer".to_string(),
                _ => "utf8".to_string(),
            },
            input: decode_input(spec.get("input")),
        })
    }

    /// `shell: true` 时要执行的命令行（`shellWhole` 表示整串本来就是命令）。
    pub(crate) fn shell_name(&self) -> String {
        if self.shell_whole {
            self.file.clone()
        } else {
            shell_line(&self.file, &self.args)
        }
    }

    fn common_env(&self, command: &mut Command) {
        if let Some(env) = &self.env {
            command.env_clear();
            for (key, value) in env {
                if let Some(value) = value.as_str() {
                    command.env(key, value);
                }
            }
        }
        command.env("A_DA_AGENT", "1");
    }

    pub(crate) fn build_std_command(&self) -> Command {
        let mut command = if self.shell {
            shell_command(&self.shell_name())
        } else {
            let mut command = Command::new(&self.file);
            command.args(&self.args);
            command
        };
        command
            .current_dir(&self.cwd)
            .stdin(stdio_for(&self.stdin_mode))
            .stdout(stdio_for(&self.stdout_mode))
            .stderr(stdio_for(&self.stderr_mode));
        self.common_env(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command
    }
}

fn run_child(spec_json: &str) -> Result<String, String> {
    let raw: Value = serde_json::from_str(spec_json)
        .map_err(|error| format!("child_process 参数不是合法 JSON: {error}"))?;
    let mut spec = SpawnSpec::parse(&raw)?;
    let mut command = spec.build_std_command();
    let timeout_ms = spec.timeout_ms;
    let max_buffer = spec.max_buffer;
    let encoding = spec.encoding.as_str();
    let stdout_mode = spec.stdout_mode.as_str();
    let stderr_mode = spec.stderr_mode.as_str();
    let input = spec.input.take();

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(json!({
                "pid": null,
                "status": null,
                "signal": null,
                "stdout": null,
                "stderr": null,
                "timedOut": false,
                "overflow": false,
                "spawnError": format!("启动进程失败: {error}"),
            })
            .to_string())
        }
    };
    let pid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        match input {
            Some(bytes) => {
                thread::spawn(move || {
                    let _ = stdin.write_all(&bytes);
                });
            }
            None => drop(stdin),
        }
    }
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_capture = child
        .stdout
        .take()
        .map(|reader| spawn_reader(reader, max_buffer, overflow.clone()));
    let stderr_capture = child
        .stderr
        .take()
        .map(|reader| spawn_reader(reader, max_buffer, overflow.clone()));

    let deadline = if timeout_ms > 0 {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    } else {
        None
    };
    let mut timed_out = false;
    let mut overflowed = false;
    let status = loop {
        if overflow.load(Ordering::Relaxed) {
            overflowed = true;
            kill_process_tree(pid);
            let _ = child.kill();
            break child.wait().ok();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                timed_out = true;
                kill_process_tree(pid);
                let _ = child.kill();
                break child.wait().ok();
            }
        }
        thread::sleep(POLL_INTERVAL);
    };

    let stdout = finish_capture(stdout_capture);
    let stderr = finish_capture(stderr_capture);
    let (status_code, signal) = if timed_out || overflowed {
        (None, Some("SIGKILL".to_string()))
    } else {
        match status {
            Some(status) => (status.code(), signal_of(&status)),
            None => (None, None),
        }
    };
    let stdout = if stdout_mode == "pipe" {
        Some(encode_output(&stdout, encoding))
    } else {
        None
    };
    let stderr = if stderr_mode == "pipe" {
        Some(encode_output(&stderr, encoding))
    } else {
        None
    };
    Ok(json!({
        "pid": pid,
        "status": status_code,
        "signal": signal,
        "stdout": stdout,
        "stderr": stderr,
        "timedOut": timed_out,
        "overflow": overflowed,
        "spawnError": null,
    })
    .to_string())
}

pub(crate) fn stdio_for(mode: &str) -> Stdio {
    match mode {
        "inherit" => Stdio::inherit(),
        "ignore" | "null" => Stdio::null(),
        _ => Stdio::piped(),
    }
}

pub(crate) fn shell_line(file: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_arg(file));
    for arg in args {
        parts.push(quote_arg(arg));
    }
    parts.join(" ")
}

fn quote_arg(raw: &str) -> String {
    if raw.is_empty() {
        return "\"\"".to_string();
    }
    #[cfg(windows)]
    {
        // cmd.exe 的转义规则比 POSIX 复杂；这里只处理最常见的空格/双引号。
        if raw
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '&' | '|' | '<' | '>' | '^'))
        {
            let escaped = raw.replace('"', "\\\"");
            return format!("\"{escaped}\"");
        }
        raw.to_string()
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", raw.replace('\'', "'\\''"))
    }
}

fn decode_input(input: Option<&Value>) -> Option<Vec<u8>> {
    let input = input?;
    let data = input.get("data").and_then(Value::as_str)?;
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("text");
    if kind == "buffer" {
        base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .ok()
    } else {
        Some(data.as_bytes().to_vec())
    }
}

/// 该用哪个 shell、以及要传给它的固定参数（同步与异步共用，别各写一份）。
pub(crate) fn shell_program_and_args(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        (
            comspec,
            vec![
                "/d".to_string(),
                "/s".to_string(),
                "/c".to_string(),
                command.to_string(),
            ],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

fn shell_command(command: &str) -> Command {
    let (program, args) = shell_program_and_args(command);
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd
}

struct Capture {
    buffer: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

fn spawn_reader<R>(mut reader: R, max_buffer: usize, overflow: Arc<AtomicBool>) -> Capture
where
    R: Read + Send + 'static,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let buffer_for_thread = buffer.clone();
    let done_for_thread = done.clone();
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut total = 0usize;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total > max_buffer {
                        overflow.store(true, Ordering::Relaxed);
                    }
                    let mut guard = buffer_for_thread
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    let remaining = max_buffer.saturating_sub(guard.len());
                    if remaining > 0 {
                        guard.extend_from_slice(&chunk[..n.min(remaining)]);
                    }
                }
                Err(_) => break,
            }
        }
        done_for_thread.store(true, Ordering::Relaxed);
    });
    Capture { buffer, done }
}

fn finish_capture(capture: Option<Capture>) -> Vec<u8> {
    let Some(capture) = capture else {
        return Vec::new();
    };
    let deadline = Instant::now() + READER_GRACE;
    while !capture.done.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let guard = capture
        .buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.clone()
}

fn encode_output(bytes: &[u8], encoding: &str) -> Value {
    if encoding == "buffer" {
        json!({
            "__ada_buffer": base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    } else {
        Value::String(String::from_utf8_lossy(bytes).to_string())
    }
}

pub(crate) fn signal_of(status: &ExitStatus) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|number| match number {
            2 => "SIGINT".to_string(),
            9 => "SIGKILL".to_string(),
            15 => "SIGTERM".to_string(),
            other => format!("SIG{other}"),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_output_uses_a_tagged_buffer_for_binary_mode() {
        assert_eq!(
            encode_output(b"hi", "utf8"),
            Value::String("hi".to_string())
        );
        let buffered = encode_output(b"hi", "buffer");
        assert_eq!(buffered["__ada_buffer"], "aGk=");
    }

    #[test]
    fn kill_process_tree_is_safe_for_an_unused_pid() {
        // 不真的断言 taskkill 成功：这里只守"没有 panic"。
        kill_process_tree(u32::MAX);
    }
}
