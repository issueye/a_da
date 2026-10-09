//! 取消端口的真实实现（`agent_base::ports::CancelToken`）。
//!
//! 设计口径（`docs/agent-base-wiring-plan.md` §5 W1-T4、AGENTS.md 计划 §1.3 的"取消断链"）：
//! 取消必须**一路传到底**——模型流、工具执行、子进程、子智能体。
//! 端口本身只要求一个轮询方法 `is_cancelled()`；但真正要把取消传导到**子进程**，
//! 适配器需要一个"等它发生"的入口，所以这里额外提供（**固有方法，不是端口契约**）：
//!
//! - [`CancelHandle::wait_cancelled`]：异步等待取消发生；
//! - [`CancelHandle::subscribe`]：拿到底层 `watch` 通道，供 `run_command` 这类
//!   需要 `abort_rx` 的适配器直接桥接（W4-T1 用它修掉 `abort_tx` 假接线）。
//!
//! 这两条**没有**加进 `CancelToken` trait：加契约方法会影响所有实现与合规套件，
//! 属于端口变更，必须单独排任务并留档（R3/R4），不在 W1-T4 里顺手做。

use std::sync::Arc;

use agent_base::ports::CancelToken;
use tokio::sync::watch;

/// 等待一个 `watch` 通道翻转为 `true`；发送端被丢弃时直接返回（视为"不会再取消"）。
async fn wait_rx(mut rx: watch::Receiver<bool>) {
    if *rx.borrow_and_update() {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        if *rx.borrow_and_update() {
            return;
        }
    }
}

/// 可触发的取消令牌。
///
/// `Clone` 共享**同一个**底层通道（不是各自一份状态）——多个持有者拿到的是同一把"停止键"。
#[derive(Clone)]
pub struct CancelHandle {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for CancelHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelHandle {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    /// 触发取消。返回**本次是否真的翻转**（重复取消返回 `false`），
    /// 这样调用方不必自己判断"是不是第一个按停止键的人"。
    pub fn cancel(&self) -> bool {
        !self.tx.send_replace(true)
    }

    /// 异步等待取消发生（已取消则立即返回）。
    pub async fn wait_cancelled(&self) {
        wait_rx(self.rx.clone()).await;
    }

    /// 订阅底层通道。给需要把取消桥接到别的机制的适配器用
    /// （例：`run_command` 的 `abort_rx`、子智能体的中止信号）。
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }

    /// 派生**子令牌**：父被取消时子随之取消；子自己取消**不会**影响父。
    ///
    /// 子智能体用这个：父会话按停止要能连带停掉子体，而子体自己的取消
    /// （例如它自己跑完/超时）不该把父会话也停掉。
    pub fn child(&self) -> ChildCancel {
        ChildCancel { parent: self.rx.clone(), own: CancelHandle::new() }
    }
}

impl CancelToken for CancelHandle {
    fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }
}

/// 子令牌：父或自己任一被取消即视为已取消。
#[derive(Clone)]
pub struct ChildCancel {
    parent: watch::Receiver<bool>,
    own: CancelHandle,
}

impl ChildCancel {
    /// 只取消自己（不影响父）。
    pub fn cancel(&self) -> bool {
        self.own.cancel()
    }

    /// 等待"父或自己"任一取消发生。
    pub async fn wait_cancelled(&self) {
        let parent = self.parent.clone();
        let own = self.own.rx.clone();
        tokio::select! {
            _ = wait_rx(parent) => {}
            _ = wait_rx(own) => {}
        }
    }
}

impl CancelToken for ChildCancel {
    fn is_cancelled(&self) -> bool {
        *self.parent.borrow() || self.own.is_cancelled()
    }
}

/// 把 [`CancelHandle`] 交给引擎时的共享句柄类型。
pub type SharedCancel = Arc<CancelHandle>;

/// **只观测**外部 `watch` 通道的取消令牌（W3-T2）。
///
/// 存在理由：宿主（`dispatch.rs`）的停止按钮走的是 `watch::Sender<bool>` 那条老链路
/// （`abort_senders` 表），而新引擎要的是 `CancelToken`。这个类型把两者桥起来——
/// 它**不持有触发端**，只如实反映通道当前值。
///
/// 为什么不给 `CancelHandle` 加一个 `from_watch(rx)`：`CancelHandle` 结构里持有
/// `watch::Sender<bool>` 才能 `cancel()`，而从一个 `Receiver` 造不出同一个通道的 `Sender`。
/// 硬塞会让"可触发"这个语义变得不成立（`cancel()` 会静默失效）。
#[derive(Clone)]
pub struct WatchedCancel {
    rx: watch::Receiver<bool>,
}

impl WatchedCancel {
    /// 观测一个已存在的取消通道。
    pub fn new(rx: watch::Receiver<bool>) -> Self {
        Self { rx }
    }

    /// 一个**永不取消**的令牌（没有中止通道时用，例如 `THREAD_EDIT_AND_RESEND`）。
    pub fn never() -> Self {
        let (_tx, rx) = watch::channel(false);
        // 故意丢弃 `_tx`：通道永不翻转 → 令牌永不取消。
        // `_tx` 一旦被 drop，`rx.changed()` 会立刻报错返回；`is_cancelled()` 仍恒为 false，
        // 这正是我们要的语义。
        Self { rx }
    }
}

impl CancelToken for WatchedCancel {
    fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_cancel_flips_once_and_is_idempotent() {
        let h = CancelHandle::new();
        assert!(!h.is_cancelled(), "新建令牌不应是已取消状态");
        assert!(h.cancel(), "首次取消应返回 true（本次真的翻转）");
        assert!(h.is_cancelled());
        assert!(!h.cancel(), "重复取消应返回 false");
        assert!(h.is_cancelled(), "取消必须单调：不得回到未取消");
    }

    #[tokio::test]
    async fn test_wait_cancelled_resolves_after_trigger() {
        let h = Arc::new(CancelHandle::new());
        let h2 = h.clone();
        let waiter = tokio::spawn(async move { h2.wait_cancelled().await });

        tokio::time::sleep(Duration::from_millis(20)).await;
        h.cancel();

        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("等待取消不应超时")
            .expect("等待任务不应 panic");
    }

    #[tokio::test]
    async fn test_wait_cancelled_returns_immediately_when_already_cancelled() {
        let h = CancelHandle::new();
        h.cancel();
        tokio::time::timeout(Duration::from_secs(1), h.wait_cancelled())
            .await
            .expect("已取消的令牌必须立即返回");
    }

    #[tokio::test]
    async fn test_subscribe_observes_flip() {
        let h = CancelHandle::new();
        let mut rx = h.subscribe();
        assert!(!*rx.borrow(), "订阅时不应是已取消");

        // Clone 共享同一通道：另一个持有者按停止键，订阅者必须看得到
        let h2 = h.clone();
        h2.cancel();

        tokio::time::timeout(Duration::from_secs(1), rx.changed())
            .await
            .expect("订阅者必须收到变更")
            .expect("发送端仍然存活");
        assert!(*rx.borrow_and_update(), "订阅者必须看到 true");
        assert!(h.is_cancelled(), "Clone 必须共享同一状态");
    }

    #[tokio::test]
    async fn test_child_inherits_parent_cancel_but_not_vice_versa() {        let parent = CancelHandle::new();
        let child = parent.child();

        assert!(!child.is_cancelled());

        // 子自己取消 → 父不受影响
        assert!(child.cancel());
        assert!(child.is_cancelled(), "子取消后子必须已取消");
        assert!(!parent.is_cancelled(), "子取消不得影响父");

        // 父取消 → 另一个子随之取消
        let child2 = parent.child();
        assert!(!child2.is_cancelled());
        parent.cancel();
        assert!(child2.is_cancelled(), "父取消后子必须随之取消");

        tokio::time::timeout(Duration::from_secs(1), child2.wait_cancelled())
            .await
            .expect("子等待取消必须在父取消后立即返回");
    }

    /// W3-T2：只观测的令牌必须**如实反映**外部通道，且永不"自己"翻转。
    #[test]
    fn test_watched_cancel_reflects_external_channel() {
        let (tx, rx) = watch::channel(false);
        let token = WatchedCancel::new(rx);
        assert!(!token.is_cancelled(), "通道初值 false → 不应已取消");

        tx.send_replace(true);
        assert!(token.is_cancelled(), "外部翻转后必须看得到");

        // Clone 共享同一通道
        let token2 = token.clone();
        assert!(token2.is_cancelled());
    }

    #[test]
    fn test_watched_cancel_never_is_never_cancelled() {
        let token = WatchedCancel::never();
        assert!(!token.is_cancelled());
        assert!(!token.is_cancelled(), "永不取消令牌必须恒定");
    }
}
