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
use std::sync::RwLock;

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
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按工作区派生实例 id（同一工作区 → 同一 id → 复用实例）。
    ///
    /// 为什么按工作区而不是按客户端：网关的职责是"按工作区/产品路由"，
    /// 同一工作区的多个客户端应当落到**同一个** agent（否则两份状态互相覆盖）。
    pub fn id_for_workspace(product: &str, workspace: &Path) -> String {
        let ws = workspace.to_string_lossy().replace('\\', "/");
        let ws = ws.trim_end_matches('/');
        format!("{product}::{}", short_hash(ws))
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
    pub fn remove(&self, id: &str) -> Option<AgentInstance> {
        self.inner.write().expect("注册表锁被毒化").remove(id)
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

/// 工作区规范化（去掉尾部分隔符，统一 `/`），让"同一工作区的不同写法"落到同一实例。
pub fn normalize_workspace(raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    PathBuf::from(if s.is_empty() { "/".to_string() } else { s })
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

    #[test]
    fn test_different_products_same_workspace_are_different_instances() {
        let ws = normalize_workspace("E:/codes/x");
        let a = AgentRegistry::id_for_workspace("ada-coding", &ws);
        let b = AgentRegistry::id_for_workspace("pm-assistant", &ws);
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
