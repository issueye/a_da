use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

#[derive(Debug, Default)]
pub struct ApprovalManager {
    waiters: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// 注册等待前端用户审批决定的通道
    pub fn register_waiter(&self, tool_item_id: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        let mut map = self.waiters.lock().unwrap();
        map.insert(tool_item_id.to_string(), tx);
        rx
    }

    /// 接收到前端 `approval.decide` 请求，派发用户批准/拒绝决定
    pub fn resolve_approval(&self, tool_item_id: &str, approved: bool) -> bool {
        let mut map = self.waiters.lock().unwrap();
        if let Some(tx) = map.remove(tool_item_id) {
            let _ = tx.send(approved);
            true
        } else {
            false
        }
    }

    /// 检查某个工具调用卡片是否在等待用户决策
    pub fn has_pending(&self, tool_item_id: &str) -> bool {
        let map = self.waiters.lock().unwrap();
        map.contains_key(tool_item_id)
    }

    /// 撤销一个等待中的审批（超时/中止时清理）。
    ///
    /// 为什么需要它：`resolve_approval` 只在**前端真的答复**时移除条目；
    /// 超时或会话中止走的是别的分支，若不清理就会在 map 里留下永久泄漏的 waiter。
    /// 幂等：条目不存在时是空操作。
    pub fn remove_waiter(&self, tool_item_id: &str) {
        self.waiters.lock().unwrap().remove(tool_item_id);
    }

    /// 当前等待中的审批数量（诊断/测试用）。
    pub fn pending_count(&self) -> usize {
        self.waiters.lock().unwrap().len()
    }

    /// 清理并拒绝所有等待中的审批（例如会话中止时）
    pub fn cancel_all(&self) {
        let mut map = self.waiters.lock().unwrap();
        for (_, tx) in map.drain() {
            let _ = tx.send(false);
        }
    }
}
