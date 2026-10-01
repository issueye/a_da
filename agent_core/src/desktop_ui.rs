//! 桌面 UI 宿主模式启动器
//!
//! 负责在原生 Rust 统一主入口下派生自身无头 Host 子进程，
//! 并通过 Meta Hermes 引擎加载 React 19 UI Bundle，驱动 GPUI 窗口加速渲染。

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tracing::info;

use crate::hermes_host::HermesHost;

/// 内嵌的 React 19 紧凑 UI Bundle (1.0MB)
const EMBEDDED_UI_CJS: &str = include_str!("../../dist/ui.cjs");

pub fn run_desktop_mode(workspace: &str) -> Result<(), anyhow::Error> {
    info!("正在启动 a_da 原生桌面 UI 模式 (基于 Meta Hermes 引擎)...");

    // 1. 生成一次性握手令牌
    let token = uuid::Uuid::new_v4().simple().to_string();
    let current_exe = std::env::current_exe()?;
    let current_pid = std::process::id();

    // 2. 派生自身作为无头 Host 后端子进程
    let mut child = Command::new(&current_exe)
        .arg("--host")
        .arg("--port")
        .arg("0")
        .arg("--token")
        .arg(&token)
        .arg("--parent-pid")
        .arg(current_pid.to_string())
        .arg("--workspace")
        .arg(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    // 3. 读取子进程的 stdout 就绪行以获得真实动态分配端口
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("无法打开子进程 stdout"))?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut host_port = 0u16;

    while reader.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.starts_with("A_DA_HOST_READY") {
            let json_str = trimmed.trim_start_matches("A_DA_HOST_READY").trim();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(p) = v.get("port").and_then(|p| p.as_u64()) {
                    host_port = p as u16;
                    break;
                }
            }
        }
        line.clear();
    }

    if host_port == 0 {
        return Err(anyhow::anyhow!("未能在规定时间内收到原生后端的就绪行"));
    }

    // 后台持续读取并消费子进程 stdout，防止管道破裂导致子进程 broken pipe 闪退
    std::thread::spawn(move || {
        let mut sink_line = String::new();
        while let Ok(n) = reader.read_line(&mut sink_line) {
            if n == 0 {
                break;
            }
            sink_line.clear();
        }
    });

    info!("原生 Host 子进程已就绪，端口: {}", host_port);

    // 4. 初始化 Meta Hermes 虚拟机
    let hermes = HermesHost::new().map_err(|e| anyhow::anyhow!(e))?;

    // 5. 查找并动态加载 @gpuix/native 原生扩展
    let native_path = find_gpuix_native_path()?;
    info!("正在加载 GPUIX 原生渲染器模块: {}", native_path.display());
    hermes
        .load_native_addon(native_path.to_str().unwrap(), "__GPUIX_NATIVE")
        .map_err(|e| anyhow::anyhow!(e))?;

    // 6. 注入完整的 Node 宿主上下文与原生 WebSocket 桥接
    hermes
        .inject_node_shims(workspace, host_port, &token)
        .map_err(|e| anyhow::anyhow!(e))?;

    // 7. 执行内嵌的 React 19 UI Bundle
    eprintln!(
        "正在通过 Hermes 虚拟机执行内嵌 React UI Bundle ({} 字节)...",
        EMBEDDED_UI_CJS.len()
    );
    match hermes.eval_to_string(EMBEDDED_UI_CJS) {
        Ok(res) => eprintln!("React 19 UI Bundle 初始化完成: {}", res),
        Err(e) => {
            eprintln!("执行 UI 脚本失败: {}", e);
            let _ = child.kill();
            return Err(anyhow::anyhow!("UI 脚本执行失败: {}", e));
        }
    }

    eprintln!("UI 界面已挂载，进入宿主事件循环泵 (驱动 GPUI 帧循环)...");

    // 8. 驱动事件循环（微任务、宏任务定时器、GPUI 帧循环）
    while let Ok(None) = child.try_wait() {
        match hermes.pump_event_loop_step() {
            Ok(wait_ms) => {
                if wait_ms < 0 {
                    // 无待办定时器，休眠 16ms 等待原生网络或 UI 事件
                    std::thread::sleep(std::time::Duration::from_millis(16));
                } else if wait_ms == 0 {
                    // 有就绪任务，让出时间片
                    std::thread::yield_now();
                } else {
                    let sleep_dur = std::cmp::min(wait_ms as u64, 16);
                    std::thread::sleep(std::time::Duration::from_millis(sleep_dur));
                }
            }
            Err(e) => {
                eprintln!("[UI EventLoop 异常] {}", e);
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }

    let status = child.wait()?;
    eprintln!("子进程已退出，状态码: {:?}", status.code());

    Ok(())
}

fn find_gpuix_native_path() -> Result<std::path::PathBuf, anyhow::Error> {
    let current_exe = std::env::current_exe()?;
    let exe_dir = current_exe.parent().unwrap_or(std::path::Path::new("."));

    let candidates = [
        exe_dir.join("gpuix-native.win32-x64-msvc.node"),
        exe_dir.join("gpuix-native.node"),
        exe_dir
            .join("node_modules")
            .join("@gpuix")
            .join("native")
            .join("gpuix-native.win32-x64-msvc.node"),
        std::path::PathBuf::from(
            r"E:\codes\rust_projects\a_da\node_modules\@gpuix\native\gpuix-native.win32-x64-msvc.node",
        ),
        std::path::PathBuf::from(
            r"E:\codes\rust_projects\gpuix\packages\native\gpuix-native.win32-x64-msvc.node",
        ),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err(anyhow::anyhow!(
        "未找到 GPUIX 原生扩展文件 (gpuix-native.win32-x64-msvc.node)"
    ))
}
