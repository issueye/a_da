//! **AGENT 生命周期**：拉起、等就绪、登记、回收。
//!
//! # 这段逻辑原先住在 `src-tauri`（UI 壳）里
//!
//! `src-tauri/src/lib.rs` 原先自己 `find_ada_coding_binary` → spawn → 解析
//! `A_DA_HOST_READY {port}` → 记 `{port, token, url, child_pid}`。也就是说
//! **UI 壳在当 agent 的进程 supervisor**——职责放错了位置，而且这就是为什么
//! "远程接入"做不到（关掉界面就把 agent 带走了）。
//!
//! S5 把它搬到这里：网关是服务，agent 实例的生命周期归网关。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{info, warn};

use crate::registry::{AgentInstance, AgentRegistry, AgentStatus, normalize_workspace};

/// 等待就绪信号的默认上限。
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// 就绪行前缀（协议 §1.8：`A_DA_HOST_READY {"ready":true,"port":…}`）。
pub const READY_PREFIX: &str = "A_DA_HOST_READY ";

/// 拉起失败的原因。**每种都如实区分**——"起不来"和"起来了但没就绪"是两回事。
#[derive(Debug)]
pub enum SpawnError {
    /// 找不到产品二进制
    BinaryNotFound { tried: Vec<String> },
    /// 进程起不来
    Spawn(std::io::Error),
    /// 进程起来了但没在时限内给出就绪行
    NotReady { waited: Duration, stderr_tail: String },
    /// 就绪行格式不对（有前缀但 JSON 解析不了）
    BadReadyLine(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinaryNotFound { tried } => write!(
                f,
                "找不到产品二进制；已尝试：{}",
                tried.join(", ")
            ),
            Self::Spawn(e) => write!(f, "拉起进程失败：{e}"),
            Self::NotReady { waited, stderr_tail } => write!(
                f,
                "进程已启动但 {:?} 内未就绪；stderr 末尾：{}",
                waited,
                if stderr_tail.is_empty() { "（空）" } else { stderr_tail.as_str() }
            ),
            Self::BadReadyLine(line) => write!(f, "就绪行无法解析：{line}"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// 产品二进制的候选名（按平台）。
fn binary_candidates(product: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{product}.exe"), product.to_string()]
    } else {
        vec![product.to_string()]
    }
}

/// 在若干候选目录里找产品二进制。
///
/// 查找顺序：显式指定 → 与当前可执行文件同目录（打包后的形态）→
/// `target/{debug,release}`（开发形态）。**不读环境变量**——那是组合根的事。
pub fn find_product_binary(product: &str, explicit: Option<&Path>) -> Result<PathBuf, SpawnError> {
    let names = binary_candidates(product);
    let mut tried: Vec<String> = Vec::new();

    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        tried.push(p.display().to_string());
    }

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    // 环境变量与本地复用缓存路径
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        dirs.push(Path::new(&td).join("debug"));
        dirs.push(Path::new(&td).join("release"));
    }
    dirs.push(PathBuf::from("../cargo_target_ada/debug"));
    dirs.push(PathBuf::from("../cargo_target_ada/release"));

    // 开发形态：网关自己通常跑在 target/debug 下
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("target/debug"));
        dirs.push(cwd.join("target/release"));
    }

    for dir in dirs {
        for name in &names {
            let cand = dir.join(name);
            if cand.exists() {
                return Ok(cand);
            }
            tried.push(cand.display().to_string());
        }
    }
    Err(SpawnError::BinaryNotFound { tried })
}

/// 拉起参数。
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub product: String,
    pub workspace: PathBuf,
    /// 显式指定二进制路径（`None` = 自动查找）
    pub binary: Option<PathBuf>,
    /// 就绪等待上限
    pub ready_timeout: Duration,
}

impl SpawnSpec {
    pub fn new(product: impl Into<String>, workspace: impl AsRef<Path>) -> Self {
        Self {
            product: product.into(),
            workspace: workspace.as_ref().to_path_buf(),
            binary: None,
            ready_timeout: DEFAULT_READY_TIMEOUT,
        }
    }

    pub fn with_binary(mut self, b: impl Into<PathBuf>) -> Self {
        self.binary = Some(b.into());
        self
    }

    pub fn with_ready_timeout(mut self, d: Duration) -> Self {
        self.ready_timeout = d;
        self
    }
}

/// 一个已拉起的实例（进程句柄 + 登记信息）。
pub struct SpawnedAgent {
    pub instance: AgentInstance,
    /// 子进程句柄。**持有它**：drop 时若没被 kill，进程会变孤儿。
    pub child: Child,
    /// 就绪行的原始 JSON（诊断用）
    pub ready: serde_json::Value,
}

impl std::fmt::Debug for SpawnedAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnedAgent")
            .field("instance", &self.instance)
            .field("ready", &self.ready)
            .finish_non_exhaustive()
    }
}

/// 拉起一个 agent 实例并等它就绪，然后登记到注册表。
///
/// 顺序刻意是「先起进程 → 等就绪 → **再登记**」：登记进去的实例必须已经是可接流量的，
/// 否则会出现"注册表里有、连不上"的窗口（那正是路由到黑洞的成因）。
pub async fn spawn_agent(
    spec: &SpawnSpec,
    registry: &Arc<AgentRegistry>,
    gateway_pid: u32,
) -> Result<SpawnedAgent, SpawnError> {
    let workspace = normalize_workspace(&spec.workspace.to_string_lossy());
    let id = AgentRegistry::id_for_workspace(&spec.product, &workspace);
    let token = uuid::Uuid::new_v4().simple().to_string();

    let binary = find_product_binary(&spec.product, spec.binary.as_deref())?;
    info!("网关拉起 agent 实例：{} ({}), 工作区 {}", id, binary.display(), workspace.display());

    let mut cmd = Command::new(&binary);
    cmd.arg("--host")
        .arg("--port")
        .arg("0")
        .arg("--token")
        .arg(&token)
        .arg("--workspace")
        .arg(workspace.to_string_lossy().to_string())
        // 看门狗：网关死了，agent 不该变孤儿
        .arg("--parent-pid")
        .arg(gateway_pid.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_child_console_window(&mut cmd);

    let mut child = cmd.spawn().map_err(SpawnError::Spawn)?;
    let pid = child.id();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // stderr 后台收集（就绪失败时把末尾贴出来——不然"起不来"没有任何线索）
    let stderr_tail = Arc::new(tokio::sync::Mutex::new(String::new()));
    if let Some(err) = stderr {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock().await;
                t.push_str(&line);
                t.push('\n');
                // 只留末尾若干行，避免长跑进程把内存吃光
                if t.len() > 8192 {
                    let cut = t.len() - 4096;
                    *t = t[cut..].to_string();
                }
            }
        });
    }

    let Some(out) = stdout else {
        let _ = child.kill().await;
        return Err(SpawnError::NotReady {
            waited: Duration::ZERO,
            stderr_tail: "（stdout 不可读）".to_string(),
        });
    };

    // 等就绪行
    let ready_result = tokio::time::timeout(spec.ready_timeout, async {
        let mut lines = BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(json_str) = line.strip_prefix(READY_PREFIX) {
                return Some(json_str.to_string());
            }
            // 其它 stdout 行只作日志，不当就绪信号
            info!("[{} stdout] {}", spec.product, line);
        }
        None
    })
    .await;

    let ready_line = match ready_result {
        Ok(Some(l)) => l,
        Ok(None) => {
            let _ = child.kill().await;
            return Err(SpawnError::NotReady {
                waited: spec.ready_timeout,
                stderr_tail: stderr_tail.lock().await.clone(),
            });
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(SpawnError::NotReady {
                waited: spec.ready_timeout,
                stderr_tail: stderr_tail.lock().await.clone(),
            });
        }
    };

    let ready: serde_json::Value = serde_json::from_str(&ready_line)
        .map_err(|_| SpawnError::BadReadyLine(ready_line.clone()))?;

    let port = ready
        .get("port")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SpawnError::BadReadyLine(ready_line.clone()))? as u16;

    let instance = AgentInstance {
        id,
        product: spec.product.clone(),
        workspace: workspace.to_string_lossy().to_string(),
        endpoint: format!("ws://127.0.0.1:{port}/rpc?token={token}"),
        status: AgentStatus::Ready,
        pid,
        started_at: now_ms(),
    };
    registry.register(instance.clone());

    info!("agent 实例就绪：{} → {} (pid {:?})", instance.id, instance.endpoint, pid);
    Ok(SpawnedAgent { instance, child, ready })
}

/// 保证某个工作区有一个**可路由**的实例：有就复用，没有就拉起。
///
/// 并发安全：调用方（网关的连接处理）会为每个客户端调用它，
/// 所以"查 + 起"必须是原子的——否则两个客户端同时连会拉起两个实例，
/// 同一工作区出现两份状态互相覆盖。这里用一把异步锁把整段串起来。
pub async fn ensure_agent(
    spec: &SpawnSpec,
    registry: &Arc<AgentRegistry>,
    spawn_lock: &Arc<tokio::sync::Mutex<()>>,
    gateway_pid: u32,
) -> Result<AgentInstance, SpawnError> {
    let workspace = normalize_workspace(&spec.workspace.to_string_lossy());
    if let Some(existing) = registry.find_by_workspace(&spec.product, &workspace) {
        if existing.status.is_routable() {
            return Ok(existing);
        }
    }

    let _guard = spawn_lock.lock().await;

    // 双检：等锁期间可能已经有人起好了
    if let Some(existing) = registry.find_by_workspace(&spec.product, &workspace) {
        if existing.status.is_routable() {
            return Ok(existing);
        }
    }

    let spawned = spawn_agent(spec, registry, gateway_pid).await?;
    // 子进程句柄交给守护协程，drop 掉会让进程成孤儿
    let SpawnedAgent { instance, mut child, .. } = spawned;
    let id = instance.id.clone();
    let reg = registry.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        warn!("agent 实例 {} 已退出：{:?}", id, status);
        reg.set_status(&id, AgentStatus::Stopped);
    });
    Ok(instance)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Windows 下隐藏子进程控制台窗口。
///
/// 与 `src-tauri` 里那份同源（那条修复的注释见 git 历史：不加这个会闪一个黑窗）。
/// `creation_flags` 是 tokio `Command` 在 Windows 上的**固有方法**，不需要引入 trait。
#[cfg(target_os = "windows")]
fn hide_child_console_window(cmd: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_child_console_window(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_not_found_lists_what_was_tried() {
        let err = find_product_binary("definitely-not-a-real-binary-xyz", None)
            .expect_err("不该找到");
        match err {
            SpawnError::BinaryNotFound { tried } => {
                assert!(!tried.is_empty(), "必须列出尝试过的路径（否则无从排查）");
            }
            other => panic!("应报 BinaryNotFound，实际：{other}"),
        }
    }

    /// 就绪超时必须**如实报错**，并且把 stderr 末尾带上——不许静默返回一个假实例。
    #[tokio::test]
    async fn test_spawn_reports_not_ready_with_stderr_tail() {
        // 用一个必定存在、但不会打印就绪行的程序：本测试进程自己的可执行文件
        let exe = std::env::current_exe().expect("测试进程自身路径");
        let ws = std::env::temp_dir().join("a_da_gw_spawn_probe");
        let spec = SpawnSpec::new("a-da-gateway-probe", &ws)
            .with_binary(exe)
            .with_ready_timeout(Duration::from_millis(300));

        let reg = Arc::new(AgentRegistry::new());
        let err = spawn_agent(&spec, &reg, std::process::id())
            .await
            .expect_err("不该就绪");

        match err {
            SpawnError::NotReady { waited, .. } => {
                assert_eq!(waited, Duration::from_millis(300));
            }
            other => panic!("应报 NotReady，实际：{other}"),
        }
        assert!(
            reg.is_empty(),
            "未就绪的实例**绝不能**进注册表——否则路由会打到黑洞上"
        );
    }
}
