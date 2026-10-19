use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// 当前线程的控制台捕获出口（P1·SP-14）。
    ///
    /// 用线程局部而不是往闭包里捕获 `Arc`：console 原生是 `from_copy_closure`（无捕获），
    /// 而**每个运行时独占一个事件循环线程**——于是线程局部正好等于"每个运行时一份"，
    /// 不用为了捕获把 Boa 的闭包签名改成需要 `Trace` 的那种。
    static CONSOLE_CAPTURE: RefCell<Option<Arc<std::sync::Mutex<ConsoleCapture>>>> = RefCell::new(None);
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
        let text = if is_err {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        let total = if is_err {
            &mut self.stderr_bytes
        } else {
            &mut self.stdout_bytes
        };
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
    pub(super) fn take(&mut self) -> ConsoleSnapshot {
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
pub(super) fn capture_console(is_err: bool, line: &str) {
    CONSOLE_CAPTURE.with(|cell| {
        if let Some(arc) = cell.borrow().as_ref() {
            if let Ok(mut guard) = arc.lock() {
                guard.push(is_err, line);
            }
        }
    });
}
/// 把捕获出口挂到当前事件循环线程（由 worker 在启动时调用）。
pub(super) fn install_capture(capture: Arc<std::sync::Mutex<ConsoleCapture>>) {
    CONSOLE_CAPTURE.with(|cell| *cell.borrow_mut() = Some(capture));
}
