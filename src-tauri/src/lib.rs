use agent_core::server::WsHostServer;
use agent_core::state::AgentStore;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;
use tracing::{error, info};

/// 同进程核心服务状态内部数据
pub struct CoreServiceInner {
    pub alive: bool,
    pub port: u16,
    pub token: String,
    pub server: Option<Arc<WsHostServer>>,
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
                server: None,
            })),
        }
    }
}

impl Default for CoreServiceState {
    fn default() -> Self {
        Self::new()
    }
}

/// 获取同进程 Core 服务的连接信息
#[tauri::command]
async fn get_core_info(
    state: tauri::State<'_, CoreServiceState>,
) -> Result<serde_json::Value, String> {
    // 轮询等待核心服务完成本地端口绑定（最多等待 5 秒）
    for _ in 0..50 {
        let guard = state.inner.read().await;
        if guard.alive && guard.port > 0 {
            return Ok(serde_json::json!({
                "alive": true,
                "port": guard.port,
                "token": guard.token,
                "url": format!("ws://127.0.0.1:{}/rpc", guard.port)
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
        "url": if guard.port > 0 {
            format!("ws://127.0.0.1:{}/rpc", guard.port)
        } else {
            "ws://127.0.0.1:52353/rpc".to_string()
        }
    }))
}

/// 启动 Tauri GUI 桌面应用
///
/// `workspace` 为命令行显式指定的工作区；为空时交给 `AgentStore` 决定
/// （环境变量 → 最近一次会话的工作区 → 公共区）。**刻意不使用进程当前目录**：
/// 双击 `target/release` 里的可执行文件会让 cwd 变成构建产物目录，
/// 那只是"从哪启动"，不代表用户的项目在哪。
pub fn run(workspace: String) {
    let core_state = CoreServiceState::new();
    let inner_clone = core_state.inner.clone();

    tauri::Builder::default()
        .manage(core_state)
        .setup(move |_app| {
            // 利用同进程异步任务启动 agent_core::WsHostServer::bind(0, token, store)
            // 端口传 0 由操作系统分配空闲端口，消除端口冲突
            let state_arc = inner_clone.clone();
            tauri::async_runtime::spawn(async move {
                let token = uuid::Uuid::new_v4().to_string();
                let store = Arc::new(RwLock::new(AgentStore::new(workspace)));

                let resolved_ws = store.read().await.workspace.project.clone();
                info!("正在启动 a-da 同进程核心服务 (动态端口绑定)... 工作区: {}", resolved_ws);
                match WsHostServer::bind(0, token.clone(), store).await {
                    Ok(server) => {
                        let actual_port = server.port;
                        info!(
                            "a-da 同进程核心服务启动成功: ws://127.0.0.1:{}/rpc (端口: {})",
                            actual_port, actual_port
                        );
                        let mut guard = state_arc.write().await;
                        guard.alive = true;
                        guard.port = actual_port;
                        guard.token = token;
                        guard.server = Some(server);
                    }
                    Err(e) => {
                        error!("启动 a-da 同进程核心服务失败: {}", e);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_core_info])
        .build(tauri::generate_context!())
        .expect("运行 Tauri 桌面应用失败")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                info!("接收到 Tauri 窗口退出事件，正在清理同进程核心服务资源...");
                if let Some(state) = app_handle.try_state::<CoreServiceState>() {
                    let inner = state.inner.clone();
                    tauri::async_runtime::block_on(async move {
                        let mut guard = inner.write().await;
                        guard.alive = false;
                        guard.server = None;
                    });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
