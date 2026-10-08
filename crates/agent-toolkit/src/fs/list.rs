use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::list_files;

/// `list_files` 工具：列出工作区目录树结构
pub struct ListFilesTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl ListFilesTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要列出的目录相对路径（默认为工作区根目录）"
                },
                "depth": {
                    "type": "integer",
                    "description": "遍历最大深度（默认 3 层）"
                }
            }
        });

        Self {
            descriptor: ToolDescriptor {
                name: "list_files".to_string(),
                summary: "按层级列出工作区目录树中的文件与文件夹结构。".to_string(),
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

impl Tool for ListFilesTool {
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

            let path = call.args.get("path").and_then(|v| v.as_str());
            let depth = call.args.get("depth").and_then(|v| v.as_u64()).map(|n| n as usize);

            let res = list_files(&self.workspace, path, depth);
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
