//! DecideTool 结构化决策工具（INV-3 描述符单一真源）

use std::sync::Arc;
use std::time::Instant;
use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use crate::decision::engine::{DecisionEngine, HeuristicDecisionEngine};
use crate::decision::types::{DecisionRequest, DecisionResponse};

#[derive(Clone)]
pub struct DecideTool {
    descriptor: ToolDescriptor,
    engine: Arc<dyn DecisionEngine>,
}

impl DecideTool {
    pub fn new() -> Self {
        Self::with_engine(Arc::new(HeuristicDecisionEngine::new()))
    }

    pub fn with_engine(engine: Arc<dyn DecisionEngine>) -> Self {
        let descriptor = ToolDescriptor {
            name: "decide".to_string(),
            summary: "对给定材料进行结构化判断，支持 choice（多选一）、noul（是否概率）与 score（档位评分）三种类型".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "state": {
                        "description": "被判断的材料（文本或结构化对象），必须自包含",
                        "type": ["string", "object"]
                    },
                    "questions": {
                        "type": "object",
                        "description": "问题映射：id -> 问题定义。包含 type（choice/noul/score）、instructions 与可选的 criteria",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string", "enum": ["choice", "noul", "score"] },
                                "instructions": { "type": "string" },
                                "criteria": { "description": "choice 为对象，score 为数组，noul 无需传递" }
                            },
                            "required": ["type", "instructions"]
                        }
                    },
                    "threshold": {
                        "type": "number",
                        "description": "覆盖默认判定阈值（默认 0.65）"
                    }
                },
                "required": ["state", "questions"]
            }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };

        Self { descriptor, engine }
    }

    /// 执行决策评估并生成 ToolReceipt
    pub async fn run(&self, args: &serde_json::Value) -> ToolReceipt {
        let start = Instant::now();
        let started_at = 1000;

        let req: DecisionRequest = match serde_json::from_value(args.clone()) {
            Ok(r) => r,
            Err(e) => {
                return ToolReceipt {
                    status: ToolStatus::Error,
                    output: format!("参数解析失败: {}", e),
                    data: None,
                    details: None,
                    started_at,
                    finished_at: started_at + start.elapsed().as_millis() as i64,
                };
            }
        };

        if req.questions.is_empty() {
            return ToolReceipt {
                status: ToolStatus::Error,
                output: "没有提供任何有效的问题定义".to_string(),
                data: None,
                details: None,
                started_at,
                finished_at: started_at + start.elapsed().as_millis() as i64,
            };
        }

        match self.engine.evaluate(&req) {
            Ok(res) => {
                let text = format_decision_response(&res);
                ToolReceipt {
                    status: ToolStatus::Success,
                    output: text,
                    data: serde_json::to_value(&res).ok(),
                    details: None,
                    started_at,
                    finished_at: started_at + start.elapsed().as_millis() as i64,
                }
            }
            Err(e) => ToolReceipt {
                status: ToolStatus::Error,
                output: format!("决策评估执行失败: {}", e),
                data: None,
                details: None,
                started_at,
                finished_at: started_at + start.elapsed().as_millis() as i64,
            },
        }
    }
}

impl Default for DecideTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for DecideTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            self.run(&call.args).await
        })
    }
}

fn format_decision_response(res: &DecisionResponse) -> String {
    let mut lines = vec![
        format!("## 决策结果（引擎: {}, 耗时: {}ms）", res.engine.as_str(), res.elapsed_ms),
        "".to_string(),
    ];

    for (id, ans) in &res.answers {
        lines.push(format!("### {}", id));
        match ans.question_type {
            crate::decision::types::QuestionType::Noul => {
                lines.push(format!("概率: {}", ans.value));
            }
            _ => {
                lines.push(format!("结论: {}", ans.value));
            }
        }
        if let Some(c) = ans.confidence {
            lines.push(format!("置信度: {:.2}", c));
        }
        lines.push(format!("已校准: {}", if ans.calibrated { "是" } else { "否" }));
        lines.push("".to_string());
    }

    if !res.notes.is_empty() {
        lines.push(format!("> 说明: {}", res.notes.join("; ")));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::DecisionQuestion;

    #[tokio::test]
    async fn test_decide_tool_execution() {
        let tool = DecideTool::new();
        let mut questions = std::collections::HashMap::new();
        questions.insert(
            "test_ok".to_string(),
            DecisionQuestion::Noul {
                instructions: "是否已完成".to_string(),
            },
        );

        let call = ToolCall {
            id: "call_1".to_string(),
            name: "decide".to_string(),
            args: serde_json::json!({
                "state": "全部任务已完成，测试成功 pass",
                "questions": questions
            }),
        };

        let dummy_scope = agent_base::testing::MockScope::default();
        let dummy_cancel = agent_base::testing::NeverCancel;
        let dummy_events = agent_base::testing::RecordingSink::default();
        let ctx = ToolContext {
            thread_id: "t1",
            scope: &dummy_scope,
            cancel: &dummy_cancel,
            events: &dummy_events,
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success);
        assert!(receipt.output.contains("决策结果"));
        assert!(receipt.output.contains("test_ok"));
    }
}
