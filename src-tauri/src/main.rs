// 在 release 模式下隐藏 Windows 控制台黑框
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{Parser, Subcommand};
use std::io::Write;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 命令行参数定义
#[derive(Parser, Debug)]
#[command(
    name = "a-da",
    author = "a_da team",
    version,
    about = "a_da 原生双角色 AI 编码智能体 (Tauri GUI + Headless Core)",
    long_about = "a_da 是一款基于 Rust 与 Tauri 的本地 AI 编码智能体。\n支持作为 GUI 桌面端双击启动，亦支持在命令行作为 Headless 核心后台服务或 CLI 执行器运行。"
)]
pub struct CliArgs {
    /// 以 Headless / 后台服务模式运行（无桌面 GUI）
    #[arg(long, default_value_t = false)]
    pub headless: bool,

    /// 核心服务监听端口（0 表示由操作系统自动分配空闲端口）
    #[arg(long, default_value_t = 0)]
    pub port: u16,

    /// 握手认证令牌（若未指定则自动生成 UUID）
    #[arg(long)]
    pub token: Option<String>,

    /// 工作区根目录路径
    #[arg(long, default_value = "")]
    pub workspace: String,

    /// 直连外部 ada-coding 核心服务 WebSocket 地址 (如 ws://127.0.0.1:4000/rpc)
    #[arg(long)]
    pub connect: Option<String>,

    /// 显式指定外部 ada-coding 二进制可执行文件路径
    #[arg(long)]
    pub host_bin: Option<String>,

    /// 强制使用同进程内嵌微内核，不连接也不拉起外部 ada-coding
    #[arg(long, default_value_t = false)]
    pub inprocess: bool,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum CliCommand {
    /// 以守护进程/核心服务模式启动
    Daemon {
        /// 服务监听端口（0 表示由操作系统分配空闲端口）
        #[arg(long, default_value_t = 0)]
        port: u16,

        /// 握手认证令牌
        #[arg(long)]
        token: Option<String>,

        /// 工作区根目录
        #[arg(long, default_value = "")]
        workspace: String,
    },
    /// 执行单次 CLI 指令任务
    Run {
        /// 工作区根目录
        #[arg(long, default_value = "")]
        workspace: String,

        /// 任务描述或指令提示词
        prompt: Option<String>,
    },
}

/// 仅当命令行带有参数调用时附加父进程控制台，确保双击启动无黑框，而命令行下运行可正常输出
#[cfg(windows)]
fn attach_console_if_needed() {
    if std::env::args().len() > 1 {
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
}

#[cfg(not(windows))]
fn attach_console_if_needed() {}

/// 把 panic 信息落到磁盘。
///
/// 注意：`PanicHookInfo` 的 `Debug` 输出会把 payload 折叠成 `Any { .. }`，
/// 真正的原因文本（panic! 的第一个参数）会丢失。必须手动 downcast 取出，
/// 否则日志里只剩一行 `Failed to setup app` 的位置，等于没有诊断信息。
fn write_panic_log(text: &str) {
    // 优先写到可执行文件旁边（双击启动时 cwd 就是 exe 目录，但安装到别处时不保证）
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

/// GUI 模式下（双击启动、无任何控制台）必须弹一个框。
///
/// release 版是 `windows_subsystem = "windows"`，stderr 无人接收、日志没人会主动去翻，
/// 用户看到的就是"双击之后毫无反应"。这里把致命错误显式端到用户面前。
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
        // 无控制台时 eprintln 是空操作，写文件才是唯一可靠的证据
        eprintln!("{}", text);
        write_panic_log(&text);

        // 无参数启动 = 双击进入 GUI 模式，此时没有任何可见通道，只能弹框
        if std::env::args().len() <= 1 {
            show_fatal_dialog(&text);
        }
    }));

    // 优先尝试附加控制台（仅有命令行参数时附加）
    attach_console_if_needed();

    let args = CliArgs::parse();

    // 判断是否进入 Headless / CLI 模式
    let is_headless_or_cli = args.headless || args.command.is_some();

    if !is_headless_or_cli {
        // GUI 桌面模式（默认双击启动或命令行带参启动窗口）
        let launcher_config = a_da_tauri::LauncherConfig {
            workspace: args.workspace.clone(),
            connect: args.connect,
            token: args.token,
            host_bin: args.host_bin,
            force_inprocess: args.inprocess,
        };
        a_da_tauri::run(launcher_config);
        return Ok(());
    }

    // 初始化日志记录器（命令行/Headless 模式下输出至终端）
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    // 分流处理不同 CLI 场景
    match args.command {
        Some(CliCommand::Run { workspace, prompt }) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let ws = if !workspace.is_empty() {
                    workspace
                } else if !args.workspace.is_empty() {
                    args.workspace.clone()
                } else {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                };

                println!("a-da 命令行任务执行器启动，工作区: {}", ws);
                if let Some(task_prompt) = prompt {
                    println!("执行任务: {}", task_prompt);
                    let mut store = agent_core::AgentStore::new(ws);
                    let tid = store.create_thread(Some("CLI 任务会话".to_string()), None);
                    println!("创建任务会话成功: {}", tid);
                } else {
                    println!("提示: 请提供需要执行的任务指令，例如: a-da run --workspace . \"审查代码\"");
                }
                std::io::stdout().flush()?;
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Some(CliCommand::Daemon {
            port,
            token,
            workspace,
        }) => {
            let actual_port = if port > 0 { port } else { args.port };
            let actual_token = token
                .filter(|t| !t.trim().is_empty())
                .or(args.token)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let actual_ws = if !workspace.is_empty() {
                workspace
            } else if !args.workspace.is_empty() {
                args.workspace.clone()
            } else {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            };

            run_headless_server(actual_port, actual_token, actual_ws)?;
        }
        None => {
            // --headless 模式
            let actual_token = args
                .token
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let actual_ws = if !args.workspace.is_empty() {
                args.workspace
            } else {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            };

            run_headless_server(args.port, actual_token, actual_ws)?;
        }
    }

    Ok(())
}

/// 运行 Headless 核心 WebSocket 服务
fn run_headless_server(
    port: u16,
    token: String,
    workspace: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        let store = Arc::new(RwLock::new(agent_core::AgentStore::new(workspace)));
        let server = agent_core::WsHostServer::bind(port, token.clone(), store).await?;
        let current_pid = std::process::id();

        let ready_json = serde_json::json!({
            "ready": true,
            "port": server.port,
            "token": token,
            "pid": current_pid,
            "protocolVersion": agent_core::protocol::PROTOCOL_VERSION,
        });

        // 打印符合协议规范的标准就绪行
        println!("A_DA_HOST_READY {}", ready_json);
        std::io::stdout().flush()?;

        // 监听退出信号
        #[cfg(target_os = "windows")]
        {
            match tokio::signal::windows::ctrl_c() {
                Ok(mut sig) => {
                    sig.recv().await;
                }
                Err(_) => {
                    std::future::pending::<()>().await;
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        println!("a-da 核心后台服务已安全退出");
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_args_parsing_defaults() {
        let args = CliArgs::try_parse_from(["a-da"]).expect("解析默认参数失败");
        assert!(!args.headless);
        assert_eq!(args.port, 0);
        assert_eq!(args.token, None);
        assert_eq!(args.workspace, "");
        assert_eq!(args.command, None);
    }

    #[test]
    fn test_cli_args_parsing_headless() {
        let args = CliArgs::try_parse_from([
            "a-da",
            "--headless",
            "--port",
            "52353",
            "--token",
            "secret_token",
            "--workspace",
            "E:/workspace",
        ])
        .expect("解析 headless 参数失败");
        assert!(args.headless);
        assert_eq!(args.port, 52353);
        assert_eq!(args.token, Some("secret_token".to_string()));
        assert_eq!(args.workspace, "E:/workspace");
    }

    #[test]
    fn test_cli_args_parsing_daemon_subcommand() {
        let args = CliArgs::try_parse_from([
            "a-da",
            "daemon",
            "--port",
            "9999",
            "--token",
            "token_abc",
            "--workspace",
            "E:/codes",
        ])
        .expect("解析 daemon 子命令失败");
        assert_eq!(
            args.command,
            Some(CliCommand::Daemon {
                port: 9999,
                token: Some("token_abc".to_string()),
                workspace: "E:/codes".to_string(),
            })
        );
    }

    #[test]
    fn test_cli_args_parsing_run_subcommand() {
        let args = CliArgs::try_parse_from([
            "a-da",
            "run",
            "--workspace",
            "E:/projects",
            "修复这个编译错误",
        ])
        .expect("解析 run 子命令失败");
        assert_eq!(
            args.command,
            Some(CliCommand::Run {
                workspace: "E:/projects".to_string(),
                prompt: Some("修复这个编译错误".to_string()),
            })
        );
    }
}
