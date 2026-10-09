pub mod builtin_tools;
pub mod engine_bridge;
pub mod executor;
pub mod prompt;
pub mod ui_events;

pub use engine_bridge::{
    project_to_loop_event, run_agent_turn, run_turn_with_engine, EngineError, LoopEventBridge,
};pub use executor::execute_tool_call;
pub use prompt::{build_system_prompt, builtin_tools, format_messages_for_model};
pub use ui_events::AgentLoopEvent;
