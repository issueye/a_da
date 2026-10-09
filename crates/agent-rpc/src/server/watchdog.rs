use std::time::Duration;
use tracing::{info, warn};

/// 启动父进程看门狗（如果提供了 parent_pid）
pub fn start_parent_watchdog(parent_pid: u32, interval_ms: u64) {
    tokio::spawn(async move {
        info!("父进程看门狗已启动，监控 PID: {}", parent_pid);
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            interval.tick().await;
            if !is_process_alive(parent_pid) {
                warn!("检测到父进程 (PID: {}) 已退出，看门狗触发主机安全退出", parent_pid);
                std::process::exit(0);
            }
        }
    });
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut std::ffi::c_void;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
        }

        // SYNCHRONIZE (0x00100000)
        let handle = OpenProcess(0x00100000, 0, pid);
        if handle.is_null() {
            return false;
        }
        // WAIT_TIMEOUT = 258 表示进程尚未退出，仍处于运行态
        let wait_res = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        wait_res == 258
    }
}

#[cfg(not(windows))]
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
