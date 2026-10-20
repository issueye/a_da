use std::path::PathBuf;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::{edit_file, write_file};

/// `batch_write` 工具：原子批量写入多个文件
pub struct BatchWriteTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl BatchWriteTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("batch_write")
            .expect("`batch_write` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();

        Self {
            descriptor,
            workspace: workspace.into(),
        }
    }
}

impl Tool for BatchWriteTool {
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

            let files = call.args.get("files").and_then(|v| v.as_array());
            let finished_at;

            if let Some(list) = files {
                let mut written = 0;
                for f in list {
                    if ctx.cancel.is_cancelled() {
                        let fin = crate::now_ms().max(started_at);
                        return ToolReceipt::new(ToolStatus::Aborted, "操作已中止", started_at, fin);
                    }
                    if let (Some(p), Some(c)) = (
                        f.get("path").and_then(|v| v.as_str()),
                        f.get("content").and_then(|v| v.as_str()),
                    ) {
                        let gate = crate::fs_tools::PathGate::scoped(ctx.scope, &self.workspace);
                        let res = write_file(gate, p, c);
                        if !res.ok {
                            let fin = crate::now_ms().max(started_at);
                            return ToolReceipt::error(
                                format!("写入 [{}] 失败: {}", p, res.output),
                                started_at,
                                fin,
                            );
                        }
                        written += 1;
                    }
                }
                finished_at = crate::now_ms().max(started_at);
                ToolReceipt::success(format!("成功原子写入 {} 个文件。", written), started_at, finished_at)
            } else {
                finished_at = started_at;
                ToolReceipt::error("缺少 files 参数", started_at, finished_at)
            }
        })
    }
}

/// `batch_replace` 工具：在多个文件中批量替换相同文本
pub struct BatchReplaceTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl BatchReplaceTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("batch_replace")
            .expect("`batch_replace` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();

        Self {
            descriptor,
            workspace: workspace.into(),
        }
    }
}

impl Tool for BatchReplaceTool {
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

            let files = call.args.get("files").and_then(|v| v.as_array());
            let old_str = call.args.get("old_string").and_then(|v| v.as_str());
            let new_str = call.args.get("new_string").and_then(|v| v.as_str());

            let finished_at;
            if let (Some(list), Some(old_s), Some(new_s)) = (files, old_str, new_str) {
                let mut replaced = 0;
                for f in list {
                    if ctx.cancel.is_cancelled() {
                        let fin = crate::now_ms().max(started_at);
                        return ToolReceipt::new(ToolStatus::Aborted, "操作已中止", started_at, fin);
                    }
                    if let Some(p) = f.as_str() {
                        let gate = crate::fs_tools::PathGate::scoped(ctx.scope, &self.workspace);
                        let res = edit_file(gate, p, Some(old_s), Some(new_s), None);
                        if res.ok {
                            replaced += 1;
                        }
                    }
                }
                finished_at = crate::now_ms().max(started_at);
                ToolReceipt::success(
                    format!("批量替换完成，在 {} 个文件中生效。", replaced),
                    started_at,
                    finished_at,
                )
            } else {
                finished_at = started_at;
                ToolReceipt::error("缺少 files / old_string / new_string 参数", started_at, finished_at)
            }
        })
    }
}
