use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;

/// `ask_user` 工具：在交互界面向用户发起单选选择或补充说明提问并阻塞等待答复。
pub struct AskUserTool {
    descriptor: ToolDescriptor,
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AskUserTool {
    pub fn new() -> Self {
        let schema = json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "向用户提出的问题（限制 600 字以内，简明扼要）"
                },
                "choices": {
                    "type": "array",
                    "description": "提供给用户的单选选项列表（最多 6 项）",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "选项标识 ID（如不填自动生成 c1, c2 等）"
                            },
                            "label": {
                                "type": "string",
                                "description": "显示给用户的选项文案"
                            },
                            "description": {
                                "type": "string",
                                "description": "选项说明或辅助解释"
                            }
                        },
                        "required": ["label"]
                    }
                },
                "allow_text": {
                    "type": "boolean",
                    "description": "是否允许用户输入自定义文本（默认为 true）"
                }
            },
            "required": ["question"]
        });

        Self {
            descriptor: ToolDescriptor {
                name: "ask_user".to_string(),
                summary: "在交互界面向用户发起单选选择或补充说明提问并阻塞等待答复。".to_string(),
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

impl Tool for AskUserTool {
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

            // 1. 参数校验与对齐
            let question = call
                .args
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();

            if question.is_empty() {
                let finished_at = started_at;
                return ToolReceipt::error("缺少 question 参数：要问用户什么？", started_at, finished_at);
            }

            if question.chars().count() > 600 {
                let finished_at = started_at;
                return ToolReceipt::error(
                    "问题太长（上限 600 字）。请把背景压缩成几句后再问。",
                    started_at,
                    finished_at,
                );
            }

            // 2. 取消检查
            if ctx.cancel.is_cancelled() {
                let finished_at = started_at;
                return ToolReceipt::new(
                    ToolStatus::Aborted,
                    "操作在发起提问前已中止。",
                    started_at,
                    finished_at,
                );
            }

            // 3. 通道检查：无交互通道时报错而非永久挂起（验收门 M2-T2）
            // 在 ToolContext 中目前只有 scope/cancel/events/thread_id，
            // 纯基座无外接问答管理器时，明确返回环境不支持交互提问的错误。
            let finished_at = started_at;
            ToolReceipt::error(
                "当前环境未接入交互提问协调器（无事件问答通道），无法向用户提问。请直接基于现有上下文作出合理推断并继续。",
                started_at,
                finished_at,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::DenialKind;
    use agent_base::testing::{ManualCancel, RecordingSink};
    use agent_base::ports::Scope;
    use std::path::PathBuf;

    struct DummyScope;
    impl Scope for DummyScope {
        fn id(&self) -> &str {
            "dummy"
        }
        fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
            Ok(PathBuf::from(raw))
        }
    }

    #[tokio::test]
    async fn test_ask_user_schema_and_parameter_alignment() {
        let tool = AskUserTool::new();
        let desc = tool.descriptor();
        assert_eq!(desc.name, "ask_user");
        assert!(desc.is_readonly());

        // 验证参数 Schema 对齐
        let schema = &desc.schema;
        assert_eq!(schema["type"], "object");
        let props = &schema["properties"];
        assert!(props.get("question").is_some());
        assert!(props.get("choices").is_some());
        assert!(props.get("allow_text").is_some());
        assert_eq!(schema["required"], json!(["question"]));
    }

    #[tokio::test]
    async fn test_ask_user_without_channel_fails_instead_of_hanging() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
        };

        let call = ToolCall {
            id: "call_1".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "question": "您希望使用哪种方案？",
                "choices": [{"label": "方案 A"}, {"label": "方案 B"}]
            }),
        };

        // 验证无通道时立即返回错误，绝不 hang 住
        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("当前环境未接入交互提问协调器"));
    }

    #[tokio::test]
    async fn test_ask_user_empty_or_too_long_question() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
        };

        let empty_call = ToolCall {
            id: "call_2".to_string(),
            name: "ask_user".to_string(),
            args: json!({ "question": "" }),
        };
        let receipt = tool.execute(&empty_call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("缺少 question 参数"));

        let long_question = "测".repeat(601);
        let long_call = ToolCall {
            id: "call_3".to_string(),
            name: "ask_user".to_string(),
            args: json!({ "question": long_question }),
        };
        let receipt = tool.execute(&long_call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("上限 600 字"));
    }
}
