use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::cmd_tools::run_command;

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

            // 构造取消 watch 通道以传入现有的 run_command 内部机制
            let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
            if ctx.cancel.is_cancelled() {
                let _ = abort_tx.send(true);
            }

            let res = run_command(
                &self.workspace,
                command,
                cwd,
                timeout,
                Some(abort_rx),
            )
            .await;

            let finished_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(started_at);

            let status = if res.ok {
                ToolStatus::Success
            } else if res.output.contains("取消") || res.output.contains("aborted") {
                ToolStatus::Aborted
            } else if res.output.contains("超时") || res.output.contains("timed out") {
                ToolStatus::Timeout
            } else {
                ToolStatus::Error
            };

            ToolReceipt::new(status, res.output, started_at, finished_at)
        })
    }
}
