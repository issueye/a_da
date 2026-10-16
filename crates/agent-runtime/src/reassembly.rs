//! 受控重装配与贡献收据（P2 · SP-46）。
//!
//! # 要解决的问题
//!
//! 插件启停（界面上的开关）此前只改了一个布尔值：`PluginManager::toggle_plugin` 往配置里
//! 写 `enabled = false`，然后**什么都不发生**——引擎手里那份工具集合、提示词片段与技能
//! 列表还是启动时装配的那份。用户看到的是"关掉了，但它还在跑"；而设计文档 §5.9 的
//! `afterPluginToggle` 要报的是"**实际生效的贡献集合**"，那份数据此前根本不存在。
//!
//! 这个模块提供两样东西：
//!
//! 1. **贡献摘要与差异**（[`ContributionDigest`] / [`ContributionDelta`]）：把一条
//!    [`AssemblyPlan`] 压成"装了哪些工具/片段/技能/钩子"，两份一比就是收据。
//! 2. **重装配条目**（[`LiveContributions::apply`]）：把新计划**真的换进活着的工具集合**
//!    （走注册表既有的 `register` / `unregister`），并如实报出增删。
//!
//! # 口径
//!
//! - **摘要与差异是纯函数**：不碰 IO、不碰全局，测试里可以直接比字符串。
//! - **工具集合的替换按名字**：同名即"换实现"（不算新增），名字消失才算移除。
//!   这与决策 1（插件只能重实现已注册的名字）一致。
//! - **替换内置必须出现在差异里**（`replaced_builtins`）：静默把用户的 `read_file`
//!   换成插件实现是这套设计明确要防的事。

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::catalog::CompositeToolCatalog;
use crate::contributions::{AssemblyPlan, ContributionOrigin};
use agent_base::ports::ToolCatalog;

/// 一条贡献的**稳定标签**：`<kind>:<归属>:<id>`。
///
/// 差异比较、收据文本、诊断面板过滤共用它。写成带归属的形态，是为了让
/// "内置的 `read_file` 被换成了插件 1 的 `read_file`"在差异里**看得出来**，
/// 而不是只显示一条不变的名字。
fn label(kind: &str, origin: &ContributionOrigin, id: &str) -> String {
    format!("{kind}:{}:{id}", origin.label())
}

/// 一次装配的**贡献摘要**（不含载荷，只有身份）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributionDigest {
    pub tools: Vec<String>,
    pub prompts: Vec<String>,
    pub skills: Vec<String>,
    pub hooks: Vec<String>,
    /// 提示词片段的总字节数（"提示词为什么变长了"要能一眼看出）
    pub prompt_bytes: u64,
}

impl ContributionDigest {
    /// 从装配产物压缩出摘要（顺序稳定：按标签字典序）。
    ///
    /// 稳定是硬要求：摘要会进收据、进诊断、进测试断言；顺序飘了，两次装配的收据就没法比。
    pub fn of(plan: &AssemblyPlan) -> Self {
        let mut tools = Vec::new();
        let mut prompts = Vec::new();
        let mut skills = Vec::new();
        let mut hooks = Vec::new();

        for c in &plan.contributions {
            let l = label(c.kind.label(), &c.origin, &c.id);
            match c.kind {
                crate::contributions::ContributionKind::Tool => tools.push(l),
                crate::contributions::ContributionKind::PromptFragment => prompts.push(l),
                crate::contributions::ContributionKind::Skill => skills.push(l),
                crate::contributions::ContributionKind::Hook => hooks.push(l),
            }
        }
        tools.sort();
        prompts.sort();
        skills.sort();
        hooks.sort();

        Self {
            tools,
            prompts,
            skills,
            hooks,
            prompt_bytes: plan.prompt.total_bytes() as u64,
        }
    }

    pub fn total(&self) -> usize {
        self.tools.len() + self.prompts.len() + self.skills.len() + self.hooks.len()
    }

    /// 由**一次插件收集**造摘要（SP-46 的启停收据用这个）。
    ///
    /// 与 [`Self::of`] 的分工写清楚，免得两份摘要各说各话：
    /// - 工具：取自装配计划——宿主那条计划里内置与插件工具都在，两次收据里内置部分相同，
    ///   在差异里自然抵消（不需要在这里过滤，过滤反而容易漏掉"插件顶掉了哪个内置"）；
    /// - 片段 / 技能：取自收集结果本身，**没有**过提示词渲染那一层。因此这里的
    ///   `prompt_bytes` 是**片段正文的字节数**（不含适配器加的分隔符），
    ///   它回答的是"插件带来的提示词变长了还是短了"，不是"整份提示词多长"。
    pub fn of_collection(
        plan: &AssemblyPlan,
        fragments: &[(String, String, String)],
        skills: &[(String, String)],
    ) -> Self {
        let tools: Vec<String> = plan
            .contributions
            .iter()
            .filter(|c| c.kind == crate::contributions::ContributionKind::Tool)
            .map(|c| label(c.kind.label(), &c.origin, &c.id))
            .collect();
        let mut prompts: Vec<String> = fragments
            .iter()
            .map(|(pid, id, _)| format!("prompt:plugin:{pid}:{id}"))
            .collect();
        let mut skill_labels: Vec<String> = skills
            .iter()
            .map(|(pid, name)| format!("skill:plugin:{pid}:{name}"))
            .collect();
        prompts.sort();
        skill_labels.sort();

        let mut tools_sorted = tools;
        tools_sorted.sort();

        Self {
            tools: tools_sorted,
            prompts,
            skills: skill_labels,
            hooks: Vec::new(),
            prompt_bytes: fragments
                .iter()
                .map(|(_, _, content)| content.trim().len() as u64)
                .sum(),
        }
    }
}

/// 两份摘要之间的差异（**收据的主体**）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributionDelta {
    /// 新出现的贡献（带 kind 前缀的稳定标签）
    pub added: Vec<String>,
    /// 消失的贡献
    pub removed: Vec<String>,
    /// 这次装配里被插件顶掉的内置工具名（审计用，见 `plugin.replaced_builtin`）
    pub replaced_builtins: Vec<String>,
    /// 提示词字节数的变化（正数 = 变长）
    pub prompt_bytes_delta: i64,
}

impl ContributionDelta {
    pub fn between(before: &ContributionDigest, after: &ContributionDigest) -> Self {
        let diff = |a: &[String], b: &[String]| -> (Vec<String>, Vec<String>) {
            let set_a: BTreeSet<&String> = a.iter().collect();
            let set_b: BTreeSet<&String> = b.iter().collect();
            (
                set_b
                    .difference(&set_a)
                    .map(|s| (*s).clone())
                    .collect::<Vec<_>>(),
                set_a
                    .difference(&set_b)
                    .map(|s| (*s).clone())
                    .collect::<Vec<_>>(),
            )
        };

        let mut added = Vec::new();
        let mut removed = Vec::new();
        for (a, b) in [
            (&before.tools, &after.tools),
            (&before.prompts, &after.prompts),
            (&before.skills, &after.skills),
            (&before.hooks, &after.hooks),
        ] {
            let (plus, minus) = diff(a, b);
            added.extend(plus);
            removed.extend(minus);
        }
        added.sort();
        removed.sort();

        Self {
            added,
            removed,
            replaced_builtins: Vec::new(),
            prompt_bytes_delta: after.prompt_bytes as i64 - before.prompt_bytes as i64,
        }
    }

    /// 没有任何变化（工具、提示词、技能、钩子、字节数都没动）。
    ///
    /// 界面据此区分"这次启停**真的**改了什么"与"点了但没影响"——后者同样是信息。
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.prompt_bytes_delta == 0
    }

    /// 一行话的收据（日志与诊断用）。
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "贡献集合无变化".to_string();
        }
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("新增 {} 条（{}）", self.added.len(), self.added.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("移除 {} 条（{}）", self.removed.len(), self.removed.join(", ")));
        }
        if self.prompt_bytes_delta != 0 {
            parts.push(format!("提示词字节 {:+#}", self.prompt_bytes_delta));
        }
        parts.join("；")
    }
}

/// 一次启停请求（设计 §5.9 的 `PluginToggleRequest`：**事前**拿得到的那份事实）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToggleRequest {
    pub plugin_id: String,
    pub scope: String,
    pub from_enabled: bool,
    pub to_enabled: bool,
}

/// 重装配收据：启停**之后**实际生效的贡献集合与差异。
///
/// P3a 的 `afterPluginToggle` 报的就是这个结构；SP-46 先把它做出来并接到
/// `plugin.setEnabled` 的回执上，于是"勾一下开关"有了可验证的后果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReassemblyReport {
    pub request: PluginToggleRequest,
    pub before: ContributionDigest,
    pub after: ContributionDigest,
    pub delta: ContributionDelta,
    /// 装不起来的插件（fail-safe：如实报出，不静默）
    pub skipped: Vec<String>,
    /// 是否真的换进了活着的集合（没有活体句柄时为 false——**如实**，不是假装成功）
    pub applied: bool,
}

/// 活着的工具集合 + 当前摘要：**受控重装配的唯一入口**。
///
/// 「受控」的含义：换进来的必须是一条完整 [`AssemblyPlan`]（走同一条装配管线），
/// 而不是调用方自己往注册表里塞几个工具——那样"谁改了什么"就再也说不清了。
pub struct LiveContributions {
    catalog: Arc<CompositeToolCatalog>,
    current: std::sync::Mutex<ContributionDigest>,
}

impl LiveContributions {
    /// 用装配产物建一个活体句柄（注册表**复用**计划里的那个，见 `AssemblyPlan::catalog`）。
    ///
    /// 复用而不是新建：引擎手里那份 `Arc<dyn ToolCatalog>` 正是计划里的这一个，
    /// 另建一个就成了"重装配换的是没人用的那本账"。
    pub fn new(plan: &AssemblyPlan) -> Self {
        let catalog = catalog_of(plan);
        Self {
            catalog,
            current: std::sync::Mutex::new(ContributionDigest::of(plan)),
        }
    }

    /// 交给 `ProductBuilder` 的注册表句柄（与引擎手里那份**是同一个**）。
    pub fn catalog(&self) -> Arc<CompositeToolCatalog> {
        self.catalog.clone()
    }

    /// 当前生效的贡献摘要。
    pub fn digest(&self) -> ContributionDigest {
        self.current
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// **受控重装配**：把新计划换进活着的工具集合，返回差异。
    ///
    /// 工具按名字对齐：新计划里有的 `register`（同名 = 换实现），旧的有而新的没有
    /// `unregister`（= 插件的工具真被摘掉了）。名字对齐而不是"清单替换"，
    /// 是因为引擎已经拿着这个 `Arc` 在跑，换不掉它、只能改它。
    pub fn apply(&self, plan: &AssemblyPlan) -> ContributionDelta {
        let after = ContributionDigest::of(plan);
        let mut delta = ContributionDelta::between(&self.digest(), &after);

        let current_names: BTreeSet<String> = name_set(self.catalog.descriptors());
        let next_names: BTreeSet<String> =
            name_set(plan.tools.iter().map(|t| t.descriptor().clone()).collect());

        for tool in &plan.tools {
            self.catalog.register(tool.clone());
        }
        for name in current_names.difference(&next_names) {
            self.catalog.unregister(name);
        }

        // 替换内置是**审计事实**，不是差异：名字没变，但实现换了（见模块说明）
        delta.replaced_builtins = plan.replaced_builtins.clone();

        if let Ok(mut cur) = self.current.lock() {
            *cur = after;
        }
        delta
    }
}

fn name_set(descriptors: Vec<agent_base::domain::ToolDescriptor>) -> BTreeSet<String> {
    descriptors.into_iter().map(|d| d.name).collect()
}

/// 取出计划里的注册表句柄。
///
/// 用的是计划里的**具体句柄**（`AssemblyPlan::tool_registry`）——它与计划里的
/// `catalog` 是同一个对象，也就是引擎手里那一个。没有它就只能向下转型，而端口上
/// 不该为了这个目的长出 `as_any`。
fn catalog_of(plan: &AssemblyPlan) -> Arc<CompositeToolCatalog> {
    plan.tool_registry.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributions::{AssemblyBuilder, SkillEntry};
    use agent_base::domain::{
        Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
        ToolReceipt,
    };
    use agent_base::ports::{BoxFuture, Tool, ToolCatalog, ToolContext};

    struct StubTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for StubTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute<'a>(
            &'a self,
            _c: &'a ToolCall,
            _x: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async { ToolReceipt::success("ok", 0, 1) })
        }
    }

    fn tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(StubTool {
            descriptor: ToolDescriptor {
                name: name.to_string(),
                summary: format!("{name} 的说明"),
                schema: serde_json::json!({ "type": "object" }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        })
    }

    /// 装配一条计划：`plugin_tools` 是 `(插件 id, 工具名)`，`skill` 是插件技能。
    fn plan(plugin_tools: &[(&str, &str)], plugin_skill: Option<&str>, plugin_prompt: bool) -> AssemblyPlan {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).expect("内置不重名");
        b.add_builtin_tool(tool("list_files")).expect("内置不重名");
        for (pid, name) in plugin_tools {
            b.add_plugin_tool(*pid, tool(name));
        }
        if plugin_prompt {
            b.add_prompt_fragment(crate::contributions::PromptFragment {
                id: "house-style".to_string(),
                source: "plugin:p1".to_string(),
                order: 300,
                content: "本项目一律用 tabs。".to_string(),
            });
        }
        if let Some(name) = plugin_skill {
            b.add_skill(SkillEntry {
                id: format!("plugin:p1:{name}"),
                name: name.to_string(),
                origin: ContributionOrigin::Plugin { id: "p1".to_string() },
                enabled: true,
                body: "正文".to_string(),
            });
        }
        b.build()
    }

    /// SP-46 的核心：启用/停用**真的改变贡献集合**，而且差异说得清增删。
    #[test]
    fn test_toggle_really_changes_the_live_tool_set_and_reports_the_delta() {
        let with_plugin = plan(&[("p1", "web_search")], Some("deploy-runbook"), true);
        let without = plan(&[], None, false);

        let live = LiveContributions::new(&without);
        assert_eq!(live.catalog().descriptors().len(), 2, "起始只有两个内置工具");

        // 启用插件 → 多出一个工具、一条片段、一条技能
        let on = live.apply(&with_plugin);
        assert!(
            on.added.iter().any(|l| l.contains("web_search")),
            "启用的插件工具必须出现在差异里：{on:?}"
        );
        assert!(on.added.iter().any(|l| l.contains("deploy-runbook")));
        assert!(on.prompt_bytes_delta > 0, "加了提示词片段，字节数必须变长：{on:?}");
        let names: Vec<String> = live
            .catalog()
            .descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(names.contains(&"web_search".to_string()), "活体集合要真的多出这个工具：{names:?}");

        // 停用插件 → 工具被摘掉，且差异如实报出移除
        let off = live.apply(&without);
        assert!(
            off.removed.iter().any(|l| l.contains("web_search")),
            "停用必须报出被移除的工具：{off:?}"
        );
        let names_after: Vec<String> = live
            .catalog()
            .descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(
            !names_after.contains(&"web_search".to_string()),
            "停用之后它不能再留在活体集合里（否则就是'关掉了却还在跑'）：{names_after:?}"
        );
        assert!(off.prompt_bytes_delta < 0, "提示词片段也没了：{off:?}");
    }

    /// 同名工具被插件重实现时：差异里要能看出**归属换了**，并且带上替换审计。
    ///
    /// 注意差异长什么样：`tool:builtin:read_file:read_file` 消失、`tool:plugin:p1:read_file`
    /// 出现。这不是"多了一条"，而是"同一个名字换了实现"——标签带归属正是为了让它可见
    /// （只比名字的话，这里会显示"什么都没变"，而用户的 `read_file` 已经被换掉了）。
    #[test]
    fn test_replaced_builtin_is_reported_in_the_delta() {
        let replaced = plan(&[("p1", "read_file")], None, false);
        let live = LiveContributions::new(&plan(&[], None, false));
        let delta = live.apply(&replaced);
        assert_eq!(
            delta.replaced_builtins,
            vec!["read_file".to_string()],
            "内置被插件顶掉必须报出来（静默替换是明令要防的）：{delta:?}"
        );
        assert!(
            delta.added.iter().any(|l| l.contains("plugin:p1:read_file")),
            "新实现要出现在新增里：{delta:?}"
        );
        assert!(
            delta.removed.iter().any(|l| l.contains("builtin:read_file")),
            "被顶掉的内置实现要出现在移除里：{delta:?}"
        );
        // 工具名字集合本身没变（引擎查 `read_file` 照样查得到），换的是实现
        let names: Vec<String> = live
            .catalog()
            .descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["read_file".to_string(), "list_files".to_string()]);
    }

    /// 摘要与差异是纯函数：顺序稳定、可比较（收据会被写进测试与诊断）。
    #[test]
    fn test_digest_is_stable_and_delta_detects_no_change() {
        let p = plan(&[("p1", "web_search")], Some("s1"), true);
        let a = ContributionDigest::of(&p);
        let b = ContributionDigest::of(&p);
        assert_eq!(a, b, "同一份计划两次压缩必须完全一致");

        let delta = ContributionDelta::between(&a, &b);
        assert!(delta.is_empty(), "自己跟自己比应当没有变化：{delta:?}");
        assert_eq!(delta.describe(), "贡献集合无变化");

        // 差异的增删数量要能对上总数变化
        let empty = ContributionDigest::default();
        let growing = ContributionDelta::between(&empty, &a);
        assert_eq!(growing.added.len(), a.total());
        assert!(growing.removed.is_empty());
    }

    /// 停用**别的**插件不该动到无关贡献（差异必须精确到条目）。
    #[test]
    fn test_delta_is_scoped_to_the_changed_plugin() {
        let two = plan(&[("p1", "web_search"), ("p2", "send_mail")], Some("s1"), true);
        let one = plan(&[("p1", "web_search")], Some("s1"), true);
        let delta = ContributionDelta::between(
            &ContributionDigest::of(&two),
            &ContributionDigest::of(&one),
        );
        assert_eq!(delta.removed.len(), 1, "只该移除 p2 的那一条：{delta:?}");
        assert!(delta.removed[0].contains("send_mail"));
        assert!(delta.added.is_empty());
        assert_eq!(delta.prompt_bytes_delta, 0, "片段没动，字节数也不该动");
    }
}
