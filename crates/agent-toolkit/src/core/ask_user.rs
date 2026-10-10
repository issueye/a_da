use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_base::domain::{
    Access, AgentEvent, AgentEventBody, ApprovalPolicy, Execution, RollbackPolicy, Termination,
    ToolCall, ToolDescriptor, ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswerPayload {
    pub choice: Option<String>,
    pub text: Option<String>,
    pub answered_by: String,
}

pub type QuestionWaiterFuture = Pin<Box<dyn Future<Output = Result<QuestionAnswerPayload, String>> + Send>>;
pub type QuestionRegistrarFn = Arc<dyn Fn(&str) -> QuestionWaiterFuture + Send + Sync>;

static QUESTION_REGISTRAR: std::sync::RwLock<Option<QuestionRegistrarFn>> =
    std::sync::RwLock::new(None);

/// 注册全局提问等待器工厂（由拥有会话与问答管理器的上层节点注入）。
pub fn set_question_registrar(registrar: QuestionRegistrarFn) {
    if let Ok(mut lock) = QUESTION_REGISTRAR.write() {
        *lock = Some(registrar);
    }
}

pub fn clear_question_registrar() {
    if let Ok(mut lock) = QUESTION_REGISTRAR.write() {
        *lock = None;
    }
}

pub fn get_question_registrar() -> Option<QuestionRegistrarFn> {
    QUESTION_REGISTRAR.read().ok().and_then(|guard| guard.clone())
}

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

            // 3. 通道检查与提问挂起
            let Some(reg) = get_question_registrar() else {
                let finished_at = started_at;
                return ToolReceipt::error(
                    "当前环境未接入交互提问协调器（无事件问答通道），无法向用户提问。请直接基于现有上下文作出合理推断并继续。",
                    started_at,
                    finished_at,
                );
            };

            // 注册等待通道
            let waiter = reg(&call.id);

            // 发射提问事件（通知界面/上层协调者）
            let choices = call.args.get("choices").cloned().unwrap_or(json!([]));
            let allow_text = call
                .args
                .get("allow_text")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            ctx.events.emit(AgentEvent::new(
                0,
                0,
                ctx.thread_id,
                AgentEventBody::QuestionAsked {
                    call_id: call.id.clone(),
                    question: json!({
                        "question": question,
                        "choices": choices,
                        "allow_text": allow_text,
                    }),
                },
            ));

            // 等待作答或取消
            let answer = tokio::select! {
                ans = waiter => ans,
                _ = async {
                    while !ctx.cancel.is_cancelled() {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } => {
                    let finished_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    return ToolReceipt::new(
                        ToolStatus::Aborted,
                        "操作在等待提问答复时被中止。",
                        started_at,
                        finished_at,
                    );
                }
            };

            let finished_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            match answer {
                Ok(ans) => {
                    if ans.answered_by == "aborted" {
                        return ToolReceipt::new(
                            ToolStatus::Aborted,
                            "提问已被取消。",
                            started_at,
                            finished_at,
                        );
                    }
                    let summary = match (ans.choice, ans.text) {
                        (Some(c), Some(t)) => {
                            format!("答复已记录（来源: {}）：选择 [{c}]，说明：{t}", ans.answered_by)
                        }
                        (Some(c), None) => format!("答复已记录（来源: {}）：选择 [{c}]", ans.answered_by),
                        (None, Some(t)) => format!("答复已记录（来源: {}）：{t}", ans.answered_by),
                        (None, None) => format!("答复已记录（来源: {}）：已确认", ans.answered_by),
                    };
                    ToolReceipt::success(summary, started_at, finished_at)
                }
                Err(err) => ToolReceipt::error(format!("提问失败：{err}"), started_at, finished_at),
            }
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
        clear_question_registrar();
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
        clear_question_registrar();
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

    #[tokio::test]
    async fn test_ask_user_with_channel_emits_event_and_receives_answer() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t_test_qa",
        };

        // 注册测试等待器
        set_question_registrar(Arc::new(|call_id: &str| {
            assert_eq!(call_id, "call_ask_1");
            Box::pin(async {
                Ok(QuestionAnswerPayload {
                    choice: Some("c1".into()),
                    text: Some("选用方案 A".into()),
                    answered_by: "main_agent".into(),
                })
            })
        }));

        let call = ToolCall {
            id: "call_ask_1".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "question": "应该选择方案 A 还是方案 B？",
                "choices": [{"id": "c1", "label": "方案 A"}, {"id": "c2", "label": "方案 B"}],
                "allow_text": true,
            }),
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success);
        assert!(receipt.output.contains("方案 A"));
        assert!(receipt.output.contains("main_agent"));

        // 验证 QuestionAsked 事件确实被发射到了事件池中
        let recorded = events.snapshot();
        assert!(recorded.iter().any(|e| matches!(&e.body, AgentEventBody::QuestionAsked { call_id, .. } if call_id == "call_ask_1")));

        clear_question_registrar();
    }
}
