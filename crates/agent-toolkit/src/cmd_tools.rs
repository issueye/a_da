use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::fs_tools::{ToolFailure, ToolResult};
use super::sandbox::check_workspace_sandbox;


const DEFAULT_TIMEOUT_S: u64 = 120;
const MAX_TIMEOUT_S: u64 = 600;
const MAX_BUFFER_BYTES: usize = 1024 * 1024; // 1MB 内存滑动窗口保护
/// 轮次取消的轮询间隔（W4-T1）。
///
/// `CancelToken` 端口只有轮询式 `is_cancelled()`，没有"等它发生"的入口
/// （加方法属端口变更，需单独排任务）。50ms 对"用户按停止键"完全够用。
pub(crate) const CANCEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// 在 Windows 上强制终结进程树
#[cfg(windows)]
pub(crate) fn kill_process_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
}

#[cfg(not(windows))]
pub(crate) fn kill_process_tree(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &format!("-{}", pid)])
        .output();
}

/// 将过长的输出按滑动窗口截断，保留头尾
pub(crate) fn truncate_buffer(buf: &str) -> String {
    if buf.len() <= MAX_BUFFER_BYTES {
        return buf.to_string();
    }
    let head_len = 64 * 1024;
    let tail_len = MAX_BUFFER_BYTES - head_len - 100;
    let head = &buf[..head_len.min(buf.len())];
    let tail_start = buf.len().saturating_sub(tail_len);
    let tail = &buf[tail_start..];
    format!("{}\n...[输出过长，已截断中间日志]...\n{}", head, tail)
}

/// 执行命令
pub async fn run_command(
    workspace: &Path,
    command_str: &str,
    cwd: Option<&str>,
    timeout_s: Option<u64>,
    mut abort_rx: Option<watch::Receiver<bool>>,
) -> ToolResult {
    let run_cwd = match cwd {
        Some(p) if !p.trim().is_empty() => match check_workspace_sandbox(workspace, p.trim()) {
            Ok(checked) => checked,
            Err(e) => return ToolResult::error(e.to_string()),
        },
        _ => workspace.to_path_buf(),
    };

    let timeout_duration = Duration::from_secs(
        timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S),
    );

    let (shell, shell_args) = if cfg!(windows) {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        (comspec, vec!["/d".to_string(), "/s".to_string(), "/c".to_string(), command_str.to_string()])
    } else {
        ("/bin/sh".to_string(), vec!["-c".to_string(), command_str.to_string()])
    };

    let mut cmd = Command::new(shell);
    cmd.args(&shell_args)
        .current_dir(run_cwd)
        .env("A_DA_AGENT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("启动子进程失败: {}", e)),
    };

    let child_pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();


    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stream) = stdout {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stream.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_BUFFER_BYTES * 2 {
                    // 防止未受限持续吞噬内存
                    let s = String::from_utf8_lossy(&buf);
                    buf = truncate_buffer(&s).into_bytes();
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stream) = stderr {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stream.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_BUFFER_BYTES * 2 {
                    let s = String::from_utf8_lossy(&buf);
                    buf = truncate_buffer(&s).into_bytes();
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    // 等待命令完成、超时或中止
    let wait_child = async { child.wait().await };

    tokio::select! {
        exit_res = wait_child => {
            let (stdout_res, stderr_res) = tokio::join!(stdout_task, stderr_task);
            let raw_stdout = stdout_res.unwrap_or_default();
            let raw_stderr = stderr_res.unwrap_or_default();

            let out_str = truncate_buffer(&raw_stdout);
            let err_str = truncate_buffer(&raw_stderr);

            let combined_output = match (out_str.trim().is_empty(), err_str.trim().is_empty()) {
                (false, false) => format!("{}\n[stderr]:\n{}", out_str, err_str),
                (false, true) => out_str,
                (true, false) => err_str,
                (true, true) => "(命令无输出)".to_string(),
            };

            match exit_res {
                Ok(status) if status.success() => ToolResult::success(combined_output),
                Ok(status) => ToolResult::failed(
                    ToolFailure::NonZeroExit,
                    format!(
                        "命令非零退出（退出码 {}）\n{}",
                        status.code().unwrap_or(-1),
                        combined_output
                    ),
                ),
                Err(e) => ToolResult::error(format!("等待命令退出失败: {}", e)),
            }
        }
        _ = tokio::time::sleep(timeout_duration) => {
            if let Some(pid) = child_pid {
                kill_process_tree(pid);
            }
            // W4-T2：结构化原因，不靠输出措辞
            ToolResult::failed(
                ToolFailure::Timeout,
                format!("命令执行超时（超过 {} 秒）", timeout_duration.as_secs()),
            )
        }
        _ = async {
            if let Some(rx) = &mut abort_rx {
                while rx.changed().await.is_ok() {
                    if *rx.borrow() {
                        return;
                    }
                }
            }
            std::future::pending::<()>().await;
        } => {
            if let Some(pid) = child_pid {
                kill_process_tree(pid);
            }
            // W4-T2：结构化原因（原先上层靠 `output.contains("取消")` 猜）
            ToolResult::failed(ToolFailure::Aborted, "用户中止了命令执行。")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_command_success() {
        let ws = std::env::current_dir().unwrap();
        let cmd = if cfg!(windows) { "echo Hello_Rust" } else { "echo Hello_Rust" };
        let result = run_command(&ws, cmd, None, Some(5), None).await;
        assert!(result.ok);
        assert!(result.output.contains("Hello_Rust"));
    }

    #[tokio::test]
    async fn test_run_command_timeout() {
        let ws = std::env::current_dir().unwrap();
        // 睡眠 5 秒，但设定 1 秒超时
        let cmd = if cfg!(windows) { "ping 127.0.0.1 -n 4 > nul" } else { "sleep 4" };
        let result = run_command(&ws, cmd, None, Some(1), None).await;
        assert!(!result.ok);
        assert!(result.output.contains("命令执行超时"));
        // W4-T2：失败原因必须**结构化**给出，而不是靠上层解析输出措辞
        assert_eq!(
            result.failure(),
            Some(ToolFailure::Timeout),
            "超时必须带 ToolFailure::Timeout"
        );
    }

    /// W4-T2：非零退出与"其它失败"必须能被区分开。
    #[tokio::test]
    async fn test_non_zero_exit_is_structurally_reported() {
        let ws = std::env::current_dir().unwrap();
        let result = run_command(&ws, "exit 7", None, Some(10), None).await;
        assert!(!result.ok);
        assert_eq!(result.failure(), Some(ToolFailure::NonZeroExit));

        // 参数/IO 类失败归 Other
        let bad_cwd = run_command(&ws, "echo x", Some("../../../../Windows"), Some(10), None).await;
        assert!(!bad_cwd.ok);
        assert_eq!(bad_cwd.failure(), Some(ToolFailure::Other));
    }

    /// 成功路径**不带**失败原因（否则上层无法用 `failure()` 判成功）。
    #[tokio::test]
    async fn test_success_has_no_failure_reason() {
        let ws = std::env::current_dir().unwrap();
        let result = run_command(&ws, "echo ok", None, Some(10), None).await;
        assert!(result.ok);
        assert_eq!(result.failure(), None);
    }
}
