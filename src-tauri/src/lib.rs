use agent_core::server::WsHostServer;
use agent_core::state::AgentStore;
use std::sync::Arc;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 桌面客户端启动配置
#[derive(Debug, Clone, Default)]
pub struct LauncherConfig {
    /// 目标工作区路径
    pub workspace: String,
    /// 直连外部 ada-coding 核心服务 WebSocket 地址 (如 ws://127.0.0.1:4000/rpc)
    pub connect: Option<String>,
    /// 握手认证令牌（与外部或拉起的 ada-coding 共享）
    pub token: Option<String>,
    /// 显式指定外部 ada-coding 二进制执行文件路径
    pub host_bin: Option<String>,
    /// 强制同进程内嵌模式（不探测也不拉起外部 ada-coding）
    pub force_inprocess: bool,
}

/// 核心服务状态内部数据（支持同进程内嵌或独立 ada-coding 进程托管）
pub struct CoreServiceInner {
    pub alive: bool,
    pub port: u16,
    pub token: String,
    pub url: String,
    pub server: Option<Arc<WsHostServer>>,
    pub child_pid: Option<u32>,
}

/// Tauri Managed State 封装
#[derive(Clone)]
pub struct CoreServiceState {
    pub inner: Arc<RwLock<CoreServiceInner>>,
}

impl CoreServiceState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(CoreServiceInner {
                alive: false,
                port: 0,
                token: String::new(),
                url: String::new(),
                server: None,
                child_pid: None,
            })),
        }
    }
}

impl Default for CoreServiceState {
    fn default() -> Self {
        Self::new()
    }
}

/// 从 WebSocket URL 中提取端口号 (例如: ws://127.0.0.1:4000/rpc -> 4000)
pub fn extract_port_from_url(url: &str) -> Option<u16> {
    if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        let host_port = rest.split('/').next()?;
        if let Some(colon) = host_port.rfind(':') {
            return host_port[colon + 1..].parse().ok();
        }
    }
    None
}

/// 拉起子进程时禁止为它新建控制台窗口。
///
/// `ada-coding` 是**控制台子系统**的二进制，而 GUI 自身是 windows 子系统、没有控制台：
/// 这种情况下 CreateProcess 默认会给子进程**新分配一个控制台**，也就是双击后一闪而出的
/// 黑窗。加 `CREATE_NO_WINDOW` 后子进程依旧拿得到我们 pipe 过去的 stdout/stderr
/// （`A_DA_HOST_READY` 就绪行照常读到），只是不再有窗口。
#[cfg(windows)]
fn hide_child_console_window(cmd: &mut tokio::process::Command) {
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console_window(_cmd: &mut tokio::process::Command) {}

/// 探测本地可用的 ada-coding 二进制可执行文件路径
pub fn find_ada_coding_binary(custom_bin: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(custom) = custom_bin {
        let p = std::path::PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }

    let exe_name = if cfg!(windows) { "ada-coding.exe" } else { "ada-coding" };

    // 候选 1: 同目录（发布形态或同一构建输出目录）
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let candidate = parent.join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 候选 2: 环境变量 CARGO_TARGET_DIR
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        let candidate_debug = std::path::Path::new(&td).join("debug").join(exe_name);
        if candidate_debug.exists() {
            return Some(candidate_debug);
        }
        let candidate_release = std::path::Path::new(&td).join("release").join(exe_name);
        if candidate_release.exists() {
            return Some(candidate_release);
        }
    }

    // 候选 3: 本地常规工程缓存路径
    let fallback_paths = [
        format!("../cargo_target_ada/debug/{}", exe_name),
        format!("../cargo_target_ada/release/{}", exe_name),
        format!("target/debug/{}", exe_name),
        format!("target/release/{}", exe_name),
    ];
    for fb in fallback_paths {
        let p = std::path::PathBuf::from(fb);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// 获取 Core 服务的连接信息（供前端 ws-client 启动时调用）
#[tauri::command]
async fn get_core_info(
    state: tauri::State<'_, CoreServiceState>,
) -> Result<serde_json::Value, String> {
    // 轮询等待核心服务完成本地端口绑定或握手就绪（最多等待 6 秒）
    for _ in 0..60 {
        let guard = state.inner.read().await;
        if guard.alive && (guard.port > 0 || !guard.url.is_empty()) {
            let port = if guard.port > 0 { guard.port } else { 52353 };
            let url = if !guard.url.is_empty() {
                guard.url.clone()
            } else {
                format!("ws://127.0.0.1:{}/rpc", port)
            };
            return Ok(serde_json::json!({
                "alive": true,
                "port": port,
                "token": guard.token,
                "url": url,
            }));
        }
        drop(guard);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let guard = state.inner.read().await;
    Ok(serde_json::json!({
        "alive": guard.alive,
        "port": guard.port,
        "token": guard.token,
        "url": if !guard.url.is_empty() {
            guard.url.clone()
        } else if guard.port > 0 {
            format!("ws://127.0.0.1:{}/rpc", guard.port)
        } else {
            "ws://127.0.0.1:52353/rpc".to_string()
        }
    }))
}

/// 兼容旧版仅传工作区的启动入口
pub fn run_with_workspace(workspace: String) {
    run(LauncherConfig {
        workspace,
        ..Default::default()
    });
}

/// 启动 Tauri GUI 桌面应用（支持连接外部 ada-coding、自 spawn ada-coding 或内嵌模式）
pub fn run(config: LauncherConfig) {
    let core_state = CoreServiceState::new();
    let inner_clone = core_state.inner.clone();

    let app_res = tauri::Builder::default()
        .manage(core_state)
        .setup(move |_app| {
            let state_arc = inner_clone.clone();
            let cfg = config.clone();

            tauri::async_runtime::spawn(async move {
                // 场景 A: 显式指定直连外部服务 URL
                if let Some(external_url) = cfg.connect {
                    let port = extract_port_from_url(&external_url).unwrap_or(52353);
                    let token = cfg.token.unwrap_or_default();
                    info!("a-da 桌面端配置直连外部核心服务: {}", external_url);
                    let mut guard = state_arc.write().await;
                    guard.alive = true;
                    guard.port = port;
                    guard.token = token;
                    guard.url = external_url;
                    return;
                }

                // 场景 B: 优先寻找并拉起独立 ada-coding 核心子进程
                if !cfg.force_inprocess {
                    if let Some(host_bin) = find_ada_coding_binary(cfg.host_bin.as_deref()) {
                        info!("找到独立 ada-coding 二进制: {}，正在拉起宿主服务...", host_bin.display());
                        let current_pid = std::process::id();
                        let token = cfg.token.clone().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());

                        let mut cmd = tokio::process::Command::new(&host_bin);
                        cmd.arg("--host")
                            .arg("--port").arg("0")
                            .arg("--token").arg(&token)
                            .arg("--parent-pid").arg(current_pid.to_string());
                        if !cfg.workspace.is_empty() {
                            cmd.arg("--workspace").arg(&cfg.workspace);
                        }
                        cmd.stdout(std::process::Stdio::piped());
                        cmd.stderr(std::process::Stdio::piped());
                        hide_child_console_window(&mut cmd);

                        match cmd.spawn() {
                            Ok(mut child) => {
                                let child_pid = child.id().unwrap_or(0);
                                info!("ada-coding 子进程已启动，PID: {}", child_pid);

                                let stdout = child.stdout.take();
                                let stderr = child.stderr.take();

                                // 监听子进程 stdout，捕获标准 A_DA_HOST_READY 就绪信号
                                let state_clone = state_arc.clone();
                                let token_clone = token.clone();
                                let mut ready_received = false;

                                if let Some(out) = stdout {
                                    let mut reader = BufReader::new(out).lines();
                                    // 限制等待就绪最多 8 秒
                                    let wait_ready = async {
                                        while let Ok(Some(line)) = reader.next_line().await {
                                            info!("[ada-coding stdout] {}", line);
                                            if line.starts_with("A_DA_HOST_READY ") {
                                                let json_str = line.trim_start_matches("A_DA_HOST_READY ");
                                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                                                    if let Some(p) = v.get("port").and_then(|x| x.as_u64()) {
                                                        return Some(p as u16);
                                                    }
                                                }
                                            }
                                        }
                                        None
                                    };

                                    match tokio::time::timeout(tokio::time::Duration::from_secs(8), wait_ready).await {
                                        Ok(Some(actual_port)) => {
                                            ready_received = true;
                                            info!("成功连接独立 ada-coding 核心服务: ws://127.0.0.1:{}/rpc", actual_port);
                                            let mut guard = state_clone.write().await;
                                            guard.alive = true;
                                            guard.port = actual_port;
                                            guard.token = token_clone;
                                            guard.url = format!("ws://127.0.0.1:{}/rpc", actual_port);
                                            guard.child_pid = Some(child_pid);
                                        }
                                        _ => {
                                            warn!("未在预期时限内收到 ada-coding 的就绪信号，准备安全降级");
                                        }
                                    }

                                    // 继续在后台消耗后续 stdout，避免管道填满挂起
                                    tauri::async_runtime::spawn(async move {
                                        while let Ok(Some(line)) = reader.next_line().await {
                                            info!("[ada-coding stdout] {}", line);
                                        }
                                    });
                                }

                                if let Some(err) = stderr {
                                    tauri::async_runtime::spawn(async move {
                                        let mut reader = BufReader::new(err).lines();
                                        while let Ok(Some(line)) = reader.next_line().await {
                                            warn!("[ada-coding stderr] {}", line);
                                        }
                                    });
                                }

                                if ready_received {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!("无法启动 ada-coding 子进程: {}，将降级回退到内嵌核心模式", e);
                            }
                        }
                    }
                }

                // 场景 C: 兜底回退模式 —— 同进程内嵌核心服务
                let token = cfg.token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let store = Arc::new(RwLock::new(AgentStore::new(cfg.workspace)));

                let resolved_ws = store.read().await.workspace.project.clone();
                info!("启动 a-da 内嵌同进程核心服务... 工作区: {}", resolved_ws);
                match WsHostServer::bind(0, token.clone(), store).await {
                    Ok(server) => {
                        let actual_port = server.port;
                        info!(
                            "a-da 内嵌核心服务启动成功: ws://127.0.0.1:{}/rpc (端口: {})",
                            actual_port, actual_port
                        );
                        let mut guard = state_arc.write().await;
                        guard.alive = true;
                        guard.port = actual_port;
                        guard.token = token;
                        guard.url = format!("ws://127.0.0.1:{}/rpc", actual_port);
                        guard.server = Some(server);
                    }
                    Err(e) => {
                        error!("启动 a-da 内嵌核心服务失败: {}", e);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_core_info])
        .build(tauri::generate_context!());

    let app = match app_res {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("Tauri build error: {:?}", e);
            let _ = std::fs::write("tauri_error.log", &msg);
            eprintln!("{}", msg);
            return;
        }
    };

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            info!("接收到 Tauri 窗口退出事件，正在清理核心服务资源...");
            if let Some(state) = app_handle.try_state::<CoreServiceState>() {
                let inner = state.inner.clone();
                tauri::async_runtime::block_on(async move {
                    let mut guard = inner.write().await;
                    guard.alive = false;
                    guard.server = None;

                    // 若存在托管的 ada-coding 子进程，安全终止
                    #[cfg(windows)]
                    if let Some(pid) = guard.child_pid {
                        unsafe {
                            use windows_sys::Win32::System::Threading::{
                                OpenProcess, TerminateProcess, PROCESS_TERMINATE,
                            };
                            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                            if !handle.is_null() {
                                let _ = TerminateProcess(handle, 0);
                                windows_sys::Win32::Foundation::CloseHandle(handle);
                            }
                        }
                    }
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_port_from_url() {
        assert_eq!(extract_port_from_url("ws://127.0.0.1:4000/rpc"), Some(4000));
        assert_eq!(extract_port_from_url("ws://localhost:52353/rpc"), Some(52353));
        assert_eq!(extract_port_from_url("http://example.com/api"), None);
    }

    #[tokio::test]
    async fn test_core_service_state_lifecycle() {
        let state = CoreServiceState::new();
        {
            let guard = state.inner.read().await;
            assert!(!guard.alive);
            assert_eq!(guard.port, 0);
            assert!(guard.token.is_empty());
        }

        let store = Arc::new(RwLock::new(AgentStore::new("E:/codes/rust_projects/a_da".to_string())));
        let test_token = "test_secret_token_123".to_string();
        let server = WsHostServer::bind(0, test_token.clone(), store).await.expect("服务绑定失败");
        let port = server.port;
        assert!(port > 0);

        {
            let mut guard = state.inner.write().await;
            guard.alive = true;
            guard.port = port;
            guard.token = test_token.clone();
            guard.server = Some(server);
        }

        {
            let guard = state.inner.read().await;
            assert!(guard.alive);
            assert_eq!(guard.port, port);
            assert_eq!(guard.token, test_token);
        }
    }
}
