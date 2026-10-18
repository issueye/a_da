//! 节点级配置端口的**桥接层实现**（S2）。
//!
//! 这是**唯一**知道"节点配置其实存在 `AgentStore` 里"的地方。
//! 节点层（`approval/gate.rs`、`subagents/tool.rs`）只认
//! [`agent_node::node_config::NodeConfigSource`]，不认 `AgentStore`。
//!
//! 放在 `server/`（桥接层）而不是节点层的理由：`AgentStore` 是**给界面看的投影**，
//! "投影里恰好带着节点要用的字段"是实现细节，只有桥接层该知道。

use std::sync::Arc;

use tokio::sync::RwLock;

use agent_node::node_config::NodeConfigSource;
use crate::state::AgentStore;
/// 从 `AgentStore` 读节点配置（生产实现）。
///
/// 持 `Arc<RwLock<AgentStore>>` 的**共享句柄**而非快照：审批档位与 provider 都是
/// 运行时可变值，界面改完必须立刻生效。
pub struct StoreBackedNodeConfig {
    store: Arc<RwLock<AgentStore>>,
    /// 审批档位的**无锁镜像**（构造时从 store 抓一份句柄）。
    ///
    /// 拿不到它才会退化成"抢锁读"；有了它，锁被占着也能读到**真实档位**
    /// （2026-10-18 真事故：后台磁盘同步握着写锁几秒，那几秒里读不到 → 回落 `Ask`
    ///  → 明明开着完全访问却弹出审批框）。
    approval_shared: Option<Arc<std::sync::atomic::AtomicU8>>,
    /// 回落路径只警告一次（否则每次工具调用刷一行日志）
    warned_lock_contention: std::sync::atomic::AtomicBool,
}

impl StoreBackedNodeConfig {
    pub fn new(store: Arc<RwLock<AgentStore>>) -> Self {
        // 构造期抢一次锁拿镜像句柄；抢不到不算失败（后面每次读还会刷新它）
        let approval_shared = {
            let mut shared = None;
            for _ in 0..50 {
                if let Ok(s) = store.try_read() {
                    shared = Some(s.approval_mode_shared_handle());
                    break;
                }
                std::thread::yield_now();
            }
            shared
        };
        Self {
            store,
            approval_shared,
            warned_lock_contention: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// P1b（SP-33）：同一个档位真源也喂给 `Scope`。
///
/// 权限档位（能不能越出工作区）与审批档位（问不问）**必须读同一处**——两处各读一份
/// 迟早会出现"界面显示完全访问、路径却仍被锁在工作区"这种自相矛盾。
/// 适配器层不认识 `AgentStore`，所以由这里（唯一知道"配置存在投影里"的地方）实现它的小接口。
impl agent_adapter::scope::ApprovalModeSource for StoreBackedNodeConfig {
    fn approval_mode(&self) -> agent_proto::ApprovalMode {
        NodeConfigSource::approval_mode(self)
    }
}

impl NodeConfigSource for StoreBackedNodeConfig {
    fn provider(&self) -> agent_base::model::ProviderConfig {
        // `try_read` 兜底：本方法在同步签名里被调用（端口要求 `&self` 返回值），
        // 而 `AgentStore` 的锁是异步的。
        //
        // 为什么不用 `blocking_read()`：审批/委派都跑在 Tokio 运行时里，
        // 在运行时线程上阻塞会 panic 或死锁。这里选择**读快照失败就用默认值**，
        // 并由消费者按"provider 不可用"如实报错（`subagents/tool.rs` 就是这么做的），
        // 绝不编造一个能连上的配置。
        match self.store.try_read() {
            Ok(s) => s.provider.clone(),
            Err(_) => agent_base::model::ProviderConfig::default(),
        }
    }

    fn approval_mode(&self) -> agent_proto::ApprovalMode {
        use std::sync::atomic::Ordering;

        if let Ok(s) = self.store.try_read() {
            let mode = s.config.approval;
            if let Some(shared) = &self.approval_shared {
                shared.store(crate::state::store::approval_mode_code(mode), Ordering::Release);
            }
            return mode;
        }

        // 锁被占着（后台磁盘同步 / 事件泵都可能持有它）：读**无锁镜像**，而不是"读不到就当 Ask"。
        //
        // 🔴 为什么不能回落 `Ask`：那等于把"我没能确认档位"变成"更严格地打扰用户"，
        // 而表现是**随机**的——同样开着完全访问，十次里弹一次框，没人能复现。
        // 真正的失败方向仍保留：镜像里**确实什么都没读到**（冷启动）才回落 `Ask`。
        if let Some(mode) = self
            .approval_shared
            .as_ref()
            .and_then(|shared| {
                crate::state::store::approval_mode_from_code(shared.load(Ordering::Acquire))
            })
        {
            if !self
                .warned_lock_contention
                .swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    "[approval] 读档位时 store 写锁被占用，已改用无锁镜像（档位：{mode:?}）"
                );
            }
            return mode;
        }

        self.warned_lock_contention.store(true, Ordering::Relaxed);
        agent_proto::ApprovalMode::Ask
    }

    /// 上下文窗口上限：读界面配置里的 `contextWindow`（运行时可改，实时读）。
    ///
    /// 读不到锁时返回 `None`（**不做治理**）而不是编一个数字——治理本身是"提前失败"的
    /// 保守动作，但依据必须是真值；没有真值就交回给 provider 自己的错误。
    fn context_window(&self) -> Option<u64> {
        match self.store.try_read() {
            Ok(s) => {
                let cw = s.config.context_window;
                if cw > 0 {
                    Some(cw)
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_node::node_config::NodeConfigSource;

    fn store_with(mode: agent_proto::ApprovalMode) -> Arc<RwLock<AgentStore>> {
        let mut st = AgentStore::new("E:/node_config_test".to_string());
        st.set_approval_mode(mode);
        st.provider.base_url = "https://example.invalid/v1".to_string();
        Arc::new(RwLock::new(st))
    }

    /// 桥接实现必须**读当前值**，不是构造时快照——否则界面改档位不生效。
    #[tokio::test]
    async fn test_store_backed_config_reads_live_values() {
        let store = store_with(agent_proto::ApprovalMode::Auto);
        let cfg = StoreBackedNodeConfig::new(store.clone());

        assert_eq!(cfg.approval_mode(), agent_proto::ApprovalMode::Auto);
        assert_eq!(cfg.provider().base_url, "https://example.invalid/v1");

        // 改档位后立刻可见
        store.write().await.set_approval_mode(agent_proto::ApprovalMode::Readonly);
        assert_eq!(
            cfg.approval_mode(),
            agent_proto::ApprovalMode::Readonly,
            "端口必须读实时值，不能缓存"
        );
    }

    /// 🔴 2026-10-18 真事故：锁被占着时读档位**不再**回落 `Ask`。
    ///
    /// 早先这里是"读不到 → 倒向 Ask（更安全的一侧）"。可"读不到"根本不是罕见分支：
    /// 后台磁盘同步（实测一次几秒，代码里自己写着它握着写锁）和事件泵都会长时间占着锁。
    /// 于是开着**完全访问**也会随机弹审批框——同一次操作十次里弹一次，没人能复现，
    /// 而用户看到的只是"完全访问好像不太管用"。
    ///
    /// 现在锁被占时改读**无锁镜像**，拿到的是真实档位；只有"镜像里也确实什么都没读到"
    /// （冷启动）才回落 `Ask`，那才是真正没有依据的时候。
    #[test]
    fn test_store_backed_config_reads_real_mode_when_locked() {
        let store = store_with(agent_proto::ApprovalMode::Full);
        let cfg = StoreBackedNodeConfig::new(store.clone());

        // 占住写锁，让 try_read 失败（模拟后台同步/事件泵正在跑）
        let _guard = store.blocking_write();
        assert_eq!(
            cfg.approval_mode(),
            agent_proto::ApprovalMode::Full,
            "锁被占着也必须读到真实档位，不能把『没读到』变成『更严格地打扰用户』"
        );
    }

    /// 镜像不是陈旧快照：改档位的同一时刻它就得跟着变。
    ///
    /// 反过来做的话就是"刚点回『每次都问』，工具还在自动放行"——那比多问一次危险得多。
    #[test]
    fn test_store_backed_config_mirror_follows_the_latest_switch() {
        let store = store_with(agent_proto::ApprovalMode::Full);
        let cfg = StoreBackedNodeConfig::new(store.clone());

        let mut guard = store.blocking_write();
        guard.set_approval_mode(agent_proto::ApprovalMode::Ask);
        assert_eq!(
            cfg.approval_mode(),
            agent_proto::ApprovalMode::Ask,
            "镜像必须与写入同步，否则收紧档位会不生效"
        );
    }
}
