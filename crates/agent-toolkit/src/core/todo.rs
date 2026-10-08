use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

/// `todo` 工具：管理与记录任务清单。
pub struct TodoTool {
    descriptor: ToolDescriptor,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoTool {
    pub fn new() -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get", "set", "update"],
                    "description": "操作类型：get (获取待办), set (重置待办), update (更新项)"
                },
                "items": {
                    "type": "array",
                    "description": "待办项列表",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "title": { "type": "string" },
                            "done": { "type": "boolean" }
                        },
                        "required": ["title"]
                    }
                }
            }
        });

        Self {
            descriptor: ToolDescriptor {
                name: "todo".to_string(),
                summary: "记录或查看当前会话的待办任务与进度清单。".to_string(),
                schema,
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        }
    }
}

impl Tool for TodoTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let action = call
                .args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("get");

            let output = match action {
                "get" => "当前待办列表已同步就绪。".to_string(),
                "set" | "update" => "待办清单状态已更新。".to_string(),
                other => format!("未知待办操作: {}", other),
            };

            let finished_at = started_at;
            ToolReceipt::success(output, started_at, finished_at)
        })
    }
}
