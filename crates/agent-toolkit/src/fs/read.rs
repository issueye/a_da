use std::path::PathBuf;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::read_file;

/// `read_file` 工具：安全读取指定路径的文件内容
pub struct ReadFileTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("read_file")
            .expect("`read_file` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();

        Self {
            descriptor,
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
            let started_at = crate::now_ms();

            if ctx.cancel.is_cancelled() {
                let finished_at = started_at;
                return ToolReceipt::new(ToolStatus::Aborted, "操作已取消", started_at, finished_at);
            }

            let path = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let offset = call.args.get("offset").and_then(|v| v.as_u64()).map(|n| n as usize);
            let limit = call.args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);

            let gate = crate::fs_tools::PathGate::scoped(ctx.scope, &self.workspace);
            let res = read_file(gate, path, offset, limit);
            let finished_at = crate::now_ms().max(started_at);

            if res.ok {
                ToolReceipt::success(res.output, started_at, finished_at)
            } else {
                ToolReceipt::error(res.output, started_at, finished_at)
            }
        })
    }
}
