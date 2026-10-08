use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::read_file;

/// `read_file` 工具：安全读取指定路径的文件内容
pub struct ReadFileTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要读取的文件相对路径"
                },
                "offset": {
                    "type": "integer",
                    "description": "起始行号（从 1 开始，可选）"
                },
                "limit": {
                    "type": "integer",
                    "description": "最大读取行数（可选）"
                }
            },
            "required": ["path"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "read_file".to_string(),
                summary: "安全读取工作区指定文件的全部或部分行内容。".to_string(),
                schema,
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            workspace: workspace.into(),
        }
    }
}

impl Tool for ReadFileTool {
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
                return ToolReceipt::new(ToolStatus::Aborted, "操作已取消", started_at, finished_at);
            }

            let path = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let offset = call.args.get("offset").and_then(|v| v.as_u64()).map(|n| n as usize);
            let limit = call.args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);

            let res = read_file(&self.workspace, path, offset, limit);
            let finished_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(started_at);

            if res.ok {
                ToolReceipt::success(res.output, started_at, finished_at)
            } else {
                ToolReceipt::error(res.output, started_at, finished_at)
            }
        })
    }
}
