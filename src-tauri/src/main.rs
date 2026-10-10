// 在 release 模式下隐藏 Windows 控制台黑框
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{Parser, Subcommand};

/// 命令行参数定义
#[derive(Parser, Debug)]
#[command(
    name = "a-da",
    author = "a_da team",
    version,
    about = "a_da 原生桌面客户端 (Tauri GUI 独立客户端)",
    long_about = "a_da 是一款基于 Rust 与 Tauri 的本地 AI 智能体桌面端。\n支持 AGENT 直连模式（连接 ada-coding / ada-pm）与网关模式（连接 ada-gateway）。"
)]
pub struct CliArgs {
    /// 运行模式：direct（直连 Agent 模式，默认）或 gateway（网关模式）
    #[arg(long)]
    pub mode: Option<String>,

    /// 目标服务 WebSocket 地址（直连 Agent 或网关的 ws:// 地址）
    #[arg(long)]
    pub connect: Option<String>,

    /// 握手认证令牌 / 配对密钥
    #[arg(long)]
    pub token: Option<String>,

    /// 默认工作区根目录路径
    #[arg(long, default_value = "")]
    pub workspace: String,

    /// 自定义 Agent 或网关二进制可执行文件路径
    #[arg(long)]
    pub host_bin: Option<String>,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum CliCommand {
    /// 启动后台守护服务 (提示：建议直接运行 ada-coding 或 a-da-gateway)
    Daemon {
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "")]
        workspace: String,
    },
    /// 执行单次 CLI 指令任务 (提示：建议直接运行 ada-coding run)
    Run {
        #[arg(long, default_value = "")]
        workspace: String,
        prompt: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

/// 尝试附加父进程控制台（如果是在命令行运行），确保能正常输出日志
#[cfg(windows)]
fn attach_console_if_needed() {
    unsafe {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
            STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };

        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            let h_stdout = GetStdHandle(STD_OUTPUT_HANDLE);
            if !h_stdout.is_null() && h_stdout != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_OUTPUT_HANDLE, h_stdout);
            }
            let h_stderr = GetStdHandle(STD_ERROR_HANDLE);
            if !h_stderr.is_null() && h_stderr != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_ERROR_HANDLE, h_stderr);
            }
            let h_stdin = GetStdHandle(STD_INPUT_HANDLE);
            if !h_stdin.is_null() && h_stdin != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_INPUT_HANDLE, h_stdin);
            }
        }
    }
}

#[cfg(not(windows))]
fn attach_console_if_needed() {}

fn write_panic_log(text: &str) {
    let mut targets: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            targets.push(dir.join("tauri_panic.log"));
        }
    }
    targets.push(std::path::PathBuf::from("tauri_panic.log"));

    for target in targets {
        if std::fs::write(&target, text).is_ok() {
            return;
        }
    }
}

#[cfg(windows)]
fn show_fatal_dialog(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let body = wide(text);
    let caption = wide("a_da 启动失败");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_dialog(_text: &str) {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());

        let text = format!(
            "panic: {}\nlocation: {}\nbacktrace: {}\n",
            message,
            location,
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("{}", text);
        write_panic_log(&text);

        if std::env::args().len() <= 1 {
            show_fatal_dialog(&text);
        }
    }));

    attach_console_if_needed();

    let app_home = agent_node::session::get_app_home();
    let log_file_path = std::path::Path::new(&app_home).join("launcher.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path);

    if let Ok(file) = log_file {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .with_target(false)
            .with_writer(file)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    let args = CliArgs::parse();

    if let Some(ref cmd) = args.command {
        match cmd {
            CliCommand::Run { prompt, .. } => {
                println!("a-da 桌面端现已完全独立为纯 GUI 宿主客户端。");
                println!("提示：若需在命令行执行指令任务，请直接使用 ada-coding 独立程序：");
                println!("  ada-coding run --workspace . {:?}", prompt.as_deref().unwrap_or(""));
                return Ok(());
            }
            CliCommand::Daemon { port, .. } => {
                println!("a-da 桌面端现已完全独立为纯 GUI 宿主客户端。");
                println!("提示：若需启动后台守护服务，请直接运行 ada-coding 或 ada-gateway：");
                println!("  ada-coding --host --port {}", port);
                return Ok(());
            }
        }
    }

    // 解析命令行指定的运行模式覆盖
    let parsed_mode = args.mode.as_deref().and_then(|m| match m.to_lowercase().as_str() {
        "direct" | "agent" => Some(ada_tauri::DesktopMode::Direct),
        "gateway" => Some(ada_tauri::DesktopMode::Gateway),
        _ => None,
    });

    let launcher_config = ada_tauri::LauncherConfig {
        workspace: args.workspace,
        mode: parsed_mode,
        connect: args.connect,
        token: args.token,
        host_bin: args.host_bin,
    };

    ada_tauri::run(launcher_config);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_args_parsing_defaults() {
        let args = CliArgs::try_parse_from(["a-da"]).expect("解析默认参数失败");
        assert_eq!(args.mode, None);
        assert_eq!(args.token, None);
        assert_eq!(args.workspace, "");
        assert_eq!(args.command, None);
    }

    #[test]
    fn test_cli_args_parsing_mode() {
        let args = CliArgs::try_parse_from([
            "a-da",
            "--mode",
            "gateway",
            "--connect",
            "ws://127.0.0.1:4000/rpc",
        ])
        .expect("解析网关参数失败");
        assert_eq!(args.mode.as_deref(), Some("gateway"));
        assert_eq!(args.connect.as_deref(), Some("ws://127.0.0.1:4000/rpc"));
    }
}
