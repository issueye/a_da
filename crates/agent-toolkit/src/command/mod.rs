pub mod run;

pub use run::RunCommandTool;

use std::path::Path;
use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `command` 工具包中的全部工具实例。
///
/// 含：
/// - **后台任务族**（`run_background`/`check_task`/`kill_task`，W2-T3）：共享同一张
///   实例态任务表（INV-8），所以必须由工厂一次性创建；
/// - **`run_tests`**（W2-T2）：它本质就是"跑一条探测出来的命令"。
pub fn command_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(RunCommandTool::new(workspace))];
    tools.extend(crate::background::background_tools(workspace));
    tools.extend(crate::plugin_tools::test_tools(workspace));
    tools
}
