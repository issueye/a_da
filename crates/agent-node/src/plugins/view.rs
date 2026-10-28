//! 插件贡献的**活体视图**（P4）：界面读的必须是"引擎正在用的那份"。
//!
//! # 为什么需要它
//!
//! `plugin.list` 读的是 `PluginManager::scan_plugins`——那是**静态目录扫描**：它只列包与
//! 入口状态，**从不执行插件**，因此每个磁盘插件的 `tools` / `skills` / `prompts` 恒为
//! 空数组。后果是插件卡上的"贡献计数"永远显示 0、核心工具页的"被插件重实现"永远不亮，
//! 而引擎里那些工具明明装上了。
//!
//! 真正知道"这个插件带来了什么"的只有**执行过它的那一层**（`collect_plugin_tools`）。
//! 所以装配期把结论写进这个活体，`plugin.list` 读它；插件启停的重装配**原地替换**它
//! ——与技能活体（`SkillStore`）、片段活体同一个模式：界面与引擎读同一份，才不会出现
//! "关掉了插件、界面还列着它的工具"。
//!
//! 🔴 这份视图**只是投影**：引擎侧的真源永远是 `LiveContributions` 的注册表与
//! `HookRuntime` 的注册表。不要用它去驱动任何执行决策。

use std::collections::BTreeMap;
use std::sync::Arc;

use super::types::{PluginPromptDeclaration, PluginToolInfo};
use crate::scripts::{PluginToolAssembly};
use crate::skills::SkillSummary;

/// 一个插件当前**真的**生效的贡献。
#[derive(Debug, Clone, PartialEq)]
pub struct PluginContributionView {
    pub plugin_id: String,
    pub tools: Vec<PluginToolInfo>,
    pub prompts: Vec<PluginPromptDeclaration>,
    pub skills: Vec<SkillSummary>,
    /// 该插件**装上了**的钩子点位（已排序、已去重）。空 = 它没订任何点位。
    pub events: Vec<String>,
}

/// 插件贡献视图的活体（装配期写、重装配原地替换、界面读）。
pub type PluginContributionStore = Arc<std::sync::RwLock<Vec<PluginContributionView>>>;

/// 造一个视图活体（装配期与重装配共用这一处）。
pub fn new_contribution_store(views: Vec<PluginContributionView>) -> PluginContributionStore {
    Arc::new(std::sync::RwLock::new(views))
}

/// 从**一次装载的产物**造视图。初装（组合根）与重装配（`LivePluginReassembler`）都走它，
/// 免得两处各写一份"怎么算这个插件的贡献"，迟早说不一样的话。
///
/// `events_by_plugin` 来自 `HookRuntime::events_by_plugin`：事件在**注册期**才有定论
/// （分级/未实现/能力开关都可能把它们挡掉），所以它由运行时给，不在这里猜。
pub fn views_from_assembly(
    asm: &PluginToolAssembly,
    events_by_plugin: &BTreeMap<String, Vec<String>>,
) -> Vec<PluginContributionView> {
    let mut views: BTreeMap<String, PluginContributionView> = BTreeMap::new();
    let entry = |plugin_id: &str| -> PluginContributionView {
        PluginContributionView {
            plugin_id: plugin_id.to_string(),
            tools: Vec::new(),
            prompts: Vec::new(),
            skills: Vec::new(),
            events: Vec::new(),
        }
    };

    for (plugin_id, tool) in &asm.tools {
        let d = tool.descriptor();
        let view = views
            .entry(plugin_id.clone())
            .or_insert_with(|| entry(plugin_id));
        view.tools.push(PluginToolInfo {
            name: d.name.clone(),
            description: d.summary.clone(),
            parameters: Some(d.schema.clone()),
            // 读写性单一真源是描述符（INV-3），这里只是把它抄进投影
            is_write: !d.is_readonly(),
        });
    }
    for (plugin_id, fragment) in &asm.prompt_fragments {
        let view = views
            .entry(plugin_id.clone())
            .or_insert_with(|| entry(plugin_id));
        view.prompts.push(PluginPromptDeclaration {
            id: fragment.id.clone(),
            content: fragment.content.clone(),
        });
    }
    for (plugin_id, plugin_name, decl) in &asm.skills {
        let view = views
            .entry(plugin_id.clone())
            .or_insert_with(|| entry(plugin_id));
        view.skills.push(decl.to_summary(plugin_id, plugin_name));
    }
    // 钩子：注册成功的点位（被分级/开关挡下的不在里面——那正是"实际生效"的意思）
    for (plugin_id, points) in events_by_plugin {
        let view = views
            .entry(plugin_id.clone())
            .or_insert_with(|| entry(plugin_id));
        view.events = points.clone();
    }

    let mut out: Vec<PluginContributionView> = views.into_values().collect();
    for view in out.iter_mut() {
        view.tools.sort_by(|a, b| a.name.cmp(&b.name));
        view.prompts.sort_by(|a, b| a.id.cmp(&b.id));
        view.skills.sort_by(|a, b| a.name.cmp(&b.name));
        view.events.sort();
        view.events.dedup();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{
        Access, Execution, RollbackPolicy, Termination, ToolDescriptor, ToolReceipt,
    };
    use agent_base::ports::{BoxFuture, Tool, ToolContext};
    use std::sync::Arc as StdArc;

    struct StubTool(ToolDescriptor);

    impl Tool for StubTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.0
        }
        fn execute<'a>(
            &'a self,
            _c: &'a agent_base::domain::ToolCall,
            _x: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async { ToolReceipt::success("ok", 0, 1) })
        }
    }

    fn tool(name: &str, read_only: bool) -> StdArc<dyn Tool> {
        StdArc::new(StubTool(ToolDescriptor {
            name: name.to_string(),
            summary: format!("{name} 的说明"),
            schema: serde_json::json!({ "type": "object" }),
            access: if read_only {
                Access::ReadOnly
            } else {
                Access::Mutates {
                    paths: agent_base::domain::PathSelector::Single("path"),
                }
            },
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        }))
    }

    /// 视图要**如实**反映"装上了什么"：读写性来自描述符、钩子只收注册成功的点位。
    #[test]
    fn test_views_group_contributions_by_plugin_and_sort_them() {
        let asm = PluginToolAssembly {
            tools: vec![
                ("workspace:demo".to_string(), tool("z_tool", false)),
                ("workspace:demo".to_string(), tool("a_tool", true)),
                ("global:other".to_string(), tool("solo", true)),
            ],
            prompt_fragments: vec![(
                "workspace:demo".to_string(),
                agent_base::ports::ScriptPromptFragment {
                    id: "style".to_string(),
                    content: "一律用 tabs".to_string(),
                },
            )],
            skills: vec![(
                "workspace:demo".to_string(),
                "Demo".to_string(),
                crate::plugins::PluginSkillDeclaration {
                    name: "deploy".to_string(),
                    path: None,
                    description: "发布流程".to_string(),
                    body: "先跑测试".to_string(),
                    when_to_use: None,
                    user_invocable: None,
                },
            )],
            ..Default::default()
        };
        let mut events = BTreeMap::new();
        events.insert(
            "workspace:demo".to_string(),
            vec!["event_after_turn".to_string(), "event_before_tool_call".to_string()],
        );

        let views = views_from_assembly(&asm, &events);
        assert_eq!(views.len(), 2, "两个插件各一条视图：{views:?}");
        let demo = views
            .iter()
            .find(|v| v.plugin_id == "workspace:demo")
            .expect("demo 插件要有视图");
        assert_eq!(demo.tools.len(), 2);
        assert_eq!(
            demo.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["a_tool", "z_tool"],
            "工具按名字排序（界面每次渲染顺序稳定）"
        );
        assert!(demo.tools[0].is_write == false, "读写性来自描述符");
        assert!(demo.tools[1].is_write, "写工具必须如实标成写");
        assert_eq!(demo.prompts.len(), 1);
        assert_eq!(demo.skills.len(), 1);
        assert_eq!(demo.skills[0].plugin_id.as_deref(), Some("workspace:demo"));
        assert_eq!(demo.events, vec!["event_after_turn", "event_before_tool_call"]);

        let other = views
            .iter()
            .find(|v| v.plugin_id == "global:other")
            .expect("另一个插件也要有视图");
        assert_eq!(other.tools.len(), 1);
        assert!(other.prompts.is_empty() && other.skills.is_empty() && other.events.is_empty());
    }
}
