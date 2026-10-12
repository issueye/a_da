use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

/// `finish` 工具：主动声明本回合工作完成，附带总结。
/// 关键属性：`termination: Termination::EndTurn`，触发基座引擎提早结束本轮。
pub struct FinishTool {
    descriptor: ToolDescriptor,
}

impl Default for FinishTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FinishTool {
    pub fn new() -> Self {
        let descriptor = crate::registry::find_tool_descriptor("finish")
            .expect("finish descriptor must be registered in agent-toolkit registry")
            .clone();

        Self { descriptor }
    }
}

impl Tool for FinishTool {
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

            let summary = call
                .args
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("任务已完成。");

            let finished_at = started_at;
            ToolReceipt::success(summary, started_at, finished_at)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::Termination;

    #[test]
    fn test_finish_tool_has_end_turn_termination() {
        let tool = FinishTool::new();
        assert_eq!(tool.descriptor().termination, Termination::EndTurn);
        assert!(tool.descriptor().is_readonly());
    }
}
