//! [`DelegationDepthSource`] 的**桥接面实现**（S6 补完）：从会话态里读委派深度。
//!
//! 为什么实现放这里而不是节点里：深度记在 `AgentStore` 上，而 `AgentStore` 是
//! **桥接面的事实**（给界面看的投影 + 驱动方记账）。节点只保留端口。
//!
//! 这正是 `verify-wiring` 的 check H 要的形状：节点声明端口，桥接面提供实现。

use std::sync::Arc;

use agent_base::ports::BoxFuture;
use agent_node::delegation_depth::DelegationDepthSource;
use tokio::sync::RwLock;

use crate::state::AgentStore;

/// 从 `AgentStore.delegation_depths` 读深度。
pub struct StoreBackedDelegationDepth {
    store: Arc<RwLock<AgentStore>>,
}

impl StoreBackedDelegationDepth {
    pub fn new(store: Arc<RwLock<AgentStore>>) -> Self {
        Self { store }
    }
}

impl DelegationDepthSource for StoreBackedDelegationDepth {
    fn depth_for_thread<'a>(&'a self, thread_id: &'a str) -> BoxFuture<'a, u32> {
        Box::pin(async move {
            let store = self.store.read().await;
            // 查不到 = 用户直接连的线程 = 0（**不猜**：缺省就是"没有被委派"）
            store
                .delegation_depths
                .get(thread_id)
                .copied()
                .unwrap_or(0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reads_depth_per_thread_and_defaults_to_zero() {
        let store = Arc::new(RwLock::new(AgentStore::new(
            std::env::temp_dir().to_string_lossy().to_string(),
        )));
        let src = StoreBackedDelegationDepth::new(store.clone());

        // 没记过 → 0（用户直接连的）
        assert_eq!(src.depth_for_thread("user-thread").await, 0);

        store
            .write()
            .await
            .delegation_depths
            .insert("delegated".to_string(), 2);
        assert_eq!(src.depth_for_thread("delegated").await, 2);
        // **按线程**：别的线程不受影响
        assert_eq!(src.depth_for_thread("user-thread").await, 0);
    }
}