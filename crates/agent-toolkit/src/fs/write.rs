use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolCall,
    ToolDescriptor, ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::write_file;

/// `write_file` 工具：全量覆写或创建新文件
pub struct WriteFileTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要写入的目标文件相对路径"
                },
                "content": {
                    "type": "string",
                    "description": "要写入的完整文本内容"
                }
            },
            "required": ["path", "content"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "write_file".to_string(),
                summary: "在工作区内全量覆写指定文件，若文件不存在则自动创建。".to_string(),
                schema,
                access: Access::Mutates {
                    paths: PathSelector::Single("path"),
                },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            workspace: workspace.into(),
        }
    }
}

impl Tool for WriteFileTool {
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
            let content = call.args.get("content").and_then(|v| v.as_str()).unwrap_or("");

            let res = write_file(&self.workspace, path, content);
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
