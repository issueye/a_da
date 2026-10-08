use std::path::PathBuf;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolCall,
    ToolDescriptor, ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

use crate::fs_tools::{edit_file, write_file};

/// `batch_write` 工具：原子批量写入多个文件
pub struct BatchWriteTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

impl BatchWriteTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "description": "待写入的文件列表",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "相对路径" },
                            "content": { "type": "string", "description": "文本内容" }
                        },
                        "required": ["path", "content"]
                    }
                }
            },
            "required": ["files"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "batch_write".to_string(),
                summary: "在工作区内批量写入多个文件。".to_string(),
                schema,
                access: Access::Mutates {
                    paths: PathSelector::Batch("files"),
                },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
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
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

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
                        let fin = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(started_at);
                        return ToolReceipt::new(ToolStatus::Aborted, "操作已中止", started_at, fin);
                    }
                    if let (Some(p), Some(c)) = (
                        f.get("path").and_then(|v| v.as_str()),
                        f.get("content").and_then(|v| v.as_str()),
                    ) {
                        let res = write_file(&self.workspace, p, c);
                        if !res.ok {
                            let fin = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(started_at);
                            return ToolReceipt::error(
                                format!("写入 [{}] 失败: {}", p, res.output),
                                started_at,
                                fin,
                            );
                        }
                        written += 1;
                    }
                }
                finished_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(started_at);
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
        let schema = json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "description": "待替换的目标文件路径列表",
                    "items": { "type": "string" }
                },
                "old_string": { "type": "string", "description": "被替换的原字符串" },
                "new_string": { "type": "string", "description": "替换后的新字符串" }
            },
            "required": ["files", "old_string", "new_string"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "batch_replace".to_string(),
                summary: "在多个目标文件中批量进行严格字符串替换。".to_string(),
                schema,
                access: Access::Mutates {
                    paths: PathSelector::Batch("files"),
                },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
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
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

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
                        let fin = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(started_at);
                        return ToolReceipt::new(ToolStatus::Aborted, "操作已中止", started_at, fin);
                    }
                    if let Some(p) = f.as_str() {
                        let res = edit_file(&self.workspace, p, Some(old_s), Some(new_s), None);
                        if res.ok {
                            replaced += 1;
                        }
                    }
                }
                finished_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(started_at);
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
