use std::path::PathBuf;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::search_files;

/// `search_files` 工具：在工作区文件内正则或字面量搜索内容
pub struct SearchFilesTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl SearchFilesTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("search_files")
            .expect("`search_files` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();

        Self {
            descriptor,
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
            let started_at = crate::now_ms();

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

            let paths = crate::fs_tools::ToolPaths::new(&self.workspace);
            let res = search_files(paths, pattern, glob, path, literal, case_sensitive, context);
            let finished_at = crate::now_ms().max(started_at);

            if res.ok {
                ToolReceipt::success(res.output, started_at, finished_at)
            } else {
                ToolReceipt::error(res.output, started_at, finished_at)
            }
        })
    }
}
