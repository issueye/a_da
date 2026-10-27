//! 节点级配置读取端口（S2）。
//!
//! # 为什么需要这个端口
//!
//! `subagents/tool.rs`（委派工具）是**节点层**代码——它决定"子智能体用哪个模型"，
//! 这是**领域行为**。但它原先直接读 `crate::state::AgentStore`（**UI 投影**）：
//!
//! ```text
//! 节点层（委派） ──读──► AgentStore（UI 投影） ──含──► agent_proto::ConfigSnapshot（线上 DTO）
//! ```
//!
//! 方向是反的：**节点行为在依据"准备发给界面的那串 JSON 的形状"做决定**。
//!
//! # 修法
//!
//! 节点层只依赖本端口；**唯一**知道"配置其实存在 `AgentStore` 里"的地方是
//! [`crate::server::node_config::StoreBackedNodeConfig`]（桥接层），
//! 由组合根（`agent-host`）装配注入。
//!
//! ```text
//! 节点层 ──► NodeConfigSource（本端口） ◄── StoreBackedNodeConfig ◄── AgentStore
//!                                              （桥接层，唯一知情者）
//! ```

use std::sync::Arc;

use agent_base::model::ProviderConfig;

/// 节点级配置读取端口。
///
/// 实现者只应有**一个生产实现**（桥接层的 `StoreBackedNodeConfig`）；
/// 测试可以用固定值的替身，不必构造整个 `AgentStore`。
pub trait NodeConfigSource: Send + Sync + 'static {
    /// 本节点当前使用的模型配置。
    ///
    /// 消费者：委派工具（子智能体要用**同一个**模型）、模型客户端装配。
    fn provider(&self) -> ProviderConfig;

    /// 当前模型的**上下文窗口上限**（tokens）。
    ///
    /// 消费者：引擎的上下文治理——上一轮如实报回的用量逼近上限时**提前失败**，
    /// 而不是发一个注定被拒的请求（实测 445k tokens 的请求被网关在流式过程中掐断，
    /// 报告只写了一半，界面只看到笼统的"模型调用失败"）。
    ///
    /// 默认实现返回 `None`（未登记 → 不做治理）：这样既有实现不必改动，
    /// 只有真正知道窗口的桥接层（`StoreBackedNodeConfig`）覆盖它。
    fn context_window(&self) -> Option<u64> {
        None
    }
}

/// 固定值的测试替身（生产代码请用桥接层的 `StoreBackedNodeConfig`）。
#[derive(Debug, Clone)]
pub struct FixedNodeConfig {
    pub provider: ProviderConfig,
}

impl FixedNodeConfig {
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }
}

impl NodeConfigSource for FixedNodeConfig {
    fn provider(&self) -> ProviderConfig {
        self.provider.clone()
    }
}

/// 便捷包装：把实现塞进 `Arc<dyn NodeConfigSource>`。
pub fn shared(config: impl NodeConfigSource) -> Arc<dyn NodeConfigSource> {
    Arc::new(config)
}
