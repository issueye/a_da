//! a-da 桌面**壳子**（Tauri 宿主）。
//!
//! 与 Electron 宿主（`electron/main.ts`）职责等价，只是换了个容器：
//!
//! 1. **唤醒并监督网关**：探测 `ada-gateway` 二进制 → 拉起子进程 → 读就绪行拿端口 →
//!    崩溃看门狗 → 退出时清理。桌面端自身**不含引擎**（INV-1），它只是网关的瘦客户端。
//! 2. **窗口控制**：最小化 / 最大化 / 还原 / 关闭 / 拖拽 / 最大化状态广播。
//!    🔴 一律走 **Rust 侧自定义命令**（`window_*`），前端不再直接调 `plugin:window|*`——
//!    自定义命令不经过 webview 的 ACL 授权面，壳子能力不该因为一条 capability 漏配就整体失效；
//!    capabilities 里仍把窗口权限补齐，供需要时回退到 JS API。
//! 3. **壳子通用能力**：进程报告 / 终止子进程 / 配置读写 / 重启 / 重启网关 / 单实例 / 开发者工具。
//!
//! 三种启动形态（`tauri dev`、`cargo run`、打包后双击）的**工作目录各不相同**，
//! 所以二进制探测一律按「候选目录集合」查找，绝不依赖 cwd（见 [`binary_search_dirs`]）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 网关就绪行前缀（与 `crates/agent-gateway/src/main.rs::READY_PREFIX` 同源约定）。
const GATEWAY_READY_PREFIX: &str = "A_DA_GATEWAY_READY ";
/// 等待就绪行的上限。网关要现拉起 agent 实例，冷启动比想象中慢，给足 15s。
const GATEWAY_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// 主窗口标签（capabilities / 命令默认作用对象都指向它）。
const MAIN_WINDOW: &str = "main";
/// 窗口最大化状态变化事件名（前端订阅它同步按钮图标）。
pub const EVENT_MAXIMIZE_CHANGE: &str = "window:maximize-change";

/// 桌面端运行模式。
///
/// **只剩网关模式**：直连模式（桌面端自己去拉起 / 连接 `ada-coding`）已下线——
/// Agent 实例的生命周期、注册发现、跨 Agent 委派统一归网关，桌面端只是它的瘦客户端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopMode {
    /// 网关模式（连接 ada-gateway 平台服务）
    Gateway,
}

impl Default for DesktopMode {
    fn default() -> Self {
        DesktopMode::Gateway
    }
}

/// 桌面端运行与连接持久化配置
///
/// 老配置里的 `mode` / `agentConnectUrl` / `agentBinPath`（直连时代留下的字段）
/// 会被 serde **忽略**——字段已删，不再有任何"切回直连"的入口。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
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
            gateway_url: None,
            gateway_bin_path: None,
            token: None,
        }
    }
}

/// 获取桌面端配置文件路径 (~/.a-da/desktop_launcher.json)
pub fn desktop_config_path() -> std::path::PathBuf {
    let app_home = agent_node::session::get_app_home();
    Path::new(&app_home).join("desktop_launcher.json")
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
    let content =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("写入配置文件失败: {e}"))?;
    Ok(())
}

/// 桌面客户端命令行启动参数
#[derive(Debug, Clone, Default)]
pub struct LauncherConfig {
    /// 目标工作区路径
    pub workspace: String,
    /// 覆盖配置的网关 WebSocket 地址（连接外部网关时用）
    pub connect: Option<String>,
    /// 握手认证令牌
    pub token: Option<String>,
    /// 自定义 ada-gateway 二进制路径（覆盖配置文件里的 `gatewayBinPath`）
    pub gateway_bin: Option<String>,
}

/// 启动参数在 Tauri 里的托管形态（`restart_gateway` 要用它重放同一套参数）。
#[derive(Clone)]
pub struct ShellConfigState(pub LauncherConfig);

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
    /// 实际使用的网关二进制路径（诊断用）
    pub gateway_bin: Option<String>,
    /// 网关启动中（用于前端区分"还没起来"与"起不来"）
    pub starting: bool,
}

impl CoreServiceInner {
    fn new(mode: DesktopMode) -> Self {
        Self {
            alive: false,
            mode,
            port: 0,
            token: String::new(),
            url: String::new(),
            child_pid: None,
            child_pids: Vec::new(),
            processes: Vec::new(),
            error: None,
            gateway_bin: None,
            starting: false,
        }
    }
}

/// Tauri Managed State
#[derive(Clone)]
pub struct CoreServiceState {
    pub inner: Arc<RwLock<CoreServiceInner>>,
}

impl CoreServiceState {
    pub fn new(mode: DesktopMode) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CoreServiceInner::new(mode))),
        }
    }
}

impl Default for CoreServiceState {
    fn default() -> Self {
        Self::new(DesktopMode::Gateway)
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

/// 探测二进制的**候选目录集合**。
///
/// 🔴 为什么不用相对路径：`tauri dev`（cwd 可能是仓库根或 `src-tauri`）、
/// `cargo run`（cwd = 仓库根）、打包后双击（cwd = exe 目录或任意目录）三种形态下
/// cwd 都不一样，拿 cwd 拼相对路径必然有一半场景探测不到网关——
/// 那正是"桌面端起来了但网关没被唤醒"的直接原因。
pub fn binary_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if !dirs.iter().any(|x| x == &d) {
            dirs.push(d);
        }
    };

    // 候选 1：宿主可执行文件所在目录（发布包 / 免安装包 / target 目录）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push(dir.to_path_buf());
            push(dir.join("bin"));
            // 与 Electron 打包布局共存：resources/bin 下也放一份二进制
            push(dir.join("resources").join("bin"));
            push(dir.join("resources"));
        }
    }

    // 候选 2：编译期已知的仓库布局（dev 场景；发布机上这些目录不存在，探测自然会跳过）
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo_root) = manifest_dir.parent() {
        push(repo_root.join("dist"));
        push(repo_root.join("target").join("debug"));
        push(repo_root.join("target").join("release"));
        // 本仓 `.cargo/config.toml` 把 target-dir 指到了仓库隔壁的 cargo_target_ada
        if let Some(projects_dir) = repo_root.parent() {
            push(projects_dir.join("cargo_target_ada").join("debug"));
            push(projects_dir.join("cargo_target_ada").join("release"));
            push(projects_dir.join("cargo_target_ada").join("debug").join("bin"));
        }
    }

    // 候选 3：显式环境变量（CI / 自定义 target 目录）
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        let td = PathBuf::from(td);
        push(td.join("debug"));
        push(td.join("release"));
    }

    // 候选 4：当前工作目录及其常见相对位置（兜底，不作为主要依据）
    if let Ok(cwd) = std::env::current_dir() {
        push(cwd.clone());
        push(cwd.join("dist"));
        for rel in [
            "cargo_target_ada/debug",
            "cargo_target_ada/release",
            "target/debug",
            "target/release",
            "../cargo_target_ada/debug",
            "../cargo_target_ada/release",
        ] {
            push(cwd.join(rel));
        }
    }

    dirs
}

/// 通用二进制探测函数：先在自定义路径找，再按候选目录集合查找。
pub fn find_binary(exe_name_base: &str, custom_bin: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(custom) = custom_bin {
        if !custom.trim().is_empty() {
            let p = std::path::PathBuf::from(custom);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{exe_name_base}.exe"), exe_name_base.to_string()]
    } else {
        vec![exe_name_base.to_string()]
    };

    for dir in binary_search_dirs() {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
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

    // 后台协程继续跟进打印输出（网关的日志是排查 agent 实例问题的唯一线索）
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

/// 检查 PID 是否依然存活
#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
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

/// 终止进程（命令与内部清理共用一份实现——单源）
fn kill_pid(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("无效的 PID".to_string());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
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

/// 启动失败时弹一个**原生**错误框。
///
/// 与 Electron 的 `dialog.showErrorBox` 对齐：网关找不到 / 起不来必须让人**看见**，
/// 而不是只在界面上显示一个"未连接"。
#[cfg(windows)]
fn show_error_dialog(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let body = wide(text);
    let caption = wide(title);
    // 放到独立线程里弹：MessageBox 自带消息循环，别把监督协程卡住。
    std::thread::spawn(move || unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    });
}

#[cfg(not(windows))]
fn show_error_dialog(_title: &str, text: &str) {
    eprintln!("{text}");
}

/// 判断是否允许弹原生错误框（自动化验证时用 `A_DA_SILENT_FATAL=1` 静音，避免对话框挡住无人值守的测试）。
fn dialog_allowed() -> bool {
    !matches!(
        std::env::var("A_DA_SILENT_FATAL").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// 唤醒网关：探测二进制 → 拉起 → 等就绪 → 交给看门狗。
///
/// 这是壳子的**第一职责**：桌面端自己不含引擎，网关没起来则整个界面都是死的。
async fn launch_gateway(
    state: Arc<RwLock<CoreServiceInner>>,
    shell: LauncherConfig,
    d_cfg: DesktopConfig,
) {
    // ==========================================
    // 场景 A: 显式指定网关地址（连接外部网关）
    // ==========================================
    if let Some(external_url) = shell.connect.clone().or(d_cfg.gateway_url.clone()) {
        let port = extract_port_from_url(&external_url).unwrap_or(4000);
        let token = shell.token.clone().or(d_cfg.token.clone()).unwrap_or_default();
        info!("连接外部网关服务: {}", external_url);
        let mut guard = state.write().await;
        guard.alive = true;
        guard.starting = false;
        guard.port = port;
        guard.token = token;
        guard.url = external_url.clone();
        guard.error = None;
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

    // ==========================================
    // 场景 B: 探测并拉起独立网关服务 (ada-gateway)
    // ==========================================
    let custom_gw_bin = shell
        .gateway_bin
        .as_deref()
        .or(d_cfg.gateway_bin_path.as_deref());
    let gateway_bin = find_binary("ada-gateway", custom_gw_bin)
        .or_else(|| find_binary("a-da-gateway", custom_gw_bin));

    let Some(gateway_bin) = gateway_bin else {
        let searched = binary_search_dirs()
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ");
        let msg = format!(
            "未找到 ada-gateway 二进制文件，网关无法唤醒。\n\
             请在设置中指定网关地址/二进制路径，或先构建：cargo build -p agent-gateway\n\
             已搜索目录：\n  {searched}"
        );
        error!("{msg}");
        let mut guard = state.write().await;
        guard.alive = false;
        guard.starting = false;
        guard.error = Some(msg.clone());
        drop(guard);
        if dialog_allowed() {
            show_error_dialog("a_da 启动错误", &msg);
        }
        return;
    };

    let token = shell.token.clone().or(d_cfg.token.clone()).unwrap_or_default();
    info!("正在拉起网关程序: {}", gateway_bin.display());

    let mut gw_cmd = tokio::process::Command::new(&gateway_bin);
    gw_cmd.arg("--port").arg("0");
    gw_cmd.arg("--allow-origin").arg("*");
    if !shell.workspace.is_empty() {
        gw_cmd.arg("--workspace").arg(&shell.workspace);
        gw_cmd.current_dir(&shell.workspace);
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
            let msg = format!("启动 ada-gateway 失败: {e}");
            error!("{msg}");
            let mut guard = state.write().await;
            guard.alive = false;
            guard.starting = false;
            guard.error = Some(msg.clone());
            drop(guard);
            if dialog_allowed() {
                show_error_dialog("a_da 启动错误", &msg);
            }
            return;
        }
    };

    let gw_pid = gw_child.id().unwrap_or(0);
    pipe_child_stderr(gw_child.stderr.take(), "ada-gateway");

    let Some(gw_out) = gw_child.stdout.take() else {
        let msg = "ada-gateway stdout 管道无法读取".to_string();
        error!("{msg}");
        let mut guard = state.write().await;
        guard.alive = false;
        guard.starting = false;
        guard.error = Some(msg);
        return;
    };

    // 看门狗：网关活着才有界面。这里立刻把子进程接过来 wait()，
    // 崩了就把状态置为 stopped 并写清楚原因（前端"进程管理"能直接看到）。
    {
        let inner = state.clone();
        tauri::async_runtime::spawn(async move {
            let status = gw_child.wait().await;
            let mut guard = inner.write().await;
            let still_current = guard.child_pid == Some(gw_pid) || guard.child_pids.contains(&gw_pid);
            for p in &mut guard.processes {
                if p.pid == Some(gw_pid) {
                    p.alive = false;
                    p.status = "stopped".to_string();
                }
            }
            if still_current {
                guard.alive = false;
                guard.starting = false;
                guard.error = Some(match &status {
                    Ok(s) => format!("ada-gateway 已退出（{s}）"),
                    Err(e) => format!("ada-gateway 等待失败（{e}）"),
                });
                warn!("ada-gateway 退出: {:?}", status);
            }
        });
    }

    let gw_reader = BufReader::new(gw_out).lines();
    let ready = wait_for_ready_line(
        gw_reader,
        GATEWAY_READY_PREFIX,
        GATEWAY_READY_TIMEOUT,
        "ada-gateway",
    )
    .await;

    let Some(gw_port) = ready else {
        let msg = format!(
            "未在 {} 秒内收到 ada-gateway 就绪信号（PID {}）。请检查 launcher.log 里网关 stdout/stderr。",
            GATEWAY_READY_TIMEOUT.as_secs(),
            gw_pid
        );
        warn!("{msg}");
        let _ = kill_pid(gw_pid);
        let mut guard = state.write().await;
        guard.alive = false;
        guard.starting = false;
        guard.child_pid = None;
        guard.child_pids.clear();
        guard.error = Some(msg.clone());
        drop(guard);
        if dialog_allowed() {
            show_error_dialog("a_da 启动错误", &msg);
        }
        return;
    };

    let gateway_endpoint = if token.is_empty() {
        format!("ws://127.0.0.1:{}/rpc", gw_port)
    } else {
        format!("ws://127.0.0.1:{}/rpc?token={}", gw_port, token)
    };
    info!("成功连接 ada-gateway 网关服务: {}", gateway_endpoint);

    let alive = is_pid_alive(gw_pid);
    let mut guard = state.write().await;
    guard.alive = true;
    guard.starting = false;
    guard.port = gw_port;
    guard.token = token.clone();
    guard.url = gateway_endpoint.clone();
    guard.child_pid = Some(gw_pid);
    guard.child_pids = vec![gw_pid];
    guard.error = None;
    guard.gateway_bin = Some(gateway_bin.display().to_string());
    guard.processes = vec![ManagedProcessInfo {
        name: "ada-gateway".to_string(),
        role: "统一网关与路由平台".to_string(),
        pid: Some(gw_pid),
        port: Some(gw_port),
        alive,
        status: if alive { "running".to_string() } else { "stopped".to_string() },
        url: Some(gateway_endpoint),
    }];
}

// ===========================================================================
// Tauri 命令：连接信息 / 壳子配置 / 进程管理
// ===========================================================================

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
    kill_pid(pid)
}

/// **重新唤醒网关**：杀掉当前托管的网关子进程后按原参数再拉起一次。
///
/// 这是壳子自愈能力的入口：网关进程意外死掉、或用户改了网关配置之后，
/// 界面上的"进程管理"可以直接把它拉回来，而不必重启整个桌面端。
#[tauri::command]
async fn restart_gateway(
    app: tauri::AppHandle,
    state: tauri::State<'_, CoreServiceState>,
) -> Result<ProcessReport, String> {
    let shell = app.state::<ShellConfigState>().0.clone();
    let d_cfg = load_desktop_config();

    let (pids, is_external) = {
        let mut guard = state.inner.write().await;
        let is_external = guard.processes.iter().any(|p| p.pid.is_none());
        let mut pids = guard.child_pids.clone();
        if let Some(p) = guard.child_pid {
            if !pids.contains(&p) {
                pids.push(p);
            }
        }
        guard.alive = false;
        guard.starting = true;
        guard.error = None;
        guard.child_pid = None;
        guard.child_pids.clear();
        guard.processes.clear();
        (pids, is_external)
    };

    if is_external {
        // 外部网关不是我们拉起的，杀了不合适——直接重新握手即可。
        info!("当前为外部网关，重新握手（不杀进程）");
    } else {
        for pid in pids {
            if pid > 0 {
                if let Err(e) = kill_pid(pid) {
                    warn!("重启网关时终止旧进程失败 (PID {pid}): {e}");
                }
            }
        }
    }

    let inner = state.inner.clone();
    let report = {
        let guard = inner.read().await;
        ProcessReport {
            mode: guard.mode,
            core_alive: guard.alive,
            core_url: guard.url.clone(),
            core_port: guard.port,
            error: guard.error.clone(),
            processes: guard.processes.clone(),
        }
    };

    tauri::async_runtime::spawn(async move {
        launch_gateway(inner, shell, d_cfg).await;
    });

    Ok(report)
}

// ===========================================================================
// Tauri 命令：窗口控制
//
// 🔴 全部走**应用自定义命令**：应用命令不经过 webview 的 capability/ACL 授权面，
// 所以窗口按钮不会再因为漏配一条 `core:window:allow-*` 而整体失效。
// capabilities 里依旧把窗口权限补齐，便于需要时回退到 `@tauri-apps/api/window`。
// ===========================================================================

/// 取主窗口
fn main_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow, String> {
    app.get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| format!("主窗口 `{MAIN_WINDOW}` 不存在"))
}

/// 广播最大化状态（前端用它同步"最大化/向下还原"图标）
fn emit_maximize_state(window: &tauri::WebviewWindow) {
    let is_max = window.is_maximized().unwrap_or(false);
    let _ = window.emit(EVENT_MAXIMIZE_CHANGE, is_max);
}

/// 窗口控制：最小化
#[tauri::command]
fn window_minimize(app: tauri::AppHandle) -> Result<(), String> {
    main_window(&app)?
        .minimize()
        .map_err(|e| format!("最小化窗口失败: {e}"))
}

/// 窗口控制：最大化 / 还原切换；返回切换后的最大化状态
#[tauri::command]
fn window_maximize(app: tauri::AppHandle) -> Result<bool, String> {
    let window = main_window(&app)?;
    let is_max = window.is_maximized().map_err(|e| e.to_string())?;
    if is_max {
        window.unmaximize().map_err(|e| format!("还原窗口失败: {e}"))?;
    } else {
        window.maximize().map_err(|e| format!("最大化窗口失败: {e}"))?;
    }
    // 有些平台在尺寸没变时不派发 Resized，这里补一次广播，保证按钮图标不漂移
    emit_maximize_state(&window);
    Ok(window.is_maximized().unwrap_or(!is_max))
}

/// 窗口控制：关闭窗口
#[tauri::command]
fn window_close(app: tauri::AppHandle) -> Result<(), String> {
    main_window(&app)?
        .close()
        .map_err(|e| format!("关闭窗口失败: {e}"))
}

/// 窗口控制：查询当前是否最大化
#[tauri::command]
fn window_is_maximized(app: tauri::AppHandle) -> Result<bool, String> {
    main_window(&app)?
        .is_maximized()
        .map_err(|e| format!("查询窗口状态失败: {e}"))
}

/// 窗口控制：开始拖拽（无边框标题栏）
#[tauri::command]
fn window_start_dragging(app: tauri::AppHandle) -> Result<(), String> {
    main_window(&app)?
        .start_dragging()
        .map_err(|e| format!("拖拽窗口失败: {e}"))
}

/// 窗口控制：切换开发者工具（等价 Electron 的 F12 / Ctrl+Shift+I）
#[tauri::command]
fn toggle_devtools(app: tauri::AppHandle) -> Result<(), String> {
    let window = main_window(&app)?;
    if window.is_devtools_open() {
        window.close_devtools();
    } else {
        window.open_devtools();
    }
    Ok(())
}

// ===========================================================================
// 启动
// ===========================================================================

/// 兼容仅传工作区的启动入口
pub fn run_with_workspace(workspace: String) {
    run(LauncherConfig {
        workspace,
        ..Default::default()
    });
}

/// 创建主窗口（防御 WebView2 缓存锁、多进程冲突与自愈重试）
fn create_main_window(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let Some(win_cfg) = app.config().app.windows.iter().find(|w| w.label == MAIN_WINDOW) else {
        warn!("窗口配置里没有 label = `{MAIN_WINDOW}` 的窗口，跳过创建");
        return Ok(());
    };

    let init_res = tauri::WebviewWindowBuilder::from_config(app.handle(), win_cfg)
        .map_err(|e| format!("解析主窗口配置失败: {e}"))
        .and_then(|b| b.build().map_err(|e| format!("{e}")));

    if let Err(err) = init_res {
        warn!("初次创建 WebView2 主窗口失败: {err}，正在尝试自愈清理并重试...");

        #[cfg(windows)]
        {
            if let Ok(local_data) = app.path().app_local_data_dir() {
                let eb_dir = local_data.join("EBWebView");
                if eb_dir.exists() {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let corrupt_bak = local_data.join(format!("EBWebView_corrupt_{ts}"));
                    if let Err(rename_err) = std::fs::rename(&eb_dir, &corrupt_bak) {
                        warn!("重命名旧 EBWebView 目录失败 ({rename_err})，尝试直接清理...");
                        let _ = std::fs::remove_dir_all(&eb_dir);
                    }
                }
            }
        }

        // 重试创建
        let retry_res = tauri::WebviewWindowBuilder::from_config(app.handle(), win_cfg)
            .map_err(|e| format!("重新解析窗口配置失败: {e}"))
            .and_then(|b| b.build().map_err(|e| format!("{e}")));

        if let Err(retry_err) = retry_res {
            warn!("重试创建主窗口仍失败: {retry_err}，尝试启用备用隔离数据目录...");
            #[cfg(windows)]
            {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                // 防御：若 local_data 目录本身有权限污染（如被管理员提权创建），
                // 备用目录必须退避至系统临时目录，确保属于当前普通用户、拥有完全读写控制权限。
                let fallback_dir =
                    std::env::temp_dir().join(format!("a_da_webview_isolated_{ts}"));
                let _ = std::fs::create_dir_all(&fallback_dir);

                let fallback_builder =
                    tauri::WebviewWindowBuilder::from_config(app.handle(), win_cfg)
                        .map_err(|e| format!("{e}"))?
                        .data_directory(fallback_dir);

                fallback_builder.build().map_err(|e| {
                    format!("创建主窗口失败 (初次: {err}; 重试: {retry_err}; 备用隔离目录: {e})")
                })?;
            }
            #[cfg(not(windows))]
            {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    retry_err,
                )));
            }
        }
    }

    Ok(())
}

/// 退出时清理所有托管的子进程（网关）。
fn cleanup_children(state: &CoreServiceState) {
    let inner = state.inner.clone();
    tauri::async_runtime::block_on(async move {
        let mut guard = inner.write().await;
        guard.alive = false;

        let mut all_pids = guard.child_pids.clone();
        if let Some(p) = guard.child_pid {
            if !all_pids.contains(&p) {
                all_pids.push(p);
            }
        }
        for p in &mut guard.processes {
            if let Some(pid) = p.pid {
                if !all_pids.contains(&pid) {
                    all_pids.push(pid);
                }
            }
        }
        drop(guard);

        for pid in all_pids {
            if pid > 0 {
                info!("退出清理托管子进程 PID: {}", pid);
                let _ = kill_pid(pid);
            }
        }
    });
}

/// 启动 Tauri GUI 桌面应用（**只有网关模式**）
pub fn run(config: LauncherConfig) {
    let desktop_config = load_desktop_config();
    let effective_mode = DesktopMode::Gateway;

    let core_state = CoreServiceState::new(effective_mode);
    let inner_clone = core_state.inner.clone();

    let app_res = tauri::Builder::default()
        .manage(core_state)
        .manage(ShellConfigState(config.clone()))
        .setup(move |app| {
            // 1. 主窗口
            create_main_window(app)?;

            // 2. 唤醒网关（异步监督，不阻塞窗口显示）
            let state_arc = inner_clone.clone();
            let cfg = config.clone();
            let d_cfg = desktop_config.clone();
            info!("桌面端启动:【网关模式】");
            tauri::async_runtime::spawn(async move {
                {
                    let mut guard = state_arc.write().await;
                    guard.starting = true;
                    guard.mode = effective_mode;
                }
                launch_gateway(state_arc, cfg, d_cfg).await;
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW {
                return;
            }
            match event {
                tauri::WindowEvent::Resized(_) => {
                    if let Ok(is_max) = window.is_maximized() {
                        let _ = window.emit(EVENT_MAXIMIZE_CHANGE, is_max);
                    }
                }
                tauri::WindowEvent::CloseRequested { .. } => {
                    info!("主窗口收到关闭请求");
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_core_info,
            get_desktop_config,
            set_desktop_config,
            restart_desktop_app,
            get_process_report,
            kill_process,
            restart_gateway,
            window_minimize,
            window_maximize,
            window_close,
            window_is_maximized,
            window_start_dragging,
            toggle_devtools
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
                cleanup_children(&state);
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
        // 直连时代的字段（mode / agentConnectUrl / agentBinPath）已删除，
        // 配置里只剩网关相关项；老配置文件里的这些字段会被 serde 忽略。
        assert!(cfg.gateway_url.is_none());
        assert!(cfg.gateway_bin_path.is_none());
        assert!(cfg.token.is_none());
    }

    /// 探测目录必须**包含编译期已知的仓库布局**，否则 `tauri dev`（cwd 可能是 src-tauri）
    /// 会找不到网关——那正是"没有唤醒网关"的根因。
    #[test]
    fn test_binary_search_dirs_covers_dev_layout() {
        let dirs = binary_search_dirs();
        assert!(!dirs.is_empty(), "候选目录集合不该为空");
        assert!(
            dirs.iter().all(|d| d.is_absolute()),
            "候选目录必须是绝对路径（不得依赖 cwd 解析）"
        );

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.parent().expect("src-tauri 必须有父目录");
        assert!(
            dirs.iter().any(|d| d == &repo_root.join("dist")),
            "应包含仓库根 dist/（发布交付套件目录）"
        );
        assert!(
            dirs.iter().any(|d| d == &repo_root.join("target").join("debug")),
            "应包含 target/debug（cargo 默认 target 目录）"
        );
        assert!(
            dirs.iter().any(|d| d.ends_with("cargo_target_ada/debug")),
            "应包含 .cargo/config.toml 里配置的 target-dir/debug"
        );
    }

    /// 自定义路径优先；不存在的自定义路径要**回落到探测**而不是直接失败。
    #[test]
    fn test_find_binary_respects_custom_then_probes() {
        let me = std::env::current_exe().expect("当前可执行文件");
        let found = find_binary("ada-tauri", Some(me.to_string_lossy().as_ref()));
        assert_eq!(found.as_deref(), Some(me.as_path()), "自定义路径应当优先命中");

        // 自定义路径不存在时，结果必须与「纯探测」完全一致（即真的回落了，而不是直接失败）
        let bogus = std::env::temp_dir().join("a-da-not-exist-9f3c1b.exe");
        assert!(!bogus.is_file());
        let probed = find_binary("ada-tauri", None);
        let fallback = find_binary("ada-tauri", Some(bogus.to_string_lossy().as_ref()));
        assert_eq!(probed, fallback, "不存在的自定义路径应当回落到目录探测");

        assert!(
            find_binary("definitely-not-a-real-binary-9f3c1b", None).is_none(),
            "不存在的名字不该被探测到"
        );
    }

    /// 渲染期不该出现"启动中却报错"的状态组合。
    #[test]
    fn test_core_state_defaults() {
        let state = CoreServiceState::new(DesktopMode::Gateway);
        let inner = state.inner.blocking_read();
        assert!(!inner.alive);
        assert!(!inner.starting);
        assert_eq!(inner.port, 0);
        assert!(inner.error.is_none());
        assert!(inner.processes.is_empty());
    }
}
