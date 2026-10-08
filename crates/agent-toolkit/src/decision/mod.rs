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
