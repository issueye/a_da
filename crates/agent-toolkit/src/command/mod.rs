pub mod run;

pub use run::RunCommandTool;

use std::path::Path;
use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `command` 工具包中的全部工具实例
pub fn command_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(RunCommandTool::new(workspace))]
}
