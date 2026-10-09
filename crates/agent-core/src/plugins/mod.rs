pub mod builtins;
pub mod manager;
pub mod sandbox;
pub mod types;

pub use manager::PluginManager;
pub use sandbox::PluginSandbox;
pub use types::*;

/// 一个插件工具是不是**写操作**。
///
/// 唯一真源是 `ToolDescriptor`（INV-3）：
/// - 内置插件：`builtins.rs` 早先的三元组里带过一个 `is_write` 布尔，它已被证明是
///   死数据（装配时用 `_` 丢弃、改由描述符派生），W2-T7 直接删掉了那个字段；
/// - 第三方插件：原先在 `manager.rs` 里**硬编码 `false`**，于是第三方写工具在界面上
///   被标成"只读"，在只读档位/plan 模式/只读子智能体里也可能被放行。
///
/// **未知工具失败安全地当作写操作**（AGENTS.md §2）：宁可多问一次审批，
/// 也不要悄悄给出写权限。
pub fn plugin_tool_is_write(tool_name: &str) -> bool {
    crate::tools::find_tool_descriptor(tool_name)
        .map(|d| !d.is_readonly())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_write_follows_descriptor() {
        // 只读工具
        assert!(!plugin_tool_is_write("git_status"));
        assert!(!plugin_tool_is_write("read_file"));
        // 写/执行工具
        assert!(plugin_tool_is_write("write_file"));
        assert!(plugin_tool_is_write("run_tests"));
        assert!(plugin_tool_is_write("kill_task"));
    }

    /// W2-T7 守门：**未知工具（第三方插件）必须被当作写操作**。
    /// 这正是原先硬编码 `false` 造成的安全缺口——第三方写工具会被当只读放行。
    #[test]
    fn test_unknown_tool_fails_safe_as_write() {
        assert!(
            plugin_tool_is_write("some_third_party_write_tool"),
            "未知工具必须失败安全地当作写操作（AGENTS.md §2）"
        );
        assert!(plugin_tool_is_write(""));
    }

    /// 内置插件的 `PluginToolInfo.is_write` 必须与描述符一致（界面徽章据此显示）。
    #[test]
    fn test_builtin_plugin_tool_is_write_matches_descriptor() {
        let items = builtins::get_builtin_plugin_items(&std::collections::HashSet::new());
        let mut checked = 0;
        for item in &items {
            for t in &item.tools {
                assert_eq!(
                    t.is_write,
                    plugin_tool_is_write(&t.name),
                    "插件 `{}` 的工具 `{}` 的 is_write 与描述符不一致",
                    item.id,
                    t.name
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "至少要检查到一些内置插件工具");
    }
}
