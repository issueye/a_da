//! 节点级配置端口的**桥接层实现**（S2）。
//!
//! 这是**唯一**知道"节点配置其实存在 `AgentStore` 里"的地方。
//! 节点层（`subagents/tool.rs`）只认
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
/// 持 `Arc<RwLock<AgentStore>>` 的**共享句柄**而非快照：provider 与上下文窗口都是
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
        // 为什么不用 `blocking_read()`：委派跑在 Tokio 运行时里，
        // 在运行时线程上阻塞会 panic 或死锁。这里选择**读快照失败就用默认值**，
        // 并由消费者按"provider 不可用"如实报错（`subagents/tool.rs` 就是这么做的），
        // 绝不编造一个能连上的配置。
        match self.store.try_read() {
            Ok(s) => s.provider.clone(),
            Err(_) => agent_base::model::ProviderConfig::default(),
        }
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

    fn store_with() -> Arc<RwLock<AgentStore>> {
        let mut st = AgentStore::new("E:/node_config_test".to_string());
        st.provider.base_url = "https://example.invalid/v1".to_string();
        st.config.context_window = 64_000;
        Arc::new(RwLock::new(st))
    }

    #[tokio::test]
    async fn test_store_backed_config_reads_live_values() {
        let store = store_with();
        let cfg = StoreBackedNodeConfig::new(store.clone());

        assert_eq!(cfg.provider().base_url, "https://example.invalid/v1");
        assert_eq!(cfg.context_window(), Some(64_000));

        store.write().await.config.context_window = 32_000;
        assert_eq!(
            cfg.context_window(),
            Some(32_000),
            "端口必须读实时值，不能缓存"
        );
    }
}
