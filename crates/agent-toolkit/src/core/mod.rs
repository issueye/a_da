pub mod finish;
pub mod todo;

pub use finish::FinishTool;
pub use todo::{TodoStep, TodoTool};

use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `core` 工具包中的全部工具实例
pub fn core_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TodoTool::new()),
        Arc::new(FinishTool::new()),
    ]
}
