//! 活着的插件贡献集合（P2 · SP-46）：**受控重装配的唯一入口**。
//!
//! # 为什么要在这一层
//!
//! 重装配要动三样东西，而它们分属三层：
//!
//! | 东西 | 谁改得动 |
//! |---|---|
//! | 工具集合 | `agent_runtime::LiveContributions`（走注册表的 `register`/`unregister`） |
//! | 插件提示词片段 | 适配器的共享片段活体（`CodingPromptSource` 每步重新读） |
//! | 插件技能 | 技能解析器的共享技能活体（每步重新求值） |
//!
//! 「活的」这件事必须在**装配期**就布好线：解析器与提示词源拿到的必须是**同一份**
//! 共享句柄，否则重装配换的是另一份副本，界面上的开关就又一次变成"点了没反应"。
//! 所以这个结构由装配函数造好、挂进 [`HostedProduct`]，组合根拿着它驱动启停。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use agent_adapter::prompt::PluginFragmentStore;
use agent_node::skills::SkillSummary;
use agent_runtime::{
    AssemblyPlan, ContributionDelta, ContributionDigest, LiveContributions,
};

/// 插件技能的**共享活体**：技能解析器每步读它一次，所以换了内容下一轮即生效。
pub type SkillStore = Arc<RwLock<Vec<SkillSummary>>>;

/// 造一个技能活体（装配期与组合根共用这一处）。
pub fn new_skill_store(skills: Vec<SkillSummary>) -> SkillStore {
    Arc::new(RwLock::new(skills))
}

/// 活的插件贡献集合。
pub struct LivePluginContributions {
    live: LiveContributions,
    fragments: PluginFragmentStore,
    skills: SkillStore,
    /// 当前生效的**插件贡献摘要**（收据的"之前"这一半从这里取）。
    ///
    /// 为什么不用 `LiveContributions::digest()`：那个摘要只看装配计划，而宿主那条计划里
    /// **只有工具**（提示词与技能是适配器/解析器另外产出的）。用它做收据会出现
    /// "片段与技能确实换了，收据却说无变化"——那是最坏的一种谎：看起来有数据，其实是空的。
    current: Mutex<ContributionDigest>,
}

impl LivePluginContributions {
    pub fn new(plan: &AssemblyPlan, fragments: PluginFragmentStore, skills: SkillStore) -> Self {
        let current = ContributionDigest::of_collection(
            plan,
            &fragments.read().map(|g| g.clone()).unwrap_or_default(),
            &skill_labels(&skills.read().map(|g| g.clone()).unwrap_or_default()),
        );
        Self {
            live: LiveContributions::new(plan),
            fragments,
            skills,
            current: Mutex::new(current),
        }
    }

    /// 引擎手里那本注册表（与装配计划里的**是同一个**）。
    pub fn catalog(&self) -> Arc<agent_runtime::CompositeToolCatalog> {
        self.live.catalog()
    }

    /// 当前生效的贡献摘要（插件视角：工具 + 片段 + 技能）。
    pub fn digest(&self) -> ContributionDigest {
        self.current.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 共享片段活体（装配时交给提示词源的那一个）。
    pub fn fragment_store(&self) -> PluginFragmentStore {
        self.fragments.clone()
    }

    /// 共享技能活体（装配时交给技能解析器的那一个）。
    pub fn skill_store(&self) -> SkillStore {
        self.skills.clone()
    }

    /// 当前生效的插件技能（快照）。
    pub fn plugin_skills(&self) -> Vec<SkillSummary> {
        self.skills.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// **受控重装配**：把新装出来的贡献换进活体，返回差异。
    ///
    /// 三样一起换：工具是引擎正在查的那本账（换完立刻一致），片段与技能本来就在
    /// 每步重新读取（下一轮取提示词即带上新内容）。
    pub fn apply(
        &self,
        plan: &AssemblyPlan,
        fragments: Vec<(String, String, String)>,
        skills: Vec<SkillSummary>,
    ) -> ContributionDelta {
        let before = self.digest();
        let after = ContributionDigest::of_collection(plan, &fragments, &skill_labels(&skills));
        let mut delta = ContributionDelta::between(&before, &after);
        // 替换内置是"名字没变、实现换了"的审计事实，差异里看不出来，单独带上
        delta.replaced_builtins = plan.replaced_builtins.clone();

        // 真的换：注册表走 runtime 的受控入口；片段与技能原地替换
        self.live.apply(plan);
        if let Ok(mut f) = self.fragments.write() {
            *f = fragments;
        }
        if let Ok(mut s) = self.skills.write() {
            *s = skills;
        }
        if let Ok(mut cur) = self.current.lock() {
            *cur = after;
        }
        delta
    }
}

/// 技能 → `(plugin_id, name)`：摘要的标签形状用插件 id 归因。
fn skill_labels(skills: &[SkillSummary]) -> Vec<(String, String)> {
    skills
        .iter()
        .map(|s| {
            (
                s.plugin_id.clone().unwrap_or_else(|| "unknown".to_string()),
                s.name.clone(),
            )
        })
        .collect()
}

/// 生产用的**重装配器**（P2 · SP-46）：插件启停 → 重新收集 → 同一条管线重建 → 换进活体。
///
/// # 为什么装在这里而不是产品 `main`
///
/// 它要三样东西：`ScriptHost`（要再执行一遍插件才知道它声明了什么）、产品声明
/// （内置工具包从哪来）、活体贡献集合（引擎正在用的那本账）。这三样都在本 crate 的装配
/// 范围内；放进产品二进制就**测不到了**（`products/*` 是 bin，没有集成测试入口），
/// 而"启停真的改变贡献集合"这件事恰恰是最需要端到端验证的。
pub struct LivePluginReassembler {
    spec: agent_runtime::AgentSpec,
    workspace: PathBuf,
    plugin_mgr: Arc<agent_node::plugins::PluginManager>,
    script_host: Arc<dyn agent_base::ports::ScriptHost>,
    live: Arc<LivePluginContributions>,
    budget: agent_base::ports::ScriptBudget,
    /// 钩子运行时（P3a）：启停要**原地重装注册**，否则"关掉插件之后它的钩子还在拦"。
    ///
    /// 与工具/片段/技能一样走活体：引擎持的是 `Arc<dyn HookRunner>`，换不掉，
    /// 只能换它内部那张注册表。
    hook_runtime: Option<Arc<agent_node::hooks::HookRuntime>>,
}

impl LivePluginReassembler {
    pub fn new(
        spec: agent_runtime::AgentSpec,
        workspace: impl Into<PathBuf>,
        plugin_mgr: Arc<agent_node::plugins::PluginManager>,
        script_host: Arc<dyn agent_base::ports::ScriptHost>,
        live: Arc<LivePluginContributions>,
    ) -> Self {
        Self {
            spec,
            workspace: workspace.into(),
            plugin_mgr,
            script_host,
            live,
            budget: agent_base::ports::ScriptBudget::default(),
            hook_runtime: None,
        }
    }

    /// 接上钩子运行时（P3a）：启停时原地重装它的注册表。
    pub fn with_hook_runtime(
        mut self,
        runtime: Arc<agent_node::hooks::HookRuntime>,
    ) -> Self {
        self.hook_runtime = Some(runtime);
        self
    }

    /// 收紧预算（组合根按产品策略调；测试可给一个很短的超时）。
    pub fn with_budget(mut self, budget: agent_base::ports::ScriptBudget) -> Self {
        self.budget = budget;
        self
    }
}

impl agent_rpc::server::dispatch::PluginReassembler for LivePluginReassembler {
    fn reassemble<'a>(
        &'a self,
        plugin_id: &'a str,
        enabled: bool,
    ) -> agent_base::ports::BoxFuture<'a, Option<agent_runtime::ReassemblyReport>> {
        Box::pin(async move {
            let before = self.live.digest();

            // 1. 按**最新**启停状态重新收集（停用的插件在这一步就被跳过）
            let collected = agent_node::scripts::collect_plugin_tools(
                &self.plugin_mgr,
                &self.script_host,
                &self.workspace,
                self.budget,
            )
            .await;

            // 2. 同一条装配管线重建计划：内置工具包按声明来（插件启停不该动内置）
            let mut assembly = agent_runtime::AssemblyBuilder::new();
            match agent_toolkit::tools_for_toolkits(&self.spec.toolkits, &self.workspace) {
                Ok(tools) => {
                    for tool in tools {
                        if let Err(e) = assembly.add_builtin_tool(tool) {
                            // 内置声明打架是**装配期该炸**的问题（同 `run_from_spec`）：
                            // 这里如实放弃重装配并记一行，绝不假装成功。
                            tracing::warn!("[reassembly] 放弃重装配（内置工具重名）：{}", e.message);
                            return None;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[reassembly] 放弃重装配（工具包声明不合法）：{e:?}");
                    return None;
                }
            }
            let fragments: Vec<(String, String, String)> = collected
                .prompt_fragments
                .iter()
                .map(|(pid, f)| (pid.clone(), f.id.clone(), f.content.clone()))
                .collect();
            let skills: Vec<SkillSummary> = collected
                .skills
                .iter()
                .map(|(pid, pname, decl)| decl.to_summary(pid, pname))
                .collect();
            for (pid, tool) in collected.tools.iter().cloned() {
                assembly.add_plugin_tool(pid, tool);
            }
            let plan = assembly.build();

            // 3. 换进活体（工具集合 + 片段 + 技能），拿差异
            let delta = self.live.apply(&plan, fragments, skills);

            // 3.5 钩子注册**原地重装**（P3a）：停用的插件，它的钩子必须一起下线。
            // 少了这一步就会出现"关掉了插件，它的 beforeToolCall 还在拦"——比工具残留更糟，
            // 因为那是**行为面**的残留。
            let mut hook_notes: Vec<String> = Vec::new();
            if let Some(rt) = &self.hook_runtime {
                let mut rej = Vec::new();
                rt.reassemble(&collected.handles, &mut rej);
                hook_notes = rej
                    .iter()
                    .map(|r| format!("{}：{}（{}）", r.plugin_id, r.point, r.reason))
                    .collect();
            }

            // 4. 收据：装不上的插件**如实报出**（fail-safe，不静默）
            let mut skipped: Vec<String> = collected
                .skipped
                .iter()
                .map(|s| format!("{}：{}", s.plugin_id, s.reason))
                .collect();
            skipped.extend(hook_notes);
            if !skipped.is_empty() {
                tracing::warn!("[reassembly] 跳过的声明：{}", skipped.join("；"));
            }

            Some(agent_runtime::ReassemblyReport {
                request: agent_runtime::PluginToggleRequest {
                    plugin_id: plugin_id.to_string(),
                    scope: "plugin".to_string(),
                    from_enabled: !enabled,
                    to_enabled: enabled,
                },
                before,
                after: self.live.digest(),
                delta,
                skipped,
                applied: true,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor};
    use agent_base::ports::ToolCatalog;
    use agent_runtime::{AssemblyBuilder, ContributionOrigin};
    use std::sync::Arc;

    /// 极简工具（只要描述符，够装配与重装配用）。
    struct StubTool(ToolDescriptor);

    impl agent_base::ports::Tool for StubTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.0
        }
        fn execute<'a>(
            &'a self,
            _c: &'a agent_base::domain::ToolCall,
            _x: &'a agent_base::ports::ToolContext<'a>,
        ) -> agent_base::ports::BoxFuture<'a, agent_base::domain::ToolReceipt> {
            Box::pin(async { agent_base::domain::ToolReceipt::success("ok", 0, 1) })
        }
    }

    fn tool(name: &str) -> Arc<dyn agent_base::ports::Tool> {
        Arc::new(StubTool(ToolDescriptor {
            name: name.to_string(),
            summary: format!("{name} 的说明"),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        }))
    }

    fn skill(id: &str, name: &str) -> SkillSummary {
        SkillSummary {
            id: id.to_string(),
            name: name.to_string(),
            description: "说明".to_string(),
            body: "正文".to_string(),
            path: String::new(),
            base_directory: String::new(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some("Demo".to_string()),
            plugin_id: Some("p1".to_string()),
            is_file_skill: Some(false),
            user_invocable: Some(true),
            when_to_use: None,
            unknown_frontmatter_keys: Vec::new(),
        }
    }

    fn plan_with_plugin() -> AssemblyPlan {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).expect("内置不重名");
        b.add_plugin_tool("p1", tool("web_search"));
        b.add_prompt_fragment(agent_runtime::PromptFragment {
            id: "plugin:p1:style".to_string(),
            source: "plugin:p1".to_string(),
            order: 300,
            content: "一律用 tabs".to_string(),
        });
        b.add_skill(agent_runtime::SkillEntry {
            id: "plugin:p1:deploy".to_string(),
            name: "deploy".to_string(),
            origin: ContributionOrigin::Plugin { id: "p1".to_string() },
            enabled: true,
            body: "正文".to_string(),
        });
        b.build()
    }

    /// 只有内置的基线计划（= 插件被停用后的样子）。
    fn plan_builtin_only() -> AssemblyPlan {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).expect("内置不重名");
        b.build()
    }

    /// SP-46 端到端（本层）：启停之后，**三样活体一起变**——工具集合、片段、技能。
    ///
    /// 只测其中一样是不够的：真实故障形态正是"工具摘掉了、提示词里还在"。
    #[test]
    fn test_apply_swaps_tools_fragments_and_skills_together() {
        let fragments = agent_adapter::prompt::new_plugin_fragment_store();
        let skills = new_skill_store(Vec::new());
        let live = LivePluginContributions::new(&plan_builtin_only(), fragments.clone(), skills.clone());

        // 启用插件
        let delta = live.apply(
            &plan_with_plugin(),
            vec![("p1".to_string(), "style".to_string(), "一律用 tabs".to_string())],
            vec![skill("plugin:p1:deploy", "deploy")],
        );
        assert!(delta.added.iter().any(|l| l.contains("web_search")), "{delta:?}");
        assert_eq!(
            live.catalog().descriptors().len(),
            2,
            "活体注册表必须真的多出插件工具"
        );
        assert_eq!(
            fragments.read().map(|g| g.len()).unwrap_or(0),
            1,
            "片段活体要能被子系统读到（提示词每步读它）"
        );
        assert_eq!(live.plugin_skills().len(), 1, "技能活体同理");

        // 停用插件 → 三样一起还原
        let delta_off = live.apply(&plan_builtin_only(), Vec::new(), Vec::new());
        assert!(delta_off.removed.iter().any(|l| l.contains("web_search")), "{delta_off:?}");
        assert_eq!(live.catalog().descriptors().len(), 1);
        assert!(fragments.read().map(|g| g.is_empty()).unwrap_or(false));
        assert!(live.plugin_skills().is_empty());
    }

    /// 摘要要跟着活体走（收据的"之后"这一半不能是旧值）。
    #[test]
    fn test_digest_follows_the_live_state() {
        let live = LivePluginContributions::new(
            &plan_builtin_only(),
            agent_adapter::prompt::new_plugin_fragment_store(),
            new_skill_store(Vec::new()),
        );
        let before = live.digest();
        live.apply(&plan_with_plugin(), Vec::new(), Vec::new());
        let after = live.digest();
        assert_ne!(before, after, "重装配之后摘要必须变（否则收据是假的）");
    }

    /// 摘要必须**同时**看得到片段与技能：只看装配计划的话，宿主那条计划里
    /// 一个片段、一条技能都没有——收据会显示"无变化"，而提示词其实已经变了。
    #[test]
    fn test_digest_counts_plugin_fragments_and_skills_not_just_tools() {
        let live = LivePluginContributions::new(
            &plan_builtin_only(),
            agent_adapter::prompt::new_plugin_fragment_store(),
            new_skill_store(Vec::new()),
        );
        let delta = live.apply(
            &plan_with_plugin(),
            vec![("p1".to_string(), "style".to_string(), "一律用 tabs".to_string())],
            vec![skill("plugin:p1:deploy", "deploy")],
        );
        assert!(
            delta.added.iter().any(|l| l.contains("prompt:plugin:p1:style")),
            "片段必须出现在差异里：{delta:?}"
        );
        assert!(
            delta.added.iter().any(|l| l.contains("skill:plugin:p1:deploy")),
            "技能必须出现在差异里：{delta:?}"
        );
        assert!(delta.prompt_bytes_delta > 0, "片段字节数要如实统计：{delta:?}");
    }
}
