use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolCall,
    ToolDescriptor, ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::{edit_file, EditPair};

/// `edit_file` 工具：精确定位替换文件中的部分内容
pub struct EditFileTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl EditFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要编辑的目标文件相对路径"
                },
                "old_string": {
                    "type": "string",
                    "description": "被替换的原始内容块（单块替换模式）"
                },
                "new_string": {
                    "type": "string",
                    "description": "替换后的新内容块（单块替换模式）"
                },
                "edits": {
                    "type": "array",
                    "description": "多处批量替换列表（批量模式）",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" }
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["path"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "edit_file".to_string(),
                summary: "通过严格文本匹配替换文件中的指定片段，支持单块与多块替换。".to_string(),
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

impl Tool for EditFileTool {
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
            let old_string = call.args.get("old_string").and_then(|v| v.as_str());
            let new_string = call.args.get("new_string").and_then(|v| v.as_str());

            let edits = call.args.get("edits").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let o = item.get("old_string").and_then(|v| v.as_str())?;
                        let n = item.get("new_string").and_then(|v| v.as_str())?;
                        Some(EditPair {
                            old_string: o.to_string(),
                            new_string: n.to_string(),
                        })
                    })
                    .collect()
            });

            let res = edit_file(&self.workspace, path, old_string, new_string, edits);
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
