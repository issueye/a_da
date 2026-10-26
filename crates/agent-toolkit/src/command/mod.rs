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
    command_tools_with_store(workspace, None)
}

/// 同 [`command_tools`]，但允许宿主注入后台任务表（管理面要读同一份）。
pub fn command_tools_with_store(
    workspace: &Path,
    background_tasks: Option<Arc<crate::background::BackgroundTasks>>,
) -> Vec<Arc<dyn Tool>> {
    let tasks = background_tasks
        .unwrap_or_else(|| Arc::new(crate::background::BackgroundTasks::new(workspace)));
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(RunCommandTool::new(workspace))];
    tools.extend(crate::background::background_tools_with_store(
        workspace,
        tasks,
    ));
    tools.extend(crate::plugin_tools::test_tools(workspace));
    tools
}
