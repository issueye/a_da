//! `child_process` 的**同步子集**：`execFileSync` / `spawnSync` / `execSync`。
//!
//! 这不是 Node 的完整实现，而是一条受控的宿主能力：
//! - 只有 `@ada-needs exec` 才注入模块（声明面，不是运行期拦截）；
//! - `cwd` 默认是工作区，`cwd` 选项可以指向任意目录（先保证功能，安全边界后议）；
//! - stdin 默认 `Stdio::null()`，可用 `input` 显式喂数据；`stdio: 'inherit'` 也支持；
//! - 超时 / 输出超过 `maxBuffer` 时杀**整棵进程树**，并返回结构化错误；
//! - 异步 `spawn` / `exec` / `execFile` / `fork` 明确报不支持，不伪装成可用。
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

            function unsupported(name, suggestion) {
                return function() {
                    throw new Error(
                        'TS 微内核暂不支持异步 child_process.' + name + '；请改用 ' +
                        suggestion + '（同步 API）。'
                    );
                };
            }

            globalThis.childProcess = {
                spawnSync: spawnSync,
                execFileSync: execFileSync,
                execSync: execSync,
                spawn: unsupported('spawn', 'spawnSync'),
                exec: unsupported('exec', 'execSync'),
                execFile: unsupported('execFile', 'execFileSync'),
                fork: unsupported('fork', 'spawnSync'),
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

fn run_child(spec_json: &str) -> Result<String, String> {
    let spec: Value = serde_json::from_str(spec_json)
        .map_err(|error| format!("child_process 参数不是合法 JSON: {error}"))?;
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
    let timeout_ms = spec.get("timeoutMs").and_then(Value::as_u64).unwrap_or(0);
    let max_buffer = match spec.get("maxBuffer").and_then(Value::as_u64) {
        // 0 = 不设上限（功能优先；调用方自己负责内存）。
        Some(0) => usize::MAX,
        Some(value) => value.min(usize::MAX as u64) as usize,
        None => DEFAULT_MAX_BUFFER,
    };
    let encoding = match spec.get("encoding").and_then(Value::as_str) {
        Some("buffer") => "buffer",
        _ => "utf8",
    };
    let shell = spec.get("shell").and_then(Value::as_bool).unwrap_or(false);
    let shell_whole = spec
        .get("shellWhole")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stdin_mode = spec
        .get("stdinMode")
        .and_then(Value::as_str)
        .unwrap_or("pipe");
    let stdout_mode = spec
        .get("stdoutMode")
        .and_then(Value::as_str)
        .unwrap_or("pipe");
    let stderr_mode = spec
        .get("stderrMode")
        .and_then(Value::as_str)
        .unwrap_or("pipe");

    let mut command = if shell {
        if shell_whole {
            shell_command(&file)
        } else {
            shell_command(&shell_line(&file, &args))
        }
    } else {
        let mut command = Command::new(&file);
        command.args(&args);
        command
    };
    command
        .current_dir(&cwd)
        .env("A_DA_AGENT", "1")
        .stdin(stdio_for(stdin_mode))
        .stdout(stdio_for(stdout_mode))
        .stderr(stdio_for(stderr_mode));
    if let Some(env) = spec.get("env").and_then(Value::as_object) {
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
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

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
        match decode_input(spec.get("input")) {
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

fn stdio_for(mode: &str) -> Stdio {
    match mode {
        "inherit" => Stdio::inherit(),
        "ignore" | "null" => Stdio::null(),
        _ => Stdio::piped(),
    }
}

fn shell_line(file: &str, args: &[String]) -> String {
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

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut cmd = Command::new(comspec);
        cmd.args(["/d", "/s", "/c", command]);
        cmd
    } else {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command]);
        cmd
    }
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

fn signal_of(status: &ExitStatus) -> Option<String> {
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

fn kill_process_tree(pid: u32) {
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
