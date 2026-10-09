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
}

impl StoreBackedNodeConfig {
    pub fn new(store: Arc<RwLock<AgentStore>>) -> Self {
        Self { store }
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
        match self.store.try_read() {
            Ok(s) => s.config.approval,
            // 读不到档位时**倒向更安全的一侧**：`Ask`（每次都问）而不是 `Auto`（自动放行）。
            // 与 `FailDirection::Closed` 同一原则——拿不到依据时不放开。
            Err(_) => agent_proto::ApprovalMode::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_node::node_config::NodeConfigSource;

    fn store_with(mode: agent_proto::ApprovalMode) -> Arc<RwLock<AgentStore>> {
        let mut st = AgentStore::new("E:/node_config_test".to_string());
        st.config.approval = mode;
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
        store.write().await.config.approval = agent_proto::ApprovalMode::Readonly;
        assert_eq!(
            cfg.approval_mode(),
            agent_proto::ApprovalMode::Readonly,
            "端口必须读实时值，不能缓存"
        );
    }

    /// 拿不到锁时**倒向安全侧**：档位回 `Ask`（每次都问），不放开。
    #[test]
    fn test_store_backed_config_fails_safe_when_locked() {
        let store = store_with(agent_proto::ApprovalMode::Auto);
        let cfg = StoreBackedNodeConfig::new(store.clone());

        // 占住写锁，让 try_read 失败
        let _guard = store.blocking_write();
        assert_eq!(
            cfg.approval_mode(),
            agent_proto::ApprovalMode::Ask,
            "读不到档位时必须倒向 Ask（更安全的一侧），而不是 Auto（自动放行）"
        );
    }
}
