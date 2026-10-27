use std::path::PathBuf;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::write_file;

/// `write_file` 工具：全量覆写或创建新文件
pub struct WriteFileTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("write_file")
            .expect("`write_file` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();

        Self {
            descriptor,
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
            let started_at = crate::now_ms();

            if ctx.cancel.is_cancelled() {
                let finished_at = started_at;
                return ToolReceipt::new(ToolStatus::Aborted, "操作已取消", started_at, finished_at);
            }

            let path = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = call.args.get("content").and_then(|v| v.as_str()).unwrap_or("");

            let paths = crate::fs_tools::ToolPaths::new(&self.workspace);
            let res = write_file(paths, path, content);
            let finished_at = crate::now_ms().max(started_at);

            if res.ok {
                ToolReceipt::success(res.output, started_at, finished_at)
            } else {
                ToolReceipt::error(res.output, started_at, finished_at)
            }
        })
    }
}
