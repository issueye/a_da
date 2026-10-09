//! 节点级配置读取端口（S2）。
//!
//! # 为什么需要这个端口
//!
//! `approval/gate.rs`（审批闸门）与 `subagents/tool.rs`（委派工具）是**节点层**代码——
//! 它们决定"要不要问用户"、"子智能体用哪个模型"，这些是**领域行为**。
//!
//! 但它们原先直接读 `crate::state::AgentStore`（**UI 投影**）：
//!
//! ```text
//! 节点层（审批/委派） ──读──► AgentStore（UI 投影） ──含──► agent_proto::ConfigSnapshot（线上 DTO）
//! ```
//!
//! 方向是反的，而且比"依赖 UI 结构"更糟：`AgentStore.config` 的类型是
//! **`agent_proto::ConfigSnapshot`——一个线上 DTO**。也就是说，
//! **节点行为在依据"准备发给界面的那串 JSON 的形状"做决定**。
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
//!
//! # 为什么端口暂时放在 `agent-core` 而不是 `agent-base`
//!
//! [`ProviderConfig`] 已经是基座类型（`agent_base::model`），但 `ApprovalMode` 目前仍是
//! **协议 DTO**（`agent_proto::dto`）。把领域概念从线上协议里挪出来是**拆包（S4）**的事，
//! 现在动它会扩大 S2 的爆炸半径。届时本端口随节点一起搬进 `agent-node`，
//! `ApprovalMode` 同时收敛为领域类型。

use std::sync::Arc;

use agent_base::model::ProviderConfig;
use agent_proto::ApprovalMode;

/// 节点级配置读取端口。
///
/// 实现者只应有**一个生产实现**（桥接层的 `StoreBackedNodeConfig`）；
/// 测试可以用固定值的替身，不必构造整个 `AgentStore`。
pub trait NodeConfigSource: Send + Sync + 'static {
    /// 本节点当前使用的模型配置。
    ///
    /// 消费者：委派工具（子智能体要用**同一个**模型）、模型客户端装配。
    fn provider(&self) -> ProviderConfig;

    /// 审批档位（`auto` / `ask` / `readonly`）。
    ///
    /// 消费者：审批闸门。这是**运行时可变**的值——界面改档位后必须立刻生效，
    /// 所以实现要读当前值，不能构造时快照。
    fn approval_mode(&self) -> ApprovalMode;
}

/// 固定值的测试替身（生产代码请用桥接层的 `StoreBackedNodeConfig`）。
#[derive(Debug, Clone)]
pub struct FixedNodeConfig {
    pub provider: ProviderConfig,
    pub approval_mode: ApprovalMode,
}

impl FixedNodeConfig {
    pub fn new(provider: ProviderConfig, approval_mode: ApprovalMode) -> Self {
        Self {
            provider,
            approval_mode,
        }
    }
}

impl NodeConfigSource for FixedNodeConfig {
    fn provider(&self) -> ProviderConfig {
        self.provider.clone()
    }

    fn approval_mode(&self) -> ApprovalMode {
        self.approval_mode
    }
}

/// 便捷包装：把实现塞进 `Arc<dyn NodeConfigSource>`。
pub fn shared(config: impl NodeConfigSource) -> Arc<dyn NodeConfigSource> {
    Arc::new(config)
}
