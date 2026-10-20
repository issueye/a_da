//! **AGENT 生命周期**：拉起、等就绪、登记、回收。
//!
//! # 这段逻辑原先住在桌面壳（当时的 Tauri 宿主）里
//!
//! 桌面壳原先自己 `find_ada_coding_binary` → spawn → 解析
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

/// 网关 ↔ agent 的**链路形态**。
///
/// 环境变量 `A_DA_GATEWAY_AGENT_LINK`：`tcp`（默认，回环端口）/ `pipe`（本地管道）。
/// 默认保持 TCP：管道形态还在灰度，默认值以"行为不变"为先。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLinkMode {
    Tcp,
    Pipe,
}

impl AgentLinkMode {
    pub fn from_env() -> Self {
        match std::env::var("A_DA_GATEWAY_AGENT_LINK")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "pipe" | "pipes" | "named-pipe" | "unix" => Self::Pipe,
            _ => Self::Tcp,
        }
    }
}

/// 本次进程唯一的管道名（Windows）或 socket 路径（Unix）。
pub fn agent_pipe_name() -> String {
    let id = uuid::Uuid::new_v4().simple();
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\a-da-agent-{id}")
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("a-da-agent-{id}.sock"))
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(any(windows, unix)))]
    {
        format!("a-da-agent-{id}")
    }
}

/// 把管道名包装成注册表里的端点字符串。
#[cfg(windows)]
fn pipe_endpoint_url(name: &str) -> String {
    format!("pipe://{name}")
}

#[cfg(unix)]
fn pipe_endpoint_url(name: &str) -> String {
    format!("unix://{name}")
}

#[cfg(not(any(windows, unix)))]
fn pipe_endpoint_url(name: &str) -> String {
    format!("pipe://{name}")
}

/// 拉起失败的原因。**每种都如实区分**——"起不来"和"起来了但没就绪"是两回事。
#[derive(Debug)]
pub enum SpawnError {
    /// 找不到产品二进制
    BinaryNotFound { tried: Vec<String> },
    /// 工作区目录不存在（拒绝为一个空气目录起进程）
    WorkspaceMissing { workspace: String },
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
            Self::WorkspaceMissing { workspace } => write!(
                f,
                "工作区目录不存在：{workspace}（拒绝为一个空气目录拉起 agent——那只会白占一个进程）"
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

    let link_mode = AgentLinkMode::from_env();
    let pipe_name = (link_mode == AgentLinkMode::Pipe).then(agent_pipe_name);

    let mut cmd = Command::new(&binary);
    cmd.arg("--host");
    match &pipe_name {
        // 管道形态：不占端口、不监听——连接方只能拿到这个管道名
        Some(name) => {
            cmd.arg("--pipe").arg(name);
        }
        None => {
            cmd.arg("--port").arg("0");
        }
    }
    cmd.arg("--token")
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

    // 端点形态由就绪行决定：管道形态带 `pipe`（Windows 管道名 / Unix socket 路径），
    // 否则必须给出 `port`。两种形态都拼成注册表里同一种端点字符串。
    let pipe = ready
        .get("pipe")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let endpoint = match &pipe {
        Some(name) => format!("{}?token={}", pipe_endpoint_url(name), token),
        None => {
            let port = ready
                .get("port")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| SpawnError::BadReadyLine(ready_line.clone()))?
                as u16;
            format!("ws://127.0.0.1:{port}/rpc?token={token}")
        }
    };

    // 实例自述服务的产品清单（老实例不带这个字段 → 退回"它自己那个产品"）
    let products: Vec<String> = ready
        .get("products")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        })
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| vec![spec.product.clone()]);

    let instance = AgentInstance {
        id,
        product: spec.product.clone(),
        workspace: workspace.to_string_lossy().to_string(),
        endpoint,
        status: AgentStatus::Ready,
        pid,
        started_at: now_ms(),
        products,
        last_active_ms: now_ms(),
    };
    registry.register(instance.clone());

    // 日志里**不带 query**：端点字符串里有 token，日志会落盘。
    let shown = instance
        .endpoint
        .split('?')
        .next()
        .unwrap_or(&instance.endpoint);
    info!("agent 实例就绪：{} → {} (pid {:?})", instance.id, shown, pid);
    Ok(SpawnedAgent { instance, child, ready })
}

/// 保证某个工作区有一个**可路由**的实例：有就复用，没有就拉起。
///
/// 并发安全：调用方（网关的连接处理）会为每个客户端调用它，
/// 所以"查 + 起"必须是原子的——否则两个客户端同时连会拉起两个实例，
/// 同一工作区出现两份状态互相覆盖（而且进程数翻倍）。这里用一把异步锁把整段串起来。
///
/// **复用**（同一工作区已有可路由实例）时只 `touch` 一下活跃时间：一个工作区下的
/// 多个会话/多个客户端共用**同一个进程**，这是"进程数可控"的前提。
pub async fn ensure_agent(
    spec: &SpawnSpec,
    registry: &Arc<AgentRegistry>,
    spawn_lock: &Arc<tokio::sync::Mutex<()>>,
    gateway_pid: u32,
) -> Result<AgentInstance, SpawnError> {
    let workspace = normalize_workspace(&spec.workspace.to_string_lossy());
    if let Some(existing) = registry.find_by_workspace(&spec.product, &workspace) {
        if existing.status.is_routable() {
            registry.touch(&existing.id);
            return Ok(existing);
        }
    }

    let _guard = spawn_lock.lock().await;

    // 双检：等锁期间可能已经有人起好了
    if let Some(existing) = registry.find_by_workspace(&spec.product, &workspace) {
        if existing.status.is_routable() {
            registry.touch(&existing.id);
            return Ok(existing);
        }
    }

    // 🔴 工作区必须真实存在：为一个不存在的目录（测试残留、已删的项目、离线盘符）
    // 拉起 agent，只会白占一个进程，而且它的文件工具全都会失败。
    if !workspace.is_dir() {
        return Err(SpawnError::WorkspaceMissing {
            workspace: workspace.to_string_lossy().to_string(),
        });
    }

    let spawned = spawn_agent(spec, registry, gateway_pid).await?;
    let SpawnedAgent { instance, mut child, .. } = spawned;
    let id = instance.id.clone();

    // 守护协程：子进程句柄归它。两条路径——
    //   · 进程自己退出 → 标 Stopped（由后台清理摘掉）
    //   · 收到"请求关停"（空闲回收 / 超上限驱逐 / 网关退出）→ kill 掉并摘出注册表
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    registry.attach_shutdown(&id, shutdown_tx);
    let reg = registry.clone();
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                warn!("agent 实例 {} 已退出：{:?}", id, status);
                reg.set_status(&id, AgentStatus::Stopped);
            }
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    info!("网关已回收 agent 实例 {}（空闲/被驱逐/退出清理）", id);
                    reg.remove(&id);
                }
            }
        }
    });
    Ok(instance)
}

/// 网关的实例回收策略。
///
/// 目的只有一个：**进程数可控**。没有回收时，只要访问过 N 个工作区，
/// 就会一直挂着 N 个 `ada-coding` 进程（每个都是几十 MB 常驻）。
#[derive(Debug, Clone, Copy)]
pub struct ReapPolicy {
    /// 空闲多久就回收（默认 15 分钟没有任何帧流量）
    pub idle_after: Duration,
    /// 同时在跑的实例上限（超过就按 LRU 驱逐）
    pub max_live: usize,
    /// 驱逐时至少空闲多久（别掐掉正在干活的那条）
    pub min_idle_before_evict: Duration,
    /// 扫描间隔
    pub interval: Duration,
}

impl Default for ReapPolicy {
    fn default() -> Self {
        Self {
            idle_after: Duration::from_secs(15 * 60),
            max_live: 6,
            min_idle_before_evict: Duration::from_secs(60),
            interval: Duration::from_secs(60),
        }
    }
}

/// 按策略扫一遍：清理已退出条目 → 回收空闲实例 → 超上限按 LRU 驱逐。
///
/// 返回 `(清理的死条目, 回收的空闲实例, 驱逐的超限实例)`，便于日志与测试断言。
pub fn reap_once(registry: &AgentRegistry, policy: &ReapPolicy) -> (usize, usize, usize) {
    let pruned = registry.prune_stopped();
    let now = now_ms();
    let idle_ms = policy.idle_after.as_millis() as i64;

    let mut reaped = 0;
    for id in registry.idle_since(now, idle_ms) {
        if registry.request_shutdown(&id) {
            info!("回收空闲 agent 实例 {id}（空闲超过 {:?}）", policy.idle_after);
            reaped += 1;
        }
    }

    let min_idle = policy.min_idle_before_evict.as_millis() as i64;
    let mut evicted = 0;
    for id in registry.lru_over_capacity(now, policy.max_live, min_idle) {
        if registry.request_shutdown(&id) {
            info!(
                "驱逐 agent 实例 {id}（在跑实例超过上限 {}）",
                policy.max_live
            );
            evicted += 1;
        }
    }

    (pruned, reaped, evicted)
}

/// 当前 Unix 毫秒时间戳（网关内**唯一**实现，`registry` / `delegate` / `relay` 都从这里取）。
///
/// 工具/节点侧的同名辅助在 `agent_toolkit::now_ms`。为什么这里不直接用它：网关有
/// "只依赖 `agent-proto`（线协议）"的硬边界（`lib.rs` + check J），不能为了一个时间戳
/// 把工具包拉进依赖图。这是**受控**的重复：两份实现必须保持同一语义
/// （`duration_since` 失败返回 0）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Windows 下隐藏子进程控制台窗口。
///
/// 同源的修复见桌面壳（已随 Tauri 宿主移除，那条注释见 git 历史：不加这个会闪一个黑窗）。
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
    use crate::registry::AgentInstance;

    /// 回收策略的三个出口：清理死条目 / 回收空闲 / 超上限驱逐。
    ///
    /// 这条钉住"进程数可控"：空闲久的一定被回收，刚动过的一律不动，
    /// 没超上限时不做任何驱逐。
    #[tokio::test]
    async fn test_reap_once_prunes_idle_and_caps() {
        let reg = AgentRegistry::new();
        let now = now_ms();
        let mk = |id: &str, last: i64, status: AgentStatus| AgentInstance {
            id: id.to_string(),
            product: "ada-coding".to_string(),
            workspace: format!("E:/{id}"),
            endpoint: format!("ws://127.0.0.1:1/rpc?token={id}"),
            status,
            pid: None,
            started_at: last,
            products: vec!["ada-coding".to_string()],
            last_active_ms: last,
        };

        reg.register(mk("old", now - 3_600_000, AgentStatus::Ready));
        reg.register(mk("recent", now - 5_000, AgentStatus::Ready));
        reg.register(mk("dead", now - 1000, AgentStatus::Stopped));
        // 注意：receiver 必须**活到断言之后**——`watch::Sender::send` 在没有接收端时会失败，
        // 而生产环境里接收端就是守卫协程（它一直在）。
        let mut _keep_rx = Vec::new();
        for id in ["old", "recent"] {
            let (tx, rx) = tokio::sync::watch::channel(false);
            reg.attach_shutdown(id, tx);
            _keep_rx.push(rx);
        }

        // 空闲阈值 1 分钟 → 只有 old（闲了 1 小时）被回收；上限 3 → 不驱逐
        let policy = ReapPolicy {
            idle_after: Duration::from_secs(60),
            max_live: 3,
            min_idle_before_evict: Duration::from_secs(30),
            interval: Duration::from_secs(60),
        };
        let (pruned, reaped, evicted) = reap_once(&reg, &policy);
        assert_eq!(pruned, 1, "死条目必须被清理（否则进程列表一直挂着死进程）");
        assert_eq!(reaped, 1, "只有空闲超阈值的 old 被回收（刚动过的不许动）");
        assert_eq!(evicted, 0, "没超上限就不驱逐");

        // 另一组：三条都闲了 30s 以上，上限 1 → 驱逐最久没流量的两条
        let reg2 = AgentRegistry::new();
        for (id, idle_ms) in [("a", 40_000), ("b", 35_000), ("c", 30_000)] {
            reg2.register(mk(id, now - idle_ms, AgentStatus::Ready));
            let (tx, rx) = tokio::sync::watch::channel(false);
            reg2.attach_shutdown(id, tx);
            _keep_rx.push(rx);
        }
        let policy2 = ReapPolicy {
            idle_after: Duration::from_secs(3600), // 这一轮不测空闲回收
            max_live: 1,
            min_idle_before_evict: Duration::from_secs(10),
            interval: Duration::from_secs(60),
        };
        let (_p, _r, evicted2) = reap_once(&reg2, &policy2);
        assert_eq!(evicted2, 2, "超上限时按 LRU 驱逐到只剩 max_live 条");
        assert!(reg2.get("c").is_some(), "最活跃的那条必须留下");
    }

    #[test]
    fn test_binary_not_found_lists_what_was_tried() {        let err = find_product_binary("definitely-not-a-real-binary-xyz", None)
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
