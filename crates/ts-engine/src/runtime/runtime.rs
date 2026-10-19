use std::sync::Arc;
use std::sync::mpsc::Sender;

use tokio::sync::oneshot;

use super::console::{ConsoleCapture, ConsoleMode, ConsoleSnapshot};
use super::limits::Limits;
use super::messages::EventLoopMsg;
use super::state::AsyncHandler;
use super::worker;

/// 纯 Rust 实现的 TypeScript 微内核运行时
#[derive(Clone)]
pub struct PureTsRuntime {
    pub(super) sender: Sender<EventLoopMsg>,
    /// 控制台捕获（每运行时一份；线程局部指向同一个 `Arc`）。
    pub(super) console: std::sync::Arc<std::sync::Mutex<ConsoleCapture>>,
}

impl PureTsRuntime {
    /// 初始化并启动专有事件循环 Actor（使用默认工作区）
    pub fn new() -> Self {
        Self::with_workspace(None)
    }

    /// 初始化并指定工作区绝对路径
    pub fn with_workspace(workspace: Option<std::path::PathBuf>) -> Self {
        Self::with_workspace_and_limits(workspace, Limits::default())
    }

    /// 初始化并指定工作区与运行限额（宿主按预算给限额时用这个）。
    pub fn with_workspace_and_limits(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
    ) -> Self {
        Self::with_environment(workspace, limits, crate::env::Needs::everything())
    }

    /// 初始化并指定工作区、限额与**环境注入清单**（P1 · SP-13）。
    ///
    /// 这是插件路径该用的构造器：`needs` 里没声明的能力**连注入都不做**
    /// （既没有 JS 包装，也没有挂到 `globalThis` 的原生函数）。
    pub fn with_environment(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
    ) -> Self {
        // 老签名保持"照旧打印"（CLI/本地脚本口径），插件路径用下面的显式版本
        Self::with_environment_and_console(workspace, limits, needs, ConsoleMode::Echo)
    }

    /// 同 [`Self::with_environment`]，但显式指定控制台模式（P1 · SP-14）。
    pub fn with_environment_and_console(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
        console_mode: ConsoleMode,
    ) -> Self {
        Self::with_environment_console_and_net(
            workspace,
            limits,
            needs,
            console_mode,
            // 宿主/运维给的策略（P4·SP-72）：环境变量读不到就用默认值
            crate::net::NetPolicy::from_env(),
        )
    }

    /// 同 [`Self::with_environment_and_console`]，但**联网策略由宿主给**（P4·SP-72）：
    /// 超时与域名白名单。默认档见 [`crate::net::NetPolicy`]。
    pub fn with_environment_console_and_net(
        workspace: Option<std::path::PathBuf>,
        limits: Limits,
        needs: crate::env::Needs,
        console_mode: ConsoleMode,
        net: crate::net::NetPolicy,
    ) -> Self {
        worker::build(workspace, limits, needs, console_mode, net)
    }
    /// 执行一段 TypeScript 代码并等待最终字符串返回
    pub async fn eval_ts(
        &self,
        ts_code: impl Into<String>,
        filename: Option<&str>,
    ) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(EventLoopMsg::Execute {
                source_code: ts_code.into(),
                filename: filename.map(|s| s.to_string()),
                response_tx: tx,
            })
            .map_err(|e| e.to_string())?;

        rx.await.map_err(|e| format!("事件循环挂起无响应: {e}"))?
    }

    /// 取走本运行时**累计**的控制台输出（P1·SP-14）。
    ///
    /// 取走即清空：调用方按自己的归属口径上报（插件路径按 `plugin_id`）。
    pub fn take_console(&self) -> ConsoleSnapshot {
        match self.console.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => ConsoleSnapshot::default(),
        }
    }

    /// 等待当前所有未决异步任务排空（空闲）
    pub async fn wait_idle(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(EventLoopMsg::WaitIdle { response_tx: tx })
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| format!("等待空闲被中断: {e}"))
    }

    /// 设置微内核中 process.argv 命令行参数列表
    pub async fn set_process_argv(&self, argv: Vec<String>) -> Result<(), String> {
        let json = serde_json::to_string(&argv).map_err(|e| e.to_string())?;
        self.eval_ts(
            format!("if (typeof process !== 'undefined') {{ process.argv = {json}; }}"),
            None,
        )
        .await?;
        Ok(())
    }

    /// 向微内核注册一个返回 Promise 的 Tokio 异步原生函数
    pub fn register_async_fn<F, Fut>(&self, name: &'static str, async_logic: F)
    where
        F: Fn(Vec<String>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let handler: AsyncHandler = Arc::new(move |args| Box::pin(async_logic(args)));
        let _ = self
            .sender
            .send(EventLoopMsg::RegisterAsyncFn { name, handler });
    }

    /// 按声明注册异步原生：`allowed == false` 时**连全局函数都不注册**。
    ///
    /// 🔴 为什么必须裁到这一层：异步原生是经 `register_global_callable` 挂到 `globalThis`
    /// 上的（`runtime/mod.rs` 的 `RegisterAsyncFn` 处理处）。只关 JS 侧的包装脚本挡不住
    /// `globalThis.__native_fetch_async(...)` 这种**直呼**——那正是"注入后再拦"的漏法。
    pub fn register_async_fn_gated<F, Fut>(&self, allowed: bool, name: &'static str, async_logic: F)
    where
        F: Fn(Vec<String>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        if allowed {
            self.register_async_fn(name, async_logic);
        }
    }

    /// 调用在微内核中注册的全局异步 TypeScript/JavaScript 工具函数
    pub async fn call_async_fn(
        &self,
        fn_name: impl Into<String>,
        args_json: impl Into<String>,
    ) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(EventLoopMsg::CallAsyncFunction {
                fn_name: fn_name.into(),
                args_json: args_json.into(),
                response_tx: tx,
            })
            .map_err(|e| e.to_string())?;

        rx.await
            .map_err(|e| format!("异步工具执行响应被中断: {e}"))?
    }

    /// 终止专有事件循环并释放后台线程资源
    pub fn terminate(&self) {
        let _ = self.sender.send(EventLoopMsg::Terminate);
    }
}

impl Default for PureTsRuntime {
    fn default() -> Self {
        Self::new()
    }
}
