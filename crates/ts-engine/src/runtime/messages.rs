use tokio::sync::oneshot;

use super::state::AsyncHandler;
/// 专有事件循环 Actor 的消息载荷（保证 100% Send）
pub enum EventLoopMsg {
    /// 执行 TypeScript 代码并异步返回结果字符串
    Execute {
        source_code: String,
        filename: Option<String>,
        response_tx: oneshot::Sender<Result<String, String>>,
    },
    /// Tokio 异步任务完成后的纯数据结果回传
    AsyncJobDone {
        job_id: u64,
        result: Result<String, String>,
    },
    /// 注册一个全局异步原生函数
    RegisterAsyncFn {
        name: &'static str,
        handler: AsyncHandler,
    },
    /// 调用全局异步 JavaScript/TypeScript 工具函数并等待异步完成
    CallAsyncFunction {
        fn_name: String,
        args_json: String,
        response_tx: oneshot::Sender<Result<String, String>>,
    },
    /// 等待当前所有未决 Promise 与微任务全部完成（达到空闲状态）
    WaitIdle { response_tx: oneshot::Sender<()> },
    /// 终止专有事件循环
    Terminate,
}
