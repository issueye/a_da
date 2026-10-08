pub mod batch;
pub mod edit;
pub mod list;
pub mod read;
pub mod search;
pub mod write;

pub use batch::{BatchReplaceTool, BatchWriteTool};
pub use edit::EditFileTool;
pub use list::ListFilesTool;
pub use read::ReadFileTool;
pub use search::SearchFilesTool;
pub use write::WriteFileTool;

use std::path::Path;
use std::sync::Arc;
use agent_base::ports::Tool;

/// 创建 `fs` 工具包中的全部工具实例
pub fn fs_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::new(workspace)),
        Arc::new(WriteFileTool::new(workspace)),
        Arc::new(EditFileTool::new(workspace)),
        Arc::new(ListFilesTool::new(workspace)),
        Arc::new(SearchFilesTool::new(workspace)),
        Arc::new(BatchWriteTool::new(workspace)),
        Arc::new(BatchReplaceTool::new(workspace)),
    ]
}
