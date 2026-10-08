use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::search_files;

/// `search_files` 工具：在工作区文件内正则或字面量搜索内容
pub struct SearchFilesTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl SearchFilesTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "搜索文本或正则表达式模式"
                },
                "glob": {
                    "type": "string",
                    "description": "文件通配符模式（例如 **/*.rs，可选）"
                },
                "path": {
                    "type": "string",
                    "description": "限制搜索的子目录相对路径（可选）"
                },
                "literal": {
                    "type": "boolean",
                    "description": "是否进行纯字面量匹配（默认 false）"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "是否区分大小写（默认 false）"
                },
                "context": {
                    "type": "integer",
                    "description": "匹配行前后的上下文行数（默认 0）"
                }
            },
            "required": ["pattern"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "search_files".to_string(),
                summary: "在工作区文件内搜索匹配的字符串或正则表达式内容。".to_string(),
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

impl Tool for SearchFilesTool {
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

            let pattern = call.args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let glob = call.args.get("glob").and_then(|v| v.as_str());
            let path = call.args.get("path").and_then(|v| v.as_str());
            let literal = call.args.get("literal").and_then(|v| v.as_bool()).unwrap_or(false);
            let case_sensitive = call.args.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(false);
            let context = call.args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            let res = search_files(&self.workspace, pattern, glob, path, literal, case_sensitive, context);
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
