pub mod ask_user;
pub mod finish;
pub mod todo;
pub mod todo_diff;

pub use ask_user::AskUserTool;
pub use finish::FinishTool;
pub use todo::TodoTool;
pub use todo_diff::{diff_todos, TodoDiff, TodoStep};

use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `core` 工具包中的全部工具实例
pub fn core_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(AskUserTool::new()),
        Arc::new(TodoTool::new()),
        Arc::new(FinishTool::new()),
    ]
}
