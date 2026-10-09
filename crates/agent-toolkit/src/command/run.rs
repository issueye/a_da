use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::cmd_tools::run_command;
use crate::fs_tools::ToolFailure;

/// `run_command` 工具：在工作区安全执行命令行指令并支持取消中断与超时
pub struct RunCommandTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl RunCommandTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的命令行指令字符串"
                },
                "cwd": {
                    "type": "string",
                    "description": "子目录路径（相对工作区根目录，可选）"
                },
                "timeout": {
                    "type": "integer",
                    "description": "命令超时等待时间（秒，默认 30）"
                }
            },
            "required": ["command"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "run_command".to_string(),
                summary: "在系统底层 shell 中执行指定命令行，并捕获标准输出、错误与退出状态。".to_string(),
                schema,
                access: Access::Executes {
                    command_arg: "command",
                },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            workspace: workspace.into(),
        }
    }
}

impl Tool for RunCommandTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            if ctx.cancel.is_cancelled() {
                let finished_at = started_at;
                return ToolReceipt::new(ToolStatus::Aborted, "命令在执行前已取消", started_at, finished_at);
            }

            let command = call.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = call.args.get("cwd").and_then(|v| v.as_str());
            let timeout = call.args.get("timeout").and_then(|v| v.as_u64());

            // W4-T1：把轮次取消**真的转发**到命令的中止通道。
            //
            // 原实现是"新建 watch 通道 → 若已取消就 send(true)"，而上面第 73 行已经
            // 在"已取消"时提前 return 了——那两行是**死代码**：命令一旦开始执行，
            // `ctx.cancel` 就再也没人看，用户按停止键对正在跑的命令**完全无效**。
            //
            // 这里改成在执行期间**轮询转发**：`CancelToken` 只有轮询式 `is_cancelled()`，
            // 所以用一个 select 分支每 50ms 看一眼，翻转后 `run_command` 内部的
            // select 会立刻 `kill_process_tree`。
            let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
            let run_fut = run_command(&self.workspace, command, cwd, timeout, Some(abort_rx));
            tokio::pin!(run_fut);

            let res = loop {
                tokio::select! {
                    r = &mut run_fut => break r,
                    _ = tokio::time::sleep(crate::cmd_tools::CANCEL_POLL_INTERVAL) => {
                        if ctx.cancel.is_cancelled() {
                            let _ = abort_tx.send(true);
                        }
                    }
                }
            };

            let finished_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(started_at);

            // W4-T2：状态来自**结构化原因**，不再解析输出措辞
            // （原实现靠 `output.contains("取消")` / `contains("超时")` 猜——
            //  换个措辞或本地化就会静默把 `Aborted` 判成 `Error`）。
            let status = match res.failure() {
                None if res.ok => ToolStatus::Success,
                Some(ToolFailure::Aborted) => ToolStatus::Aborted,
                Some(ToolFailure::Timeout) => ToolStatus::Timeout,
                Some(_) => ToolStatus::Error,
                None => ToolStatus::Error,
            };

            ToolReceipt::new(status, res.output, started_at, finished_at)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::AgentEvent;
    use agent_base::ports::{CancelToken, EventSink, Scope};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct NoopSink;
    impl EventSink for NoopSink {
        fn emit(&self, _event: AgentEvent) {}
    }
    struct RootScope(PathBuf);
    impl Scope for RootScope {
        fn id(&self) -> &str {
            "test_root"
        }
        fn resolve_path(&self, raw: &str) -> Result<PathBuf, agent_base::domain::DenialKind> {
            Ok(self.0.join(raw))
        }
    }
    /// 可外部翻转的取消令牌（测试替身；生产用 `agent-adapter` 的 `CancelHandle`）。
    struct FlagCancel(Arc<AtomicBool>);
    impl CancelToken for FlagCancel {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn call(args: serde_json::Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: "run_command".into(), args }
    }

    fn sleep_cmd() -> &'static str {
        if cfg!(windows) {
            "ping 127.0.0.1 -n 20 > nul"
        } else {
            "sleep 20"
        }
    }

    #[tokio::test]
    async fn test_success_and_non_zero_exit_are_distinguished_structurally() {
        let ws = std::env::current_dir().unwrap();
        let tool = RunCommandTool::new(&ws);
        let flag = Arc::new(AtomicBool::new(false));
        let (scope, sink) = (RootScope(ws.clone()), NoopSink);
        let cancel = FlagCancel(flag);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let ok = tool.execute(&call(json!({ "command": "echo hello" })), &ctx).await;
        assert_eq!(ok.status, ToolStatus::Success, "{}", ok.output);

        let bad = if cfg!(windows) {
            tool.execute(&call(json!({ "command": "exit 3" })), &ctx).await
        } else {
            tool.execute(&call(json!({ "command": "exit 3" })), &ctx).await
        };
        assert_eq!(bad.status, ToolStatus::Error, "非零退出应为 Error：{}", bad.output);
    }

    /// W4-T2 守门：**输出里出现关键词不等于那种失败**。
    ///
    /// 原实现用 `output.contains("aborted")` 判 `Aborted`——这条命令**成功**却会输出
    /// `aborted`，靠子串猜就会把成功判成"被取消"。现在状态来自 `ToolFailure`，不会误判。
    #[tokio::test]
    async fn test_status_does_not_come_from_output_substrings() {
        let ws = std::env::current_dir().unwrap();
        let tool = RunCommandTool::new(&ws);
        let flag = Arc::new(AtomicBool::new(false));
        let (scope, sink) = (RootScope(ws.clone()), NoopSink);
        let cancel = FlagCancel(flag);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        // 成功，但输出含 "aborted"（旧判定会误报 Aborted）
        let r = tool
            .execute(&call(json!({ "command": "echo aborted" })), &ctx)
            .await;
        assert!(
            r.output.contains("aborted"),
            "前置条件：输出确实含关键词：{}",
            r.output
        );
        assert_eq!(
            r.status,
            ToolStatus::Success,
            "输出含 'aborted' 不代表命令被取消（W4-T2）"
        );
    }

    /// W4-T1 守门：**执行中**取消必须真的杀掉命令，而不是等它跑完。
    ///
    /// 原实现只在"已取消"时发一次中止信号（而那之前早已 return）——执行期间
    /// `ctx.cancel` 无人观察，用户按停止键对正在跑的命令完全无效。
    #[tokio::test]
    async fn test_cancel_during_execution_actually_kills_the_command() {
        let ws = std::env::current_dir().unwrap();
        let tool = RunCommandTool::new(&ws);
        let flag = Arc::new(AtomicBool::new(false));
        let (scope, sink) = (RootScope(ws.clone()), NoopSink);
        let cancel = FlagCancel(flag.clone());
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        // 100ms 后按"停止键"
        let f = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            f.store(true, Ordering::SeqCst);
        });

        let started = Instant::now();
        let r = tool
            .execute(&call(json!({ "command": sleep_cmd(), "timeout": 30 })), &ctx)
            .await;
        let elapsed = started.elapsed();

        assert_eq!(
            r.status,
            ToolStatus::Aborted,
            "执行中取消必须产出 Aborted：{}",
            r.output
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "取消必须**立刻**生效，而不是等命令自己跑完（实际 {elapsed:?}）"
        );
    }

    /// 超时必须产出结构化的 `Timeout`（而不是靠输出措辞）。
    #[tokio::test]
    async fn test_timeout_is_structurally_reported() {
        let ws = std::env::current_dir().unwrap();
        let tool = RunCommandTool::new(&ws);
        let flag = Arc::new(AtomicBool::new(false));
        let (scope, sink) = (RootScope(ws.clone()), NoopSink);
        let cancel = FlagCancel(flag);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = tool
            .execute(&call(json!({ "command": sleep_cmd(), "timeout": 1 })), &ctx)
            .await;
        assert_eq!(r.status, ToolStatus::Timeout, "{}", r.output);
    }

    /// 已取消时**不启动**子进程（保持原有短路语义）。
    #[tokio::test]
    async fn test_already_cancelled_short_circuits_before_spawn() {
        let ws = std::env::current_dir().unwrap();
        let tool = RunCommandTool::new(&ws);
        let flag = Arc::new(AtomicBool::new(true));
        let (scope, sink) = (RootScope(ws.clone()), NoopSink);
        let cancel = FlagCancel(flag);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = tool.execute(&call(json!({ "command": "echo 不该执行" })), &ctx).await;
        assert_eq!(r.status, ToolStatus::Aborted);
        assert!(r.output.contains("执行前"), "{}", r.output);
    }
}
