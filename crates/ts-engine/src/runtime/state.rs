use boa_engine::builtins::promise::ResolvingFunctions;
use futures_util::future::BoxFuture;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::Sender;
use tokio::sync::oneshot;

use super::messages::EventLoopMsg;

pub(super) static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// 仅限专有事件循环单线程内部持有的未决 Promise 解析器映射表（零跨线程移动，完美避开 !Send）
    pub(super) static PENDING_PROMISES: RefCell<HashMap<u64, ResolvingFunctions>> = RefCell::new(HashMap::new());
    /// 异步工具调用挂起的返回通道映射表
    pub(super) static PENDING_TOOL_CALLS: RefCell<HashMap<u64, oneshot::Sender<Result<String, String>>>> = RefCell::new(HashMap::new());
    /// 当前事件循环单线程持有的内部消息发送通道
    pub(super) static WORKER_TX: RefCell<Option<Sender<EventLoopMsg>>> = RefCell::new(None);
    /// 当前事件循环持有的异步处理逻辑
    pub(super) static ASYNC_HANDLERS: RefCell<HashMap<&'static str, AsyncHandler>> = RefCell::new(HashMap::new());
    /// 当前关联的 Tokio 运行时调度句柄
    pub(super) static TOKIO_HANDLE: RefCell<Option<tokio::runtime::Handle>> = RefCell::new(None);
    /// 等待所有未决任务排空的回调通道队列
    pub(super) static PENDING_IDLE_WAITERS: RefCell<Vec<oneshot::Sender<()>>> = RefCell::new(Vec::new());
}

pub(super) type AsyncHandler =
    Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, Result<String, String>> + Send + Sync + 'static>;
