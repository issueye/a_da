use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 桌面端运行模式：AGENT 直连模式（默认）或 网关模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopMode {
    /// 直连 Agent 模式（主要是 ada-coding 和 ada-pm）
    Direct,
    /// 网关模式（连接 ada-gateway 平台服务）
    Gateway,
}

impl Default for DesktopMode {
    fn default() -> Self {
        DesktopMode::Direct
    }
}

/// 桌面端运行与连接持久化配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// 运行模式，默认为 Direct 直连模式
    #[serde(default)]
    pub mode: DesktopMode,
    /// 直连模式下的自定义外部 Agent WebSocket 地址 (如 ws://127.0.0.1:4000/rpc)
    #[serde(default)]
    pub agent_connect_url: Option<String>,
    /// 直连模式下的自定义 ada-coding 二进制路径
    #[serde(default)]
    pub agent_bin_path: Option<String>,
    /// 网关模式下的自定义网关 WebSocket 地址 (如 ws://127.0.0.1:4000/rpc)
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// 网关模式下的自定义 ada-gateway 二进制路径
    #[serde(default)]
    pub gateway_bin_path: Option<String>,
    /// 认证 Token / 配对密钥
    #[serde(default)]
    pub token: Option<String>,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            mode: DesktopMode::Direct,
            agent_connect_url: None,
            agent_bin_path: None,
            gateway_url: None,
            gateway_bin_path: None,
            token: None,
        }
    }
}

/// 获取桌面端配置文件路径 (~/.a-da/desktop_launcher.json)
pub fn desktop_config_path() -> std::path::PathBuf {
    let app_home = agent_node::session::get_app_home();
    std::path::Path::new(&app_home).join("desktop_launcher.json")
}

/// 加载持久化配置
pub fn load_desktop_config() -> DesktopConfig {
    let path = desktop_config_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<DesktopConfig>(&content) {
            return cfg;
        }
    }
    DesktopConfig::default()
}

/// 保存持久化配置
pub fn save_desktop_config(cfg: &DesktopConfig) -> Result<(), String> {
    let path = desktop_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(&path, content)
        .map_err(|e| format!("写入配置文件失败: {e}"))?;
    Ok(())
}

/// 桌面客户端命令行启动参数
#[derive(Debug, Clone, Default)]
pub struct LauncherConfig {
    /// 目标工作区路径
    pub workspace: String,
    /// 覆盖配置的运行模式
    pub mode: Option<DesktopMode>,
    /// 覆盖配置的直连或网关 WebSocket 地址
    pub connect: Option<String>,
    /// 握手认证令牌
    pub token: Option<String>,
    /// 自定义二进制程序路径
    pub host_bin: Option<String>,
}

/// 托管子进程信息条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManagedProcessInfo {
    pub name: String,
    pub role: String,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub alive: bool,
    pub status: String, // "running", "stopped", "starting", "error"
    pub url: Option<String>,
}

/// 进程管理监控汇总报告
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessReport {
    pub mode: DesktopMode,
    pub core_alive: bool,
    pub core_url: String,
    pub core_port: u16,
    pub error: Option<String>,
    pub processes: Vec<ManagedProcessInfo>,
}

/// 核心服务内部状态
pub struct CoreServiceInner {
    pub alive: bool,
    pub mode: DesktopMode,
    pub port: u16,
    pub token: String,
    pub url: String,
    pub child_pid: Option<u32>,
    pub child_pids: Vec<u32>,
    pub processes: Vec<ManagedProcessInfo>,
    pub error: Option<String>,
}

/// Tauri Managed State
#[derive(Clone)]
pub struct CoreServiceState {
    pub inner: Arc<RwLock<CoreServiceInner>>,
}

impl CoreServiceState {
    pub fn new(mode: DesktopMode) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CoreServiceInner {
                alive: false,
                mode,
                port: 0,
                token: String::new(),
                url: String::new(),
                child_pid: None,
                child_pids: Vec::new(),
                processes: Vec::new(),
                error: None,
            })),
        }
    }
}

impl Default for CoreServiceState {
    fn default() -> Self {
        Self::new(DesktopMode::Direct)
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

/// 通用二进制探测函数
pub fn find_binary(exe_name_base: &str, custom_bin: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(custom) = custom_bin {
        if !custom.trim().is_empty() {
            let p = std::path::PathBuf::from(custom);
            if p.exists() {
                return Some(p);
            }
        }
    }

    let exe_name = if cfg!(windows) {
        format!("{}.exe", exe_name_base)
    } else {
        exe_name_base.to_string()
    };

    // 候选 1: 当前可执行文件同目录（发布打包形态）
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let candidate = parent.join(&exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 候选 2: 环境变量 CARGO_TARGET_DIR
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        let candidate_debug = std::path::Path::new(&td).join("debug").join(&exe_name);
        if candidate_debug.exists() {
            return Some(candidate_debug);
        }
        let candidate_release = std::path::Path::new(&td).join("release").join(&exe_name);
        if candidate_release.exists() {
            return Some(candidate_release);
        }
    }

    // 候选 3: 本地常规 target 路径
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

/// 拉起子进程时隐藏 Windows 控制台黑框
#[cfg(windows)]
fn hide_child_console_window(cmd: &mut tokio::process::Command) {
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console_window(_cmd: &mut tokio::process::Command) {}

/// 等待子进程的标准输出打印就绪行（并捕获端口号）
async fn wait_for_ready_line(
    mut reader: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    ready_prefix: &'static str,
    timeout: std::time::Duration,
    process_name: &'static str,
) -> Option<u16> {
    let wait_ready = async {
        while let Ok(Some(line)) = reader.next_line().await {
            info!("[{} stdout] {}", process_name, line);
            if line.starts_with(ready_prefix) {
                let json_str = line.trim_start_matches(ready_prefix);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let Some(p) = v.get("port").and_then(|x| x.as_u64()) {
                        return Some(p as u16);
                    }
                }
            }
        }
        None
    };

    let result = tokio::time::timeout(timeout, wait_ready).await.ok().flatten();

    // 后台协程继续跟进打印输出
    tauri::async_runtime::spawn(async move {
        while let Ok(Some(line)) = reader.next_line().await {
            info!("[{} stdout] {}", process_name, line);
        }
    });

    result
}

/// 后台管道跟进打印子进程的标准错误
fn pipe_child_stderr(stderr: Option<tokio::process::ChildStderr>, process_name: &'static str) {
    if let Some(err) = stderr {
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(err).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                warn!("[{} stderr] {}", process_name, line);
            }
        });
    }
}

/// 获取 Core 服务的连接信息（供前端 ws-client 启动连接时调用）
#[tauri::command]
async fn get_core_info(
    state: tauri::State<'_, CoreServiceState>,
) -> Result<serde_json::Value, String> {
    for _ in 0..250 {
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
                "mode": guard.mode,
                "port": port,
                "token": guard.token,
                "url": url,
                "error": null,
            }));
        }
        if let Some(ref err) = guard.error {
            return Ok(serde_json::json!({
                "alive": false,
                "mode": guard.mode,
                "port": 0,
                "token": guard.token,
                "url": "",
                "error": err,
            }));
        }
        drop(guard);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let guard = state.inner.read().await;
    Ok(serde_json::json!({
        "alive": guard.alive,
        "mode": guard.mode,
        "port": guard.port,
        "token": guard.token,
        "url": if !guard.url.is_empty() {
            guard.url.clone()
        } else if guard.port > 0 {
            format!("ws://127.0.0.1:{}/rpc", guard.port)
        } else {
            "ws://127.0.0.1:52353/rpc".to_string()
        },
        "error": guard.error,
    }))
}

/// 获取桌面端配置
#[tauri::command]
fn get_desktop_config() -> Result<DesktopConfig, String> {
    Ok(load_desktop_config())
}

/// 保存桌面端配置（切换模式等）
#[tauri::command]
fn set_desktop_config(config: DesktopConfig) -> Result<(), String> {
    save_desktop_config(&config)
}

/// 重启桌面端程序（重启后生效）
#[tauri::command]
fn restart_desktop_app(app: tauri::AppHandle) {
    info!("前端请求重启桌面端程序...");
    app.restart();
}

/// 检查 PID 是否依然存活
#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            false
        } else {
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            ok != 0 && exit_code == 259 // STILL_ACTIVE = 259
        }
    }
}

#[cfg(not(windows))]
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// 获取当前所有托管进程状态信息
#[tauri::command]
async fn get_process_report(
    state: tauri::State<'_, CoreServiceState>,
) -> Result<ProcessReport, String> {
    let guard = state.inner.read().await;
    let mut procs = guard.processes.clone();
    for p in &mut procs {
        if let Some(pid) = p.pid {
            p.alive = is_pid_alive(pid);
            p.status = if p.alive {
                "running".to_string()
            } else {
                "stopped".to_string()
            };
        }
    }
    Ok(ProcessReport {
        mode: guard.mode,
        core_alive: guard.alive,
        core_url: guard.url.clone(),
        core_port: guard.port,
        error: guard.error.clone(),
        processes: procs,
    })
}

/// 终止指定的托管子进程
#[tauri::command]
fn kill_process(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("无效的 PID".to_string());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return Err(format!("无法打开目标进程句柄 (PID: {})", pid));
            }
            let res = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if res != 0 {
                info!("成功终止进程 PID: {}", pid);
                Ok(())
            } else {
                Err(format!("终止进程失败 (PID: {})", pid))
            }
        }
    }
    #[cfg(not(windows))]
    {
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0 {
            info!("成功终止进程 PID: {}", pid);
            Ok(())
        } else {
            Err(format!("终止进程失败 (PID: {})", pid))
        }
    }
}

/// 兼容仅传工作区的启动入口
pub fn run_with_workspace(workspace: String) {
    run(LauncherConfig {
        workspace,
        ..Default::default()
    });
}

/// 启动 Tauri GUI 桌面应用
pub fn run(config: LauncherConfig) {
    let desktop_config = load_desktop_config();
    let effective_mode = config.mode.unwrap_or(desktop_config.mode);

    let core_state = CoreServiceState::new(effective_mode);
    let inner_clone = core_state.inner.clone();

    let app_res = tauri::Builder::default()
        .manage(core_state)
        .setup(move |_app| {
            let state_arc = inner_clone.clone();
            let cfg = config.clone();
            let d_cfg = desktop_config.clone();

            tauri::async_runtime::spawn(async move {
                match effective_mode {
                    // ==========================================
                    // 1. AGENT 直连模式 (Direct Mode，默认模式)
                    // ==========================================
                    DesktopMode::Direct => {
                        info!("桌面端启动:【AGENT 直连模式】");

                        // 场景 A: 显式指定外部直连地址
                        let direct_url = cfg.connect.or(d_cfg.agent_connect_url);
                        if let Some(external_url) = direct_url {
                            let port = extract_port_from_url(&external_url).unwrap_or(52353);
                            let token = cfg.token.or(d_cfg.token).unwrap_or_default();
                            info!("直连外部 Agent 核心服务: {}", external_url);
                            let mut guard = state_arc.write().await;
                            guard.alive = true;
                            guard.port = port;
                            guard.token = token;
                            guard.url = external_url.clone();
                            guard.processes = vec![ManagedProcessInfo {
                                name: "ada-coding (外部)".to_string(),
                                role: "外部 Agent 核心服务".to_string(),
                                pid: None,
                                port: Some(port),
                                alive: true,
                                status: "running".to_string(),
                                url: Some(external_url),
                            }];
                            return;
                        }

                        // 场景 B: 探测并拉起独立 ada-coding 核心子进程 (集成 Coding 与 PM 双引擎)
                        let custom_bin = cfg.host_bin.as_deref().or(d_cfg.agent_bin_path.as_deref());
                        if let Some(host_bin) = find_binary("ada-coding", custom_bin) {
                            info!("找到独立 ada-coding 二进制: {}，正在拉起...", host_bin.display());
                            let current_pid = std::process::id();
                            let token = cfg.token.or(d_cfg.token).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());

                            let mut cmd = tokio::process::Command::new(&host_bin);
                            cmd.arg("--host")
                                .arg("--port").arg("0")
                                .arg("--token").arg(&token)
                                .arg("--parent-pid").arg(current_pid.to_string());
                            if !cfg.workspace.is_empty() {
                                cmd.arg("--workspace").arg(&cfg.workspace);
                                cmd.current_dir(&cfg.workspace);
                            } else {
                                cmd.current_dir(agent_node::session::get_app_home());
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

                                    let state_clone = state_arc.clone();
                                    let token_clone = token.clone();
                                    let mut ready_received = false;

                                    if let Some(out) = stdout {
                                        let mut reader = BufReader::new(out).lines();
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

                                        match tokio::time::timeout(tokio::time::Duration::from_secs(10), wait_ready).await {
                                            Ok(Some(actual_port)) => {
                                                ready_received = true;
                                                info!("成功连接独立 ada-coding 服务: ws://127.0.0.1:{}/rpc", actual_port);
                                                let mut guard = state_clone.write().await;
                                                guard.alive = true;
                                                guard.port = actual_port;
                                                guard.token = token_clone;
                                                guard.url = format!("ws://127.0.0.1:{}/rpc", actual_port);
                                                guard.child_pid = Some(child_pid);
                                                guard.child_pids = vec![child_pid];
                                                guard.processes = vec![ManagedProcessInfo {
                                                    name: "ada-coding".to_string(),
                                                    role: "Coding / PM 核心引擎服务".to_string(),
                                                    pid: Some(child_pid),
                                                    port: Some(actual_port),
                                                    alive: true,
                                                    status: "running".to_string(),
                                                    url: Some(format!("ws://127.0.0.1:{}/rpc", actual_port)),
                                                }];
                                            }
                                            _ => {
                                                warn!("未在预期时限内收到 ada-coding 就绪信号");
                                                let mut guard = state_clone.write().await;
                                                guard.error = Some("未在预期时限内收到 ada-coding 就绪信号".to_string());
                                            }
                                        }

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
                                    error!("无法拉起 ada-coding 进程: {e}");
                                    let mut guard = state_arc.write().await;
                                    guard.error = Some(format!("启动 ada-coding 失败: {e}"));
                                }
                            }
                        } else {
                            warn!("未探测到 ada-coding 二进制可执行文件！");
                            let mut guard = state_arc.write().await;
                            guard.error = Some("未找到 ada-coding 二进制文件，请确认已编译或在设置中指定路径".to_string());
                        }
                    }

                    // ==========================================
                    // 2. 网关模式 (Gateway Mode)
                    // ==========================================
                    DesktopMode::Gateway => {
                        info!("桌面端启动:【网关模式】");

                        // 场景 A: 显式指定网关地址
                        let gw_url = cfg.connect.or(d_cfg.gateway_url);
                        if let Some(external_url) = gw_url {
                            let port = extract_port_from_url(&external_url).unwrap_or(4000);
                            let token = cfg.token.or(d_cfg.token).unwrap_or_default();
                            info!("连接外部网关服务: {}", external_url);
                            let mut guard = state_arc.write().await;
                            guard.alive = true;
                            guard.port = port;
                            guard.token = token;
                            guard.url = external_url.clone();
                            guard.processes = vec![ManagedProcessInfo {
                                name: "ada-gateway (外部)".to_string(),
                                role: "外部接入网关平台".to_string(),
                                pid: None,
                                port: Some(port),
                                alive: true,
                                status: "running".to_string(),
                                url: Some(external_url),
                            }];
                            return;
                        }

                        // 场景 B: 探测并拉起独立网关服务 (ada-gateway)
                        let custom_gw_bin = d_cfg.gateway_bin_path.as_deref();
                        let gateway_bin = find_binary("ada-gateway", custom_gw_bin)
                            .or_else(|| find_binary("a-da-gateway", custom_gw_bin));

                        if gateway_bin.is_none() {
                            warn!("未探测到 ada-gateway 二进制可执行文件！");
                            let mut guard = state_arc.write().await;
                            guard.error = Some("未找到 ada-gateway 二进制文件，请确认已编译或在设置中指定网关地址".to_string());
                            return;
                        }

                        let gateway_bin = gateway_bin.unwrap();
                        let token = cfg.token.or(d_cfg.token).unwrap_or_default();

                        // 启动 a-da-gateway 平台服务（由网关自身接管 Agent 实例生命周期）
                        info!("正在拉起网关程序: {}", gateway_bin.display());
                        let mut gw_cmd = tokio::process::Command::new(&gateway_bin);
                        gw_cmd.arg("--port").arg("0");
                        gw_cmd.arg("--allow-origin").arg("*");
                        if !cfg.workspace.is_empty() {
                            gw_cmd.arg("--workspace").arg(&cfg.workspace);
                            gw_cmd.current_dir(&cfg.workspace);
                        } else {
                            gw_cmd.current_dir(agent_node::session::get_app_home());
                        }
                        if !token.is_empty() {
                            gw_cmd.arg("--token").arg(&token);
                        }
                        gw_cmd.stdout(std::process::Stdio::piped());
                        gw_cmd.stderr(std::process::Stdio::piped());
                        hide_child_console_window(&mut gw_cmd);

                        let mut gw_child = match gw_cmd.spawn() {
                            Ok(c) => c,
                            Err(e) => {
                                error!("无法拉起 ada-gateway 进程: {e}");
                                let mut guard = state_arc.write().await;
                                guard.error = Some(format!("启动 ada-gateway 失败: {e}"));
                                return;
                            }
                        };

                        let gw_pid = gw_child.id().unwrap_or(0);
                        pipe_child_stderr(gw_child.stderr.take(), "ada-gateway");

                        let Some(gw_out) = gw_child.stdout.take() else {
                            error!("ada-gateway stdout 管道无法读取");
                            let mut guard = state_arc.write().await;
                            guard.error = Some("ada-gateway stdout 管道无法读取".to_string());
                            return;
                        };

                        let gw_reader = BufReader::new(gw_out).lines();
                        let gw_port = match wait_for_ready_line(gw_reader, "A_DA_GATEWAY_READY ", std::time::Duration::from_secs(10), "ada-gateway").await {
                            Some(p) => p,
                            None => {
                                warn!("未在预期时限内收到 ada-gateway 就绪信号");
                                let mut guard = state_arc.write().await;
                                guard.error = Some("未在预期时限内收到 ada-gateway 就绪信号".to_string());
                                return;
                            }
                        };

                        let gateway_endpoint = if token.is_empty() {
                            format!("ws://127.0.0.1:{}/rpc", gw_port)
                        } else {
                            format!("ws://127.0.0.1:{}/rpc?token={}", gw_port, token)
                        };
                        info!("成功连接 ada-gateway 网关服务: {}", gateway_endpoint);
                        let mut guard = state_arc.write().await;
                        guard.alive = true;
                        guard.port = gw_port;
                        guard.token = token.clone();
                        guard.url = gateway_endpoint.clone();
                        guard.child_pid = Some(gw_pid);
                        guard.child_pids = vec![gw_pid];
                        guard.processes = vec![
                            ManagedProcessInfo {
                                name: "ada-gateway".to_string(),
                                role: "统一网关与路由平台".to_string(),
                                pid: Some(gw_pid),
                                port: Some(gw_port),
                                alive: is_pid_alive(gw_pid),
                                status: if is_pid_alive(gw_pid) { "running".to_string() } else { "stopped".to_string() },
                                url: Some(gateway_endpoint),
                            },
                        ];
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_core_info,
            get_desktop_config,
            set_desktop_config,
            restart_desktop_app,
            get_process_report,
            kill_process
        ])
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
            info!("接收到 Tauri 窗口退出事件，正在清理托管子进程...");
            if let Some(state) = app_handle.try_state::<CoreServiceState>() {
                let inner = state.inner.clone();
                tauri::async_runtime::block_on(async move {
                    let mut guard = inner.write().await;
                    guard.alive = false;

                    // 若存在托管的子进程，安全清理所有子进程
                    #[cfg(windows)]
                    {
                        use windows_sys::Win32::System::Threading::{
                            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
                        };
                        let mut all_pids = guard.child_pids.clone();
                        if let Some(p) = guard.child_pid {
                            if !all_pids.contains(&p) {
                                all_pids.push(p);
                            }
                        }
                        for pid in all_pids {
                            if pid > 0 {
                                unsafe {
                                    let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                                    if !handle.is_null() {
                                        info!("退出清理托管子进程 PID: {}", pid);
                                        let _ = TerminateProcess(handle, 0);
                                        windows_sys::Win32::Foundation::CloseHandle(handle);
                                    }
                                }
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

    #[test]
    fn test_desktop_config_defaults() {
        let cfg = DesktopConfig::default();
        assert_eq!(cfg.mode, DesktopMode::Direct);
        assert!(cfg.agent_connect_url.is_none());
        assert!(cfg.gateway_url.is_none());
    }
}
