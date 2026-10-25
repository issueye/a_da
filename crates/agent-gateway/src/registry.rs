//! **AGENT 管理平台的数据面**：实例注册表。
//!
//! # 它持有的是"网关自己的事实"
//!
//! 注册表里是 `{id, product, workspace, endpoint, status, pid}`——**这些都是网关
//! 自己产生的事实**（它拉起了哪个进程、连上了哪个端口），不是领域事实的副本。
//!
//! 这条区分很重要（INV-8「一个事实一个所有者」）：
//!
//! | 事实 | 所有者 |
//! |---|---|
//! | 进程/端口/健康状态 | **网关**（本文件） |
//! | 线程、消息、工具回执、会话状态 | **agent 节点**（网关**不得**缓存） |
//!
//! 所以本文件里**没有** `threads` / `messages` 之类的字段——那是节点的。
//! `verify-wiring` 的 check J 会盯着"网关不引用节点/引擎"这件事。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::supervisor::now_ms;

/// 一个 agent 实例的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// 已拉起进程，还没收到就绪信号
    Starting,
    /// 收到 `A_DA_HOST_READY`，可以接流量
    Ready,
    /// 进程还在，但健康检查失败
    Unhealthy,
    /// 进程已退出
    Stopped,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Unhealthy => "unhealthy",
            Self::Stopped => "stopped",
        }
    }

    /// 是否可接流量（路由判据）。
    pub fn is_routable(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// 一个被网关管理的 agent 实例。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstance {
    /// 稳定标识。默认按工作区派生（同一工作区复用同一实例）
    pub id: String,
    /// 产品 id（如 `ada-coding`）
    pub product: String,
    /// 工作区绝对路径
    pub workspace: String,
    /// WS 端点（含 token query）
    pub endpoint: String,
    pub status: AgentStatus,
    /// 子进程 pid（未拉起时为 `None`）
    pub pid: Option<u32>,
    /// 拉起时间（毫秒）
    pub started_at: i64,
    /// 这个进程**实际服务**的产品清单（实例自述，如 `["ada-coding"]`）。
    ///
    /// PM 曾是一个独立产品（那时这里是 `["ada-coding", "ada-pm"]`），现已降级为
    /// `ada-coding` 的**模式预设**——所以正常情况下只有一个产品。
    /// "这条会话是不是 PM"由会话的 `mode` 表达，不该再从产品清单里猜。
    /// 老实例不带这个字段 → 空数组。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub products: Vec<String>,
    /// 最后一次**有流量**的时刻（毫秒；网关自己记账，见 [`AgentRegistry::touch`]）。
    ///
    /// 空闲回收（`reap_idle`）与超上限驱逐（LRU）都靠它。0 = 未知（外部注册的老实例）。
    #[serde(rename = "lastActiveMs", default)]
    pub last_active_ms: i64,
}

impl AgentInstance {
    /// 用于对外暴露的摘要（**不含 token**）。
    ///
    /// 管理 API 会把它发给客户端，所以必须剥掉凭据——`endpoint` 里带 `?token=`。
    pub fn redacted(&self) -> serde_json::Value {
        let endpoint = self
            .endpoint
            .split("?")
            .next()
            .unwrap_or(&self.endpoint)
            .to_string();
        serde_json::json!({
            "id": self.id,
            "product": self.product,
            "workspace": self.workspace,
            "endpoint": endpoint,
            "status": self.status.as_str(),
            "pid": self.pid,
            "startedAt": self.started_at,
            "lastActiveMs": self.last_active_ms,
            "products": self.products,
        })
    }
}

/// 注册表：网关对"有哪些 agent、各自什么状态"的唯一记录处。
///
/// 用 `RwLock` + `BTreeMap`：读多写少，且 `BTreeMap` 让 `list()` 的顺序**确定**
/// （测试与快照都依赖确定性，`HashMap` 的迭代顺序会让"同一状态两次读到不同顺序"）。
#[derive(Debug, Default)]
pub struct AgentRegistry {
    inner: RwLock<BTreeMap<String, AgentInstance>>,
    /// 每条实例的**请求关停**开关（内部记账，不序列化、不对外暴露）。
    ///
    /// 空闲回收 / 超上限驱逐 / 网关退出都要能"让那个进程停下来"，
    /// 而子进程句柄握在 `supervisor` 的守护协程手里——所以这里只留一个开关，
    /// 守护协程 `select!` 到它就 kill 子进程并把自己从注册表摘掉。
    shutdowns: RwLock<BTreeMap<String, tokio::sync::watch::Sender<bool>>>,
    /// 每条实例当前的**活跃客户端连接数**（内部记账，不序列化）。
    ///
    /// 🔴 有客户端挂着就**不许回收**：用户可能正对着一条长回答发呆（十几分钟没有帧），
    /// 这时候把 agent 掐掉，界面会莫名其妙掉线重连。所以回收判据是"没人连着 **且** 空闲够久"。
    connections: RwLock<BTreeMap<String, usize>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// **实例身份键**：同一物理工作区的不同写法必须落到同一个实例。
    ///
    /// 为什么不能直接拿字符串去哈希：客户端与配置里的路径写法五花八门——
    /// `E:\codes\x` / `E:/codes/x/` / `e:/Codes/X` / `E:/codes/a/../x` 说的是同一个目录，
    /// 哈希不同就会**给同一个工作区起出好几个 `ada-coding` 进程**（这正是"进程数失控"的成因）。
    /// 所以：统一分隔符 → 折叠 `.`/`..` 段 → 去掉尾分隔符 → Windows 下再折叠大小写。
    /// 只用于**算 id**，展示用的 `workspace` 仍保留首次见到的写法。
    pub fn workspace_key(workspace: &Path) -> String {
        let mut s = workspace.to_string_lossy().replace('\\', "/");
        s = collapse_dot_segments(&s);
        if cfg!(windows) {
            s = s.to_lowercase();
        }
        s
    }

    /// 按工作区派生实例 id（同一工作区 → 同一 id → 复用实例）。
    ///
    /// 为什么按工作区而不是按客户端：网关的职责是"按工作区/产品路由"，
    /// 同一工作区的多个客户端（以及同一工作区下的多个会话）应当落到**同一个** agent
    /// ——否则两份状态互相覆盖，而且一个工作区会堆出好几个进程。
    pub fn id_for_workspace(product: &str, workspace: &Path) -> String {
        format!("{product}::{}", short_hash(&Self::workspace_key(workspace)))
    }

    /// 登记一个实例。返回 `true` 表示是**新登记**，`false` 表示覆盖了已有条目。
    pub fn register(&self, instance: AgentInstance) -> bool {
        let mut map = self.inner.write().expect("注册表锁被毒化");
        map.insert(instance.id.clone(), instance).is_none()
    }

    /// 按 id 取（**含** token 的完整实例，仅供网关内部路由用）。
    pub fn get(&self, id: &str) -> Option<AgentInstance> {
        self.inner.read().expect("注册表锁被毒化").get(id).cloned()
    }

    /// 按工作区取（路由用）。
    pub fn find_by_workspace(&self, product: &str, workspace: &Path) -> Option<AgentInstance> {
        let id = Self::id_for_workspace(product, workspace);
        self.get(&id)
    }

    /// 全部实例（顺序确定）。
    pub fn list(&self) -> Vec<AgentInstance> {
        self.inner
            .read()
            .expect("注册表锁被毒化")
            .values()
            .cloned()
            .collect()
    }

    /// 更新状态。实例不存在时返回 `false`（**不静默创建**）。
    pub fn set_status(&self, id: &str, status: AgentStatus) -> bool {
        let mut map = self.inner.write().expect("注册表锁被毒化");
        match map.get_mut(id) {
            Some(inst) => {
                inst.status = status;
                true
            }
            None => false,
        }
    }

    /// 移除实例（进程退出后回收）。
    ///
    /// 三张表**逐个加锁、绝不同时持有两把写锁**（内部表 / 关停开关 / 连接计数）：
    /// 同时持锁会跟别的路径形成相反的加锁顺序而死锁。
    pub fn remove(&self, id: &str) -> Option<AgentInstance> {
        let removed = self.inner.write().expect("注册表锁被毒化").remove(id);
        self.shutdowns.write().expect("注册表锁被毒化").remove(id);
        self.connections.write().expect("注册表锁被毒化").remove(id);
        removed
    }

    /// 记一次客户端接入（`relay` 绑定实例时调用）。
    pub fn conn_inc(&self, id: &str) {
        *self
            .connections
            .write()
            .expect("注册表锁被毒化")
            .entry(id.to_string())
            .or_insert(0) += 1;
    }

    /// 记一次客户端断开（`relay` 结束时调用；用 guard 保证异常路径也会减）。
    pub fn conn_dec(&self, id: &str) {
        let mut guard = self.connections.write().expect("注册表锁被毒化");
        if let Some(n) = guard.get_mut(id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                guard.remove(id);
            }
        }
    }

    /// 打开一条"长连接"计数守卫：drop 时自动减一。
    ///
    /// 回收器只认这个计数（`connections > 0` 的实例永不回收），所以**任何长期订阅
    /// agent 事件的路径都必须持有它**——包括网关进程内的 Web UI 事件桥（`web.rs`）。
    pub fn open_connection(self: &Arc<Self>, id: impl Into<String>) -> ConnectionGuard {
        let id = id.into();
        self.conn_inc(&id);
        ConnectionGuard {
            registry: self.clone(),
            id,
        }
    }

    /// 当前挂着几个客户端连接。
    pub fn active_connections(&self, id: &str) -> usize {
        self.connections
            .read()
            .expect("注册表锁被毒化")
            .get(id)
            .copied()
            .unwrap_or(0)
    }

    /// 记一次"这条实例还有流量"（路由转发时调用）。
    ///
    /// 空闲回收与 LRU 驱逐都读它：**只要还在转发帧就不算空闲**，
    /// 长跑的一轮对话因此不会被回收掉。
    pub fn touch(&self, id: &str) -> bool {
        let now = now_ms();
        let mut map = self.inner.write().expect("注册表锁被毒化");
        match map.get_mut(id) {
            Some(inst) => {
                inst.last_active_ms = now;
                true
            }
            None => false,
        }
    }

    /// 给实例挂上"请求关停"开关（`supervisor` 拉起后调用）。
    pub fn attach_shutdown(&self, id: &str, tx: tokio::sync::watch::Sender<bool>) {
        self.shutdowns
            .write()
            .expect("注册表锁被毒化")
            .insert(id.to_string(), tx);
    }

    /// 请求关停某条实例：守护协程会 kill 子进程并把它从注册表摘掉。
    ///
    /// 返回 `false` = 这条实例没有受管进程（外部自注册的实例），调用方应改用别的方式。
    pub fn request_shutdown(&self, id: &str) -> bool {
        let guard = self.shutdowns.read().expect("注册表锁被毒化");
        match guard.get(id) {
            Some(tx) => tx.send(true).is_ok(),
            None => false,
        }
    }

    /// 摘掉**已退出**的条目（`Stopped`），避免进程列表里一直挂着死进程。
    ///
    /// 返回清掉几条。
    pub fn prune_stopped(&self) -> usize {
        let dead: Vec<String> = {
            let mut map = self.inner.write().expect("注册表锁被毒化");
            let dead: Vec<String> = map
                .values()
                .filter(|i| i.status == AgentStatus::Stopped)
                .map(|i| i.id.clone())
                .collect();
            for id in &dead {
                map.remove(id);
            }
            dead
        };
        if dead.is_empty() {
            return 0;
        }
        // 三张表逐个加锁，绝不同时持有两把写锁（否则与别的路径形成相反加锁顺序而死锁）
        {
            let mut guard = self.shutdowns.write().expect("注册表锁被毒化");
            for id in &dead {
                guard.remove(id);
            }
        }
        {
            let mut guard = self.connections.write().expect("注册表锁被毒化");
            for id in &dead {
                guard.remove(id);
            }
        }
        dead.len()
    }

    /// 空闲超过 `idle_ms` 的实例 id（按最久没动过排序）。
    ///
    /// 判据是"**没人连着** 且 空闲够久"：`last_active_ms == 0`（外部注册、网关没记过流量）
    /// 视为从 `started_at` 起算；有活跃客户端连接的实例一律不算空闲
    /// （用户可能正对着一条长回答发呆十几分钟，这时候把 agent 掐掉会莫名其妙掉线）。
    pub fn idle_since(&self, now: i64, idle_ms: i64) -> Vec<String> {
        let candidates: Vec<(i64, String)> = {
            let guard = self.inner.read().expect("注册表锁被毒化");
            guard
                .values()
                .filter(|i| i.status.is_routable())
                .map(|i| {
                    let last = if i.last_active_ms > 0 { i.last_active_ms } else { i.started_at };
                    (last, i.id.clone())
                })
                .collect()
        };
        let mut out: Vec<(i64, String)> = candidates
            .into_iter()
            .filter(|(last, id)| now - last >= idle_ms && self.active_connections(id) == 0)
            .collect();
        out.sort();
        out.into_iter().map(|(_, id)| id).collect()
    }

    /// 超出 `max_live` 时需要驱逐的实例（**最久没流量的先走**）。
    ///
    /// 只驱逐"确实闲下来"的（`now - last >= min_idle_before_evict` 且没有客户端连着）：
    /// 正在跑的那条不能被掐掉，哪怕它让总数短暂超限。
    pub fn lru_over_capacity(
        &self,
        now: i64,
        max_live: usize,
        min_idle_before_evict: i64,
    ) -> Vec<String> {
        let live: Vec<(i64, String)> = {
            let guard = self.inner.read().expect("注册表锁被毒化");
            guard
                .values()
                .filter(|i| i.status.is_routable())
                .map(|i| {
                    let last = if i.last_active_ms > 0 { i.last_active_ms } else { i.started_at };
                    (last, i.id.clone())
                })
                .collect()
        };
        if live.len() <= max_live {
            return Vec::new();
        }
        let mut sorted = live;
        // 最久没动过的排前面
        sorted.sort();
        let victims = sorted.len() - max_live;
        sorted
            .into_iter()
            .filter(|(last, id)| {
                now - last >= min_idle_before_evict && self.active_connections(id) == 0
            })
            .take(victims)
            .map(|(_, id)| id)
            .collect()
    }

    /// 请求关停**全部**受管实例（网关退出时用；外部自注册的实例不动）。
    pub fn shutdown_all(&self) -> usize {
        let ids: Vec<String> = self
            .inner
            .read()
            .expect("注册表锁被毒化")
            .values()
            .filter(|i| i.status.is_routable())
            .map(|i| i.id.clone())
            .collect();
        let mut n = 0;
        for id in ids {
            if self.request_shutdown(&id) {
                n += 1;
            }
        }
        n
    }

    pub fn len(&self) -> usize {
        self.inner.read().expect("注册表锁被毒化").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 可接流量的实例（路由候选）。
    pub fn routable(&self) -> Vec<AgentInstance> {
        self.list().into_iter().filter(|i| i.status.is_routable()).collect()
    }
}

/// 稳定的短哈希（不引入 sha2 依赖：网关只需要"同输入同输出"，不需要抗碰撞）。
fn short_hash(s: &str) -> String {
    // FNV-1a 64：确定、快、够用
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 文本层面折叠 `.` / `..` 段（不查文件系统，所以对"还不存在的目录"同样有效）。
///
/// 保留开头的 `/` 或 `X:/` 前缀；`..` 在无可折叠段时原样保留（宁可保守，不乱吃路径）。
fn collapse_dot_segments(path: &str) -> String {
    let (prefix, rest) = if path.len() >= 3 && path.as_bytes()[1] == b':' {
        (&path[..2], &path[2..]) // 盘符前缀 "C:"
    } else if path.starts_with('/') {
        ("", path)
    } else {
        ("", path)
    };
    let absolute = rest.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                // 只有在能安全回退时才回退（绝对路径退到根就停住）
                if stack.last().map(|s| *s != "..").unwrap_or(false) {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    let joined = stack.join("/");
    let lead = if absolute { "/" } else { "" };
    let out = format!("{prefix}{lead}{joined}");
    // 盘符根（"C:" + "/"）与全空都给个稳定形态
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// 工作区规范化（去掉尾部分隔符，统一 `/`），让"同一工作区的不同写法"落到同一实例。
pub fn normalize_workspace(raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    PathBuf::from(if s.is_empty() { "/".to_string() } else { s })
}

/// 长连接计数守卫：持有期间该实例不会被回收器判定为空闲。
///
/// 由 [`AgentRegistry::open_connection`] 创建；drop 时自动 `conn_dec`，
/// 所以异常路径也不会把计数漏高。
pub struct ConnectionGuard {
    registry: Arc<AgentRegistry>,
    id: String,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.registry.conn_dec(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(id: &str, ws: &str, status: AgentStatus) -> AgentInstance {
        AgentInstance {
            id: id.to_string(),
            product: "ada-coding".to_string(),
            workspace: ws.to_string(),
            endpoint: format!("ws://127.0.0.1:1234/rpc?token=secret-{id}"),
            status,
            pid: Some(42),
            started_at: 1,
            products: vec!["ada-coding".to_string()],
            last_active_ms: 1,
        }
    }

    fn inst_at(id: &str, ws: &str, last_active_ms: i64) -> AgentInstance {
        AgentInstance {
            last_active_ms,
            ..inst(id, ws, AgentStatus::Ready)
        }
    }

    #[test]
    fn test_register_is_idempotent_by_id() {
        let reg = AgentRegistry::new();
        assert!(reg.register(inst("a", "E:/ws", AgentStatus::Starting)));
        assert!(!reg.register(inst("a", "E:/ws", AgentStatus::Ready)), "同 id 是覆盖，不是新增");
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("a").unwrap().status, AgentStatus::Ready);
    }

    #[test]
    fn test_id_is_stable_per_workspace() {
        let a = AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace("E:/codes/x/"));
        let b = AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace("E:\\codes\\x"));
        assert_eq!(a, b, "同一工作区的不同写法必须派生同一个 id（否则实例会重复拉起）");
        let c = AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace("E:/codes/y"));
        assert_ne!(a, c, "不同工作区必须是不同 id");
    }

    /// 🔴 路径写法差异必须收敛到同一个 id，否则**同一个工作区会被起出好几个进程**：
    /// 大小写、`.`/`..`、重复分隔符、尾分隔符在 Windows 上都是同一个目录。
    #[test]
    fn test_workspace_key_collapses_path_spellings() {
        let base = AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace("E:/codes/x"));
        for variant in [
            "E:/codes/x",
            "E:/codes/x/",
            "E:\\codes\\x",
            "E:/codes//x",
            "E:/codes/./x",
            "E:/codes/other/../x",
        ] {
            assert_eq!(
                AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace(variant)),
                base,
                "写法 {variant} 应落到同一实例"
            );
        }
        if cfg!(windows) {
            assert_eq!(
                AgentRegistry::id_for_workspace("ada-coding", &normalize_workspace("e:/CODEs/X")),
                base,
                "Windows 路径大小写不敏感：同一个目录不能起两个进程"
            );
        }
    }

    /// 空闲回收 / 超限驱逐的判据：只有"确实闲下来"的才回收，正在跑的不许掐。
    #[test]
    fn test_idle_and_lru_selection() {
        let reg = AgentRegistry::new();
        reg.register(inst_at("old", "E:/old", 1_000));
        reg.register(inst_at("mid", "E:/mid", 5_000));
        reg.register(inst_at("hot", "E:/hot", 10_000));

        let idle = reg.idle_since(10_000, 6_000);
        assert_eq!(idle, vec!["old".to_string()], "只该回收超过空闲阈值的");

        // 上限 2（在跑 3 条）→ 驱逐 1 条：最久没动过的先走；"刚刚还在动"的不许驱逐
        assert_eq!(reg.lru_over_capacity(10_000, 2, 100), vec!["old".to_string()]);
        // 上限 1 → 驱逐 2 条；min_idle=0 时刚动过的也可驱逐
        assert_eq!(
            reg.lru_over_capacity(10_000, 1, 0),
            vec!["old".to_string(), "mid".to_string()]
        );
        // 上限 2 但只允许驱逐"闲够 1 秒的"：只有 old 符合 → 只驱逐 1 条（宁可短暂超限，也不掐正在跑的）
        assert_eq!(reg.lru_over_capacity(10_000, 1, 6_000), vec!["old".to_string()]);
        assert!(reg.lru_over_capacity(10_000, 3, 0).is_empty(), "没超上限就不动");
    }

    #[test]
    fn test_touch_keeps_instance_off_the_reap_list() {
        let reg = AgentRegistry::new();
        reg.register(inst_at("a", "E:/a", 1_000));
        assert_eq!(reg.idle_since(now_ms(), 60_000), vec!["a".to_string()]);
        assert!(reg.touch("a"), "touch 命中已登记的实例");
        assert!(
            reg.idle_since(now_ms(), 60_000).is_empty(),
            "刚有流量就不该被回收"
        );
        assert!(!reg.touch("ghost"), "不存在的实例不静默创建");
    }

    /// 🔴 **有客户端连着就不许回收**：用户可能正对着一条长回答发呆十几分钟，
    /// 这时候把 agent 掐掉，界面会莫名其妙掉线重连。
    #[test]
    fn test_active_connection_blocks_reaping() {
        let reg = AgentRegistry::new();
        reg.register(inst_at("attached", "E:/a", 1_000));
        reg.register(inst_at("idle", "E:/b", 1_000));

        reg.conn_inc("attached");
        assert_eq!(reg.active_connections("attached"), 1);
        assert_eq!(
            reg.idle_since(now_ms(), 1),
            vec!["idle".to_string()],
            "挂客户端的实例不算空闲（哪怕它很久没有帧）"
        );
        assert_eq!(
            reg.lru_over_capacity(now_ms(), 1, 0),
            vec!["idle".to_string()],
            "超上限驱逐也只能动没连接的那条"
        );

        reg.conn_dec("attached");
        assert_eq!(reg.active_connections("attached"), 0);
        assert_eq!(reg.idle_since(now_ms(), 1).len(), 2, "断开后两条都可回收");
    }

    /// 连接计数用 guard 语义：重复减不会变成负数（异常路径重复 drop 也不炸）。
    #[test]
    fn test_connection_count_never_underflows() {
        let reg = AgentRegistry::new();
        reg.register(inst_at("a", "E:/a", 1_000));
        reg.conn_inc("a");
        reg.conn_inc("a");
        assert_eq!(reg.active_connections("a"), 2);
        reg.conn_dec("a");
        reg.conn_dec("a");
        reg.conn_dec("a");
        assert_eq!(reg.active_connections("a"), 0, "多减一次不能变成负数/残留");
    }

    #[test]
    fn test_prune_stopped_removes_dead_rows() {
        let reg = AgentRegistry::new();
        reg.register(inst("live", "E:/a", AgentStatus::Ready));
        reg.register(inst("dead", "E:/b", AgentStatus::Stopped));
        assert_eq!(reg.prune_stopped(), 1);
        assert_eq!(reg.len(), 1);
        assert!(reg.get("dead").is_none());
    }

    /// 受管实例能被"请求关停"（回收/驱逐走这条路）；外部自注册的实例没有开关。
    #[tokio::test]
    async fn test_request_shutdown_only_for_managed_instances() {
        let reg = AgentRegistry::new();
        reg.register(inst("managed", "E:/a", AgentStatus::Ready));
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        reg.attach_shutdown("managed", tx);

        assert!(reg.request_shutdown("managed"), "受管实例应能请求关停");
        rx.changed().await.expect("应收到关停信号");
        assert!(*rx.borrow(), "信号值必须是 true");

        assert!(!reg.request_shutdown("external"), "没有受管进程的实例返回 false");
    }

    #[test]
    fn test_different_products_same_workspace_are_different_instances() {
        let ws = normalize_workspace("E:/codes/x");
        let a = AgentRegistry::id_for_workspace("ada-coding", &ws);
        let b = AgentRegistry::id_for_workspace("ada-pm", &ws);
        assert_ne!(a, b, "产品是路由维度的一部分");
    }

    #[test]
    fn test_set_status_does_not_silently_create() {
        let reg = AgentRegistry::new();
        assert!(!reg.set_status("ghost", AgentStatus::Ready), "不存在的实例不得被静默创建");
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_list_order_is_deterministic() {
        let reg = AgentRegistry::new();
        for id in ["c", "a", "b"] {
            reg.register(inst(id, "E:/ws", AgentStatus::Ready));
        }
        let ids: Vec<String> = reg.list().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["a", "b", "c"], "BTreeMap 保证顺序确定（快照/测试都依赖它）");
    }

    #[test]
    fn test_routable_filters_out_non_ready() {
        let reg = AgentRegistry::new();
        reg.register(inst("a", "E:/a", AgentStatus::Ready));
        reg.register(inst("b", "E:/b", AgentStatus::Starting));
        reg.register(inst("c", "E:/c", AgentStatus::Stopped));
        let ids: Vec<String> = reg.routable().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["a"], "只有 ready 可接流量");
    }

    /// 管理 API 会把实例摘要发给客户端，**必须剥掉 token**。
    #[test]
    fn test_redacted_strips_token_from_endpoint() {
        let i = inst("a", "E:/ws", AgentStatus::Ready);
        let v = i.redacted();
        let endpoint = v.get("endpoint").and_then(|x| x.as_str()).unwrap();
        assert_eq!(endpoint, "ws://127.0.0.1:1234/rpc", "endpoint 不得带 token");
        let dumped = serde_json::to_string(&v).unwrap();
        assert!(!dumped.contains("secret-a"), "整份摘要里都不得出现 token：{dumped}");
    }

    #[test]
    fn test_remove_returns_the_entry_and_clears_it() {
        let reg = AgentRegistry::new();
        reg.register(inst("a", "E:/ws", AgentStatus::Ready));
        let gone = reg.remove("a").expect("应返回被移除的实例");
        assert_eq!(gone.id, "a");
        assert!(reg.is_empty());
        assert!(reg.remove("a").is_none());
    }
}
