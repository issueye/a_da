//! 决策与门禁工具包（`agent-toolkit::decision`，遵循 AGENTS.md §9 "绝不捏造确定性" 原则）

pub mod decide_tool;
pub mod engine;
pub mod gate;
pub mod gate_tool;
pub mod types;

pub use decide_tool::DecideTool;
pub use engine::{DecisionEngine, HeuristicDecisionEngine};
pub use gate::{run_gate, GateOptions, DEFAULT_GATE_THRESHOLD};
pub use gate_tool::CheckGateTool;
pub use types::{
    DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionResponse, EngineId, GateOutcome,
    GateSource, QuestionType,
};

use std::path::Path;
use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `decision` 工具包中的全部工具实例。
///
/// 与 `core`/`fs`/`command` 对齐：工具包必须有**工厂**，否则
/// `agent.spec.json` 里的 `toolkits` 声明就没有落点（W2-T1 之前的缺口）。
pub fn decision_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(DecideTool::new()),
        Arc::new(CheckGateTool::new(workspace)),
    ]
}
