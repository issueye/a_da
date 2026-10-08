//! 会话存储端口：会话流水消息的加载与追加。
//!
//! 规则（设计 §3.2、INV-2、INV-8）：
//! 零 IO、无默认实现。引擎不关心落盘是 JSONL 还是 SQLite，只关心按顺序存取领域消息。

use crate::domain::{AgentError, AgentMessage};
use crate::ports::tools::BoxFuture;

pub trait SessionStore: Send + Sync {
    /// 加载指定会话（thread_id）的所有历史消息。若会话不存在返回空列表。
    fn load_messages<'a>(
        &'a self,
        thread_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AgentMessage>, AgentError>>;

    /// 向指定会话（thread_id）末尾追加单条领域消息。
    fn append_message<'a>(
        &'a self,
        thread_id: &'a str,
        message: &'a AgentMessage,
    ) -> BoxFuture<'a, Result<(), AgentError>>;
}
