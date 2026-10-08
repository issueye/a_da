//! 兼容 shim：工具实现已按设计搬到 `agent-toolkit`。
//!
//! 注意：`is_readonly_tool` / `is_write_tool` 这两张**按名字的名单**是已知缺陷
//! （见 docs/agent-base-plan.md §1.3、M2 的 `ToolDescriptor`），暂随工具包一起搬走，M2 删除。
//! M1 收敛完后删除本文件。

pub use agent_toolkit::*;