//! 工具包真源：`agent.spec.json` 的 `toolkits` 字段就是查这张表。
//!
//! 为什么需要它：在 W2-T1 之前，`spec.toolkits` 被解析出来却**没有任何消费者**——
//! `ada-coding` 甚至声明了一个不存在的 `patch` 工具包（能力实际藏在 `fs` 的
//! edit/write 里），而 `decision` 有真实模块却没人声明。两侧都不一致，
//! 且没有任何机械检查能发现（`verify-spec` 当时只能靠"目录是否存在"猜）。
//!
//! 现在**名字 → 工具实例**只有这一处映射：
//! - 产品声明（`spec.toolkits`）→ [`tools_for_toolkits`]；
//! - 声明校验（`cargo xtask verify-spec`）→ [`TOOLKIT_NAMES`]；
//! - 接线审计（`cargo xtask verify-wiring`）→ 用本表判定"工具是否有执行路径"。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use agent_base::ports::Tool;

/// 已知工具包名（**唯一真源**）。
///
/// `decision` 已于 2026-10-18 整体下线（`decide` / `check_gate` 两个工具 + 实现模块一起删），
/// 产品声明里再出现它会在装配期直接报错——这正是这张表存在的意义。
///
/// `git` 已于 2026-10-27 搬成插件（`plugins/git-tools`）：它现在是**插件启停**控制，
/// 不再是一个工具包。产品声明里再写 `"git"` 同样会在装配期报错。
pub const TOOLKIT_NAMES: &[&str] = &["core", "fs", "command", "project", "forge"];

/// 该名字是否是已知工具包。
pub fn is_known_toolkit(name: &str) -> bool {
    TOOLKIT_NAMES.contains(&name)
}

/// 装配工具包时需要知道的**环境**。
///
/// 为什么不是"再传一个参数"：工具工厂（`forge`）要用应用目录决定插件落到工作区还是全局，
/// 而工作区之外的一切都必须由**组合根注入**——工具层自己读 `USERPROFILE` 或
/// `A_DA_HOME` 就会在测试里写到用户真实目录（本项目为这类污染出过事故）。
#[derive(Clone, Copy)]
pub struct ToolkitEnv<'a> {
    pub workspace: &'a Path,
    /// 应用目录（`~/.a-da`）。`None` = 这个装配面没有应用目录概念
    /// （测试 / 子体 / 合规套件）——需要它的能力会**如实报错**，而不是猜一个路径。
    pub app_home: Option<&'a Path>,
    /// 宿主注入的后台任务表；`command` 工具包与管理面必须共用同一份。
    pub background_tasks: Option<&'a Arc<crate::background::BackgroundTasks>>,
}

impl<'a> ToolkitEnv<'a> {
    /// 只给工作区（不需要应用目录的装配面用这个）。
    pub fn workspace_only(workspace: &'a Path) -> Self {
        Self {
            workspace,
            app_home: None,
            background_tasks: None,
        }
    }
}

/// 取一个工具包的全部工具实例。未知名返回 `None`。
pub fn tools_for_toolkit(name: &str, workspace: &Path) -> Option<Vec<Arc<dyn Tool>>> {
    tools_for_toolkit_in(name, ToolkitEnv::workspace_only(workspace))
}

/// 带环境取一个工具包的全部工具实例。
pub fn tools_for_toolkit_in(name: &str, env: ToolkitEnv<'_>) -> Option<Vec<Arc<dyn Tool>>> {
    let workspace = env.workspace;
    match name {
        "core" => Some(crate::core::core_tools()),
        "fs" => Some(crate::fs::fs_tools(workspace)),
        "command" => Some(crate::command::command_tools_with_store(
            workspace,
            env.background_tasks.cloned(),
        )),
        // 插件包脚手架：生成可加载的最小插件包（`create_plugin`）。
        // 它需要应用目录：用户没指定工作区还是全局时，工具要**先问**；
        // 指定全局时得知道 `~/.a-da/agent-coding/extensions` 在哪。
        "forge" => Some(crate::forge::forge_tools_in(workspace, env.app_home)),
        // W2-T2：原先只在 `execute_builtin_plugin_tool` 字符串分派里的插件工具
        "project" => Some(crate::plugin_tools::project_tools(workspace)),
        _ => None,
    }
}

/// 按产品声明装配多个工具包。
///
/// 两条**失败安全**约束（不许静默忽略）：
/// 1. 未知名 → `Err`（声明了一个不存在的工具包，就该在装配期炸掉，而不是少装几个工具）；
/// 2. 重名 → `Err`（两个工具包提供同名工具会让 catalog 出现两份同名描述符，
///    而"注册表即真源"要求名字唯一）。
pub fn tools_for_toolkits(
    names: &[String],
    workspace: &Path,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    tools_for_toolkits_in(names, ToolkitEnv::workspace_only(workspace))
}

/// 带环境装配多个工具包（组合根走这个：它要注入应用目录给 `forge`）。
pub fn tools_for_toolkits_in(
    names: &[String],
    env: ToolkitEnv<'_>,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    for name in names {
        let tools = tools_for_toolkit_in(name, env).ok_or_else(|| {
            format!(
                "未知工具包 `{name}`（已知：{}）",
                TOOLKIT_NAMES.join(", ")
            )
        })?;
        out.extend(tools);
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for t in &out {
        let n = t.descriptor().name.clone();
        if !seen.insert(n.clone()) {
            return Err(format!("工具 `{n}` 被多个工具包重复提供"));
        }
    }
    Ok(out)
}

/// 所有工具包提供的工具名（用于接线审计与"声明 ↔ 实现"对账）。
pub fn all_toolkit_tool_names(workspace: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in TOOLKIT_NAMES {
        if let Some(tools) = tools_for_toolkit(name, workspace) {
            for t in tools {
                out.insert(t.descriptor().name.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> &'static Path {
        Path::new("E:/toolkit_test_ws")
    }

    #[test]
    fn test_every_known_toolkit_yields_tools() {
        for name in TOOLKIT_NAMES {
            let tools = tools_for_toolkit(name, ws())
                .unwrap_or_else(|| panic!("工具包 `{name}` 必须存在"));
            assert!(!tools.is_empty(), "工具包 `{name}` 不能是空的");
        }
    }

    #[test]
    fn test_unknown_toolkit_is_an_error_not_silently_skipped() {
        // `patch` 就是曾经被 `ada-coding` 误声明的那一个
        assert!(tools_for_toolkit("patch", ws()).is_none());
        let err = match tools_for_toolkits(&["patch".to_string()], ws()) {
            Ok(_) => panic!("未知名必须报错，而不是少装几个工具"),
            Err(e) => e,
        };
        assert!(err.contains("patch"), "错误信息要能定位：{err}");
    }

    #[test]
    fn test_declared_toolkits_assemble_without_duplicates() {
        let names: Vec<String> = TOOLKIT_NAMES.iter().map(|s| s.to_string()).collect();
        let tools = tools_for_toolkits(&names, ws()).expect("全部工具包必须能装配");
        assert_eq!(
            tools.len(),
            all_toolkit_tool_names(ws()).len(),
            "装配出的工具数必须等于去重后的名字数（说明没有重名）"
        );
    }

    #[test]
    fn test_all_toolkit_tool_names_covers_the_expected_core_tools() {
        let names = all_toolkit_tool_names(ws());
        for expect in ["read_file", "write_file", "run_command", "ask_user", "todo", "finish", "project_inspect", "create_plugin"] {
            assert!(names.contains(expect), "`{expect}` 应来自某个工具包：{names:?}");
        }
    }

    #[test]
    fn test_duplicate_tool_names_across_toolkits_are_rejected() {
        // 同一工具包声明两次 → 重名，必须报错
        let names = vec!["core".to_string(), "core".to_string()];
        let err = match tools_for_toolkits(&names, ws()) {
            Ok(_) => panic!("重名必须报错"),
            Err(e) => e,
        };
        assert!(err.contains("重复提供"), "错误信息要说明原因：{err}");
    }

    /// 守门测试：通过工具包装配的所有工具实例，其描述符必须与 registry.rs 中的单一真源完全一致（INV-3）。
    #[test]
    fn test_all_instantiated_tools_match_registry_descriptors_exactly() {
        let names: Vec<String> = TOOLKIT_NAMES.iter().map(|s| s.to_string()).collect();
        let tools = tools_for_toolkits(&names, ws()).expect("全部工具包必须能装配");
        assert!(!tools.is_empty(), "装配出的工具集不能为空");

        for tool in &tools {
            let desc = tool.descriptor();
            let registered = crate::registry::find_tool_descriptor(&desc.name)
                .unwrap_or_else(|| panic!("工具 `{}` 实例化的描述符未在 registry.rs 注册", desc.name));

            assert_eq!(
                desc.name, registered.name,
                "工具 `{}` name 不匹配", desc.name
            );
            assert_eq!(
                desc.summary, registered.summary,
                "工具 `{}` summary 不匹配", desc.name
            );
            assert_eq!(
                desc.schema, registered.schema,
                "工具 `{}` schema 不匹配", desc.name
            );
            assert_eq!(
                desc.access, registered.access,
                "工具 `{}` access 语义不匹配", desc.name
            );
            assert_eq!(
                desc.approval, registered.approval,
                "工具 `{}` approval 策略不匹配", desc.name
            );
            assert_eq!(
                desc.rollback, registered.rollback,
                "工具 `{}` rollback 策略不匹配", desc.name
            );
            assert_eq!(
                desc.execution, registered.execution,
                "工具 `{}` execution 模式不匹配", desc.name
            );
            assert_eq!(
                desc.termination, registered.termination,
                "工具 `{}` termination 语义不匹配", desc.name
            );
        }
    }
}
