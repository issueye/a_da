pub mod manager;
pub mod slug;
pub mod types;

// `app_home` / `set_app_home` 是过渡期的端口缝隙（见 manager.rs 的说明）：调用方最终应改为
// 接收 `&dyn AppHome`，届时这两个函数会消失。
pub use manager::{app_home, get_app_home, get_config_path, set_app_home, SessionManager};
pub use slug::{safe_id, workspace_slug};
pub use types::*;
