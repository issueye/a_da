pub mod cmd_tools;
pub mod diff;
pub mod fs_tools;
pub mod sandbox;

pub use cmd_tools::run_command;
pub use diff::{patch_stats, unified_diff};
pub use fs_tools::{
    edit_file, list_files, read_file, search_files, write_file, EditPair, ToolResult,
};
pub use sandbox::{check_workspace_sandbox, resolve_real_path, should_skip_dir, SKIP_DIRS};

pub fn is_readonly_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file"
            | "list_files"
            | "search_files"
            | "get_outline"
            | "find_symbol"
            | "read_files"
            | "inspect_project"
            | "read_url_content"
            | "git_status"
            | "git_diff"
            | "git_log"
            | "decide"
            | "todo"
            | "Skill"
            | "design_decision"
    )
}

pub fn is_write_tool(tool_name: &str) -> bool {
    !is_readonly_tool(tool_name)
}
