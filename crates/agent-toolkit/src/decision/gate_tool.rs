//! CheckGateTool 门禁判定工具（INV-3 描述符单一真源）

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde::Deserialize;
use crate::decision::engine::{DecisionEngine, HeuristicDecisionEngine};
use crate::decision::gate::{run_gate, GateOptions};
use crate::decision::types::{GateOutcome, GateSource};

#[derive(Debug, Deserialize)]
struct CheckGateArgs {
    criteria: String,
    #[serde(default = "default_source")]
    source: String,
    file: Option<String>,
    text: Option<String>,
    threshold: Option<f64>,
    #[serde(default)]
    fail_open: bool,
}

fn default_source() -> String {
    "diff".to_string()
}

#[derive(Clone)]
pub struct CheckGateTool {
    descriptor: ToolDescriptor,
    engine: Arc<dyn DecisionEngine>,
    workspace: PathBuf,
}

impl CheckGateTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self::with_engine(workspace, Arc::new(HeuristicDecisionEngine::new()))
    }

    pub fn with_engine(workspace: impl Into<PathBuf>, engine: Arc<dyn DecisionEngine>) -> Self {
        let descriptor = ToolDescriptor {
            name: "check_gate".to_string(),
            summary: "对当前工作区改动、指定文件或文本依据验收标准执行门禁判定（fail-close 安全默认）".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "criteria": {
                        "type": "string",
                        "description": "具体的验收标准，如「所有新增函数必须包含单元测试」"
                    },
                    "source": {
                        "type": "string",
                        "enum": ["diff", "file", "text"],
                        "description": "门禁材料来源，默认为 diff（当前 git 未提交改动）"
                    },
                    "file": {
                        "type": "string",
                        "description": "当 source=file 时必填，工作区内的相对文件路径"
                    },
                    "text": {
                        "type": "string",
                        "description": "当 source=text 时必填，直接输入的文本材料"
                    },
                    "threshold": {
                        "type": "number",
                        "description": "通过概率阈值（默认 0.65）"
                    },
                    "fail_open": {
                        "type": "boolean",
                        "description": "评估出错时是否默认放行，默认为 false（即 fail-close 默认拦截）"
                    }
                },
                "required": ["criteria"]
            }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };

        Self {
            descriptor,
            engine,
            workspace: workspace.into(),
        }
    }

    /// 执行门禁判定并生成 ToolReceipt
    pub async fn run(&self, args: &serde_json::Value) -> ToolReceipt {
        let start = Instant::now();
        let started_at = 1000;

        let args: CheckGateArgs = match serde_json::from_value(args.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolReceipt {
                    status: ToolStatus::Error,
                    output: format!("参数解析错误: {}", e),
                    data: None,
                    details: None,
                    started_at,
                    finished_at: started_at + start.elapsed().as_millis() as i64,
                };
            }
        };

        let source = match args.source.as_str() {
            "file" => GateSource::File,
            "text" => GateSource::Text,
            _ => GateSource::Diff,
        };

        let options = GateOptions {
            criteria: args.criteria,
            source,
            file: args.file,
            text: args.text,
            threshold: args.threshold,
            fail_open: args.fail_open,
            workspace: self.workspace.clone(),
        };

        let outcome: GateOutcome = run_gate(options, self.engine.as_ref());
        let output = format_gate_outcome(&outcome);

        ToolReceipt {
            status: ToolStatus::Success,
            output,
            data: serde_json::to_value(&outcome).ok(),
            details: None,
            started_at,
            finished_at: started_at + start.elapsed().as_millis() as i64,
        }
    }
}

impl Tool for CheckGateTool {
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

fn format_gate_outcome(outcome: &GateOutcome) -> String {
    let mut lines = vec![
        format!("## 门禁判定：{}", if outcome.passed { "通过 [PASS]" } else { "未通过 [BLOCKED]" }),
        format!("- 验收标准: {}", outcome.criteria),
        format!("- 评估概率: {:.3} (阈值: {:.2})", outcome.probability, outcome.threshold),
        format!("- 决策引擎: {} (已校准: {})", outcome.engine.as_str(), if outcome.calibrated { "是" } else { "否" }),
        format!("- 耗时: {}ms", outcome.elapsed_ms),
    ];

    if let Some(ref note) = outcome.note {
        lines.push(format!("- 补充说明: {}", note));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_gate_tool_execution() {
        let tool = CheckGateTool::new(".");
        let call = ToolCall {
            id: "call_gate".to_string(),
            name: "check_gate".to_string(),
            args: serde_json::json!({
                "criteria": "代码无报错且测试通过",
                "source": "text",
                "text": "all tests passed, 0 errors, build success",
                "threshold": 0.6
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
        assert!(receipt.output.contains("通过 [PASS]"));
    }
}
