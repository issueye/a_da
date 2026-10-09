//! **委派深度端口**（S6 补完）：让被派活的节点知道自己**在第几层**。
//!
//! # 为什么必须有这个端口
//!
//! 网关能强制"一次派活的深度"（`gateway.delegate { depth }` + 上限），但它**管不到下一跳**：
//! 目标节点 B 自己再发起委派时，携带的深度是从**它的装配声明**来的（`GatewayAgentBus::new(_, 1)`），
//! 于是又变成 1 —— A→B→C→D… 每一跳都是 1，**上限形同虚设**。
//!
//! 修法是让 B 知道"我是被第 1 层派活驱动起来的"，于是它发起的委派是第 2 层。
//! 这条信息只有**驱动方**（网关）能给，而网关能给的唯一通道是它本来就有的
//! `thread.create` 参数 —— 所以链路是：
//!
//! ```text
//! 网关 thread.create { delegationDepth: 1 }
//!   → 宿主把深度记在**线程**上（AgentStore）
//!   → 本端口按 thread_id 查出来
//!   → 工具算出"我这一跳"的深度 = 我的深度 + 1
//!   → GatewayAgentBus 带进 gateway.delegate → 网关按上限拒绝
//! ```
//!
//! # 为什么按**线程**而不是按进程
//!
//! 同一个节点进程同时服务多个线程：用户直接连的线程（深度 0）与被委派的线程（深度 ≥1）。
//! 深度是**线程的属性**，不是进程的属性——按进程记会把"用户那一轮"也算成被委派的。

use agent_base::ports::BoxFuture;

/// 查询某个线程**被第几层委派**驱动起来的。
///
/// - `0` = 不是被委派的（用户直接连上来的那一轮）
/// - `n ≥ 1` = 由第 n 层委派驱动
///
/// **异步**：实现要读宿主的会话态（在异步锁后面）。同步签名会逼实现去加锁阻塞。
pub trait DelegationDepthSource: Send + Sync + 'static {
    fn depth_for_thread<'a>(&'a self, thread_id: &'a str) -> BoxFuture<'a, u32>;
}

/// 固定深度（离线形态 / 测试 / 没有会话态的宿主）。
///
/// `0` 是**正确**的默认值：没有会话态就意味着"不是被网关派活驱动起来的"。
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedDelegationDepth(pub u32);

impl DelegationDepthSource for FixedDelegationDepth {
    fn depth_for_thread<'a>(&'a self, _thread_id: &'a str) -> BoxFuture<'a, u32> {
        let d = self.0;
        Box::pin(async move { d })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fixed_source_returns_its_depth() {
        let s = FixedDelegationDepth(2);
        assert_eq!(s.depth_for_thread("any").await, 2);
        assert_eq!(FixedDelegationDepth::default().depth_for_thread("any").await, 0);
    }
}
