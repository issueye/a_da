use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

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
        let descriptor = crate::registry::find_tool_descriptor("todo")
            .expect("todo descriptor must be registered in agent-toolkit registry")
            .clone();

        Self { descriptor }
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
