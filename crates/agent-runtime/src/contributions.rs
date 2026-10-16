//! 统一贡献模型与装配管线（P2 · SP-40）。
//!
//! # 为什么要有它
//!
//! 到 P1 为止，"什么东西参与这一轮"是**三条各写一份**的装配：
//! 工具在 `tools_for_toolkits` + `agent-host` 的两段拼装里、提示词在
//! `compose_system_prompt_full` 的硬编码段落里、技能在 `SkillManager::scan_skills` 里。
//! 三处的"谁赢、谁先、失败怎么办"各有一套，而**插件**要同时往三处塞东西——
//! 于是每加一类贡献都要动三个地方，且规则迟早不一致。
//!
//! 这里把它们收成一条管线：**收集 → 校验 → 排序 → 产出四项产物**
//! （`ToolCatalog` / `PromptPlan` / `SkillIndex` / `HookTable`）。
//!
//! # 三条规则（设计 §4.2 / §4.7）
//!
//! 1. **内置 vs 插件，失败方向不同**：内置重名 = 项目自己的声明打架 → **炸**（fail-fast）；
//!    插件重名/装不起来 = 第三方输入 → **跳过并记录**（fail-safe）。这与
//!    `tools_for_toolkits` 对"未知工具包"炸掉装配同一逻辑：**谁的声明谁负责**。
//! 2. **同名工具：插件胜出**（决策 1 的口径：插件只能重实现已注册的工具名），
//!    但**必须记入 `replaced_builtins`**——静默替换等于把用户信任的工具悄悄换掉。
//! 3. **排序稳定**：`order` 升序 → 内置优先 → `id` 字典序兜底。稳定是硬要求：
//!    提示词片段顺序变了，模型看到的东西就变了，而那是"行为变更"而非"重构"。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_base::ports::Tool;

/// 一条贡献的来源。埋点归因与"谁赢"的判据都用它。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContributionOrigin {
    /// 本项目自带（编译进来的实现）
    Builtin { name: String },
    /// 第三方/工作区插件
    Plugin { id: String },
}

impl ContributionOrigin {
    /// 由标签还原（ / ）。
    ///
    /// 为什么要有它：提示词片段（适配器）用字符串标签表示来源——适配器不依赖 runtime 的类型；
    /// 而贡献元数据用枚举。**两个表示法之间只允许这一处转换**，认不出的一律当外部来源（更保守）。
    pub fn from_label(label: &str) -> Self {
        match label.split_once(':') {
            Some(("plugin", id)) => Self::Plugin { id: id.to_string() },
            Some(("builtin", name)) => Self::Builtin { name: name.to_string() },
            _ => Self::Plugin { id: label.to_string() },
        }
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin { .. })
    }

    /// 稳定标签（埋点、日志、诊断面板过滤共用）。
    pub fn label(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Plugin { id } => format!("plugin:{id}"),
        }
    }
}

/// 贡献的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContributionKind {
    Tool,
    PromptFragment,
    Skill,
    Hook,
}

impl ContributionKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::PromptFragment => "prompt",
            Self::Skill => "skill",
            Self::Hook => "hook",
        }
    }
}

/// 失败策略：**谁的声明谁负责**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    /// 内置：声明打架就炸（装配期发现，而不是运行期行为诡异）
    FailFast,
    /// 插件：跳过这一条并记录（第三方输入不该让整个装配失败）
    FailSafe,
}

/// 一条贡献的元数据（不含载荷）——校验、排序、埋点都靠它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    pub id: String,
    pub kind: ContributionKind,
    pub origin: ContributionOrigin,
    /// 排序权重（小的在前；同值按"内置优先 → id"兜底）
    pub order: i32,
    pub failure: FailurePolicy,
}

impl Contribution {
    pub fn tool(origin: ContributionOrigin, name: impl Into<String>) -> Self {
        Self {
            id: name.into(),
            kind: ContributionKind::Tool,
            failure: if origin.is_builtin() {
                FailurePolicy::FailFast
            } else {
                FailurePolicy::FailSafe
            },
            origin,
            order: 0,
        }
    }

    pub fn prompt(origin: ContributionOrigin, id: impl Into<String>, order: i32) -> Self {
        Self {
            id: id.into(),
            kind: ContributionKind::PromptFragment,
            failure: FailurePolicy::FailSafe,
            origin,
            order,
        }
    }

    pub fn skill(origin: ContributionOrigin, id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: ContributionKind::Skill,
            failure: FailurePolicy::FailSafe,
            origin,
            order: 0,
        }
    }

    pub fn hook(origin: ContributionOrigin, point: impl Into<String>) -> Self {
        Self {
            id: point.into(),
            kind: ContributionKind::Hook,
            failure: FailurePolicy::FailSafe,
            origin,
            order: 0,
        }
    }
}

/// 被跳过的贡献（fail-safe 那一类）+ 理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedContribution {
    pub kind: ContributionKind,
    pub id: String,
    pub origin: ContributionOrigin,
    pub reason: String,
}

/// 提示词片段与计划：**直接复用适配器那份类型**（SP-41）。
///
/// 为什么不在这里另立一套：片段是适配器产出的（`build_prompt_plan`），管线只是**追加**
/// 与**排序**。两份等价类型就是"第二份名单"——迟早会出现"适配器改了字段、这里没跟着改"。
pub use agent_adapter::prompt::{PromptFragment, PromptPlan, PromptSegmentReport};

/// 技能条目（索引）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub origin: ContributionOrigin,
    pub enabled: bool,
    /// 正文（P2 先带全量；"索引 + 按需"是决策 4 之后的事）
    pub body: String,
}

/// 技能索引：同名按**小写**去重（与 `SkillManager` 的既有规则一致）。
#[derive(Debug, Clone, Default)]
pub struct SkillIndex {
    entries: Vec<SkillEntry>,
}

impl SkillIndex {
    pub fn entries(&self) -> &[SkillEntry] {
        &self.entries
    }

    pub fn enabled_count(&self) -> usize {
        self.entries.iter().filter(|e| e.enabled).count()
    }
}

/// 一条钩子注册（P3a 会把处理函数挂上来；SP-40 先把"表"与顺序定下来）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistration {
    pub point: String,
    pub origin: ContributionOrigin,
    pub order: i32,
}

/// 点位表：`point → 按顺序注册的钩子`。
#[derive(Debug, Clone, Default)]
pub struct HookTable {
    points: BTreeMap<String, Vec<HookRegistration>>,
}

impl HookTable {
    pub fn registrations(&self, point: &str) -> &[HookRegistration] {
        self.points.get(point).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn points(&self) -> impl Iterator<Item = &String> {
        self.points.keys()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// 装配产物（四项）。
pub struct AssemblyPlan {
    /// 装配好的工具（顺序已定：插件在前、内置在后，同名已去重）
    pub tools: Vec<Arc<dyn Tool>>,
    pub catalog: Arc<dyn agent_base::ports::ToolCatalog>,
    /// 注册表的**具体句柄**（与 `catalog` 是同一个对象）。
    ///
    /// 为什么要多存一个：SP-46 的受控重装配要 `register` / `unregister`，而那两个方法在
    /// 具体类型上（端口只有 `descriptors`/`resolve`/`validate`）。给端口加 `as_any` 是为了
    /// 向下转型而扩大端口面，不如在这里直接拿住具体类型——反正它本来就是本 crate 造的。
    pub tool_registry: Arc<crate::catalog::CompositeToolCatalog>,
    pub prompt: PromptPlan,
    pub skills: SkillIndex,
    pub hooks: HookTable,
    /// 被插件**顶掉**的内置工具名（要报审计，见 SP-16 的 `plugin.replaced_builtin`）
    pub replaced_builtins: Vec<String>,
    /// 跳过的贡献（fail-safe）与理由
    pub skipped: Vec<SkippedContribution>,
    /// 全部贡献的元数据（装配顺序）——"这轮装了什么"的可读答案
    pub contributions: Vec<Contribution>,
}

/// 装配期错误（**只有 fail-fast 的那一类**：内置自己的声明打架）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyError {
    pub message: String,
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AssemblyError {}

/// 装配管线：**收集**（各来源往里加）→ 校验 → 排序 → 产出四项产物。
#[derive(Default)]
pub struct AssemblyBuilder {
    builtin_tools: Vec<Arc<dyn Tool>>,
    plugin_tools: Vec<(String, Arc<dyn Tool>)>,
    prompt_fragments: Vec<PromptFragment>,
    skills: Vec<SkillEntry>,
    hooks: Vec<HookRegistration>,
}

impl AssemblyBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加一个**内置**工具（声明打架 = 内置自己的问题 → 立即报错）。
    pub fn add_builtin_tool(&mut self, tool: Arc<dyn Tool>) -> Result<(), AssemblyError> {
        let name = tool.descriptor().name.clone();
        if self.builtin_tools.iter().any(|t| t.descriptor().name == name) {
            return Err(AssemblyError {
                message: format!(
                    "内置工具重名：`{name}`——两个工具包提供了同名工具，装配期就该炸，\
                     而不是让 catalog 出现两份同名描述符"
                ),
            });
        }
        self.builtin_tools.push(tool);
        Ok(())
    }

    /// 加一个**插件**工具（fail-safe：与内置同名时**插件胜出**并记入替换审计；
    /// 与别的插件同名时**跳过后来的那个**，设计 §4.7"重名拒绝加载"）。
    pub fn add_plugin_tool(&mut self, plugin_id: impl Into<String>, tool: Arc<dyn Tool>) {
        self.plugin_tools.push((plugin_id.into(), tool));
    }

    pub fn add_prompt_fragment(&mut self, fragment: PromptFragment) {
        self.prompt_fragments.push(fragment);
    }

    pub fn add_skill(&mut self, entry: SkillEntry) {
        self.skills.push(entry);
    }

    pub fn add_hook(&mut self, registration: HookRegistration) {
        self.hooks.push(registration);
    }

    /// 产出四项产物。
    pub fn build(mut self) -> AssemblyPlan {
        let mut skipped = Vec::new();
        let mut contributions = Vec::new();

        // ── 工具：同名时插件胜出（内置让位），插件之间先到先得 ──────────────
        let mut replaced_builtins: Vec<String> = Vec::new();
        let plugin_names: std::collections::BTreeSet<String> = self
            .plugin_tools
            .iter()
            .map(|(_, t)| t.descriptor().name.clone())
            .collect();
        let mut seen_plugin_names = std::collections::BTreeSet::new();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        for (plugin_id, tool) in self.plugin_tools.drain(..) {
            let name = tool.descriptor().name.clone();
            if !seen_plugin_names.insert(name.clone()) {
                skipped.push(SkippedContribution {
                    kind: ContributionKind::Tool,
                    id: name.clone(),
                    origin: ContributionOrigin::Plugin {
                        id: plugin_id.clone(),
                    },
                    reason: "duplicate_tool".to_string(),
                });
                continue;
            }
            if plugin_names.contains(&name)
                && self
                    .builtin_tools
                    .iter()
                    .any(|t| t.descriptor().name == name)
            {
                replaced_builtins.push(name.clone());
            }
            contributions.push(Contribution::tool(
                ContributionOrigin::Plugin { id: plugin_id },
                &name,
            ));
            tools.push(tool);
        }
        for tool in self.builtin_tools.drain(..) {
            let name = tool.descriptor().name.clone();
            if plugin_names.contains(&name) {
                // 插件胜出：内置让位（替换审计在上面记了）
                continue;
            }
            contributions.push(Contribution::tool(
                ContributionOrigin::Builtin { name: name.clone() },
                &name,
            ));
            tools.push(tool);
        }

        // ── 提示词片段：进适配器的 （**同一个类型**）──────────────
        //
        // 排序由  自己保证（order → id 稳定）；
        // 这里不重复一份排序规则——两处排序迟早会不一致。
        let mut prompt = PromptPlan::new();
        for f in self.prompt_fragments.drain(..) {
            prompt.push(f);
        }
        for f in prompt.fragments() {
            // 来源标签 → 贡献来源：**这一处**转换（两个表示法之间不各写一份）
            contributions.push(Contribution::prompt(
                ContributionOrigin::from_label(&f.source),
                f.id.clone(),
                f.order,
            ));
        }

        // ── 技能：同名（小写）去重，先到先得 ────────────────────────────────
        let mut seen_skill_names: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let mut skills = SkillIndex::default();
        for entry in self.skills.drain(..) {
            if !seen_skill_names.insert(entry.name.to_lowercase()) {
                skipped.push(SkippedContribution {
                    kind: ContributionKind::Skill,
                    id: entry.id.clone(),
                    origin: entry.origin.clone(),
                    reason: "duplicate_skill_name".to_string(),
                });
                continue;
            }
            contributions.push(Contribution::skill(entry.origin.clone(), entry.id.clone()));
            skills.entries.push(entry);
        }

        // ── 钩子：按点位分组，组内按 order → id 稳定排序 ─────────────────────
        self.hooks.sort_by(|a, b| {
            a.point
                .cmp(&b.point)
                .then_with(|| a.order.cmp(&b.order))
                .then_with(|| a.origin.label().cmp(&b.origin.label()))
        });
        let mut hooks = HookTable::default();
        for reg in self.hooks.drain(..) {
            contributions.push(Contribution::hook(reg.origin.clone(), reg.point.clone()));
            hooks.points.entry(reg.point.clone()).or_default().push(reg);
        }

        let tool_registry = Arc::new(crate::catalog::CompositeToolCatalog::new(tools.clone(), None));
        let catalog: Arc<dyn agent_base::ports::ToolCatalog> = tool_registry.clone();

        AssemblyPlan {
            tools,
            catalog,
            tool_registry,
            prompt,
            skills,
            hooks,
            replaced_builtins,
            skipped,
            contributions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor, ToolReceipt, ToolCall};
    use agent_base::ports::{BoxFuture, ToolContext};

    struct StubTool {
        descriptor: ToolDescriptor,
    }

    impl StubTool {
        fn new(name: &str) -> Self {
            Self {
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
            }
        }
    }

    impl Tool for StubTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        fn execute<'a>(&'a self, _c: &'a ToolCall, _x: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async { ToolReceipt::success("ok", 0, 1) })
        }
    }

    fn tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(StubTool::new(name))
    }

    /// 规则 1：**内置重名 → 炸**（fail-fast，项目自己的声明打架）。
    #[test]
    fn test_duplicate_builtin_tool_fails_fast() {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).expect("第一个应当成功");
        let err = b.add_builtin_tool(tool("read_file")).expect_err("重名必须报错");
        assert!(err.message.contains("内置工具重名"), "{err}");
    }

    /// 规则 2：**同名时插件胜出**，且**必须记入替换审计**（静默替换不可接受）。
    #[test]
    fn test_plugin_replaces_builtin_and_records_the_replacement() {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).unwrap();
        b.add_builtin_tool(tool("write_file")).unwrap();
        b.add_plugin_tool("p1", tool("read_file"));

        let plan = b.build();
        assert_eq!(
            plan.replaced_builtins,
            vec!["read_file".to_string()],
            "被顶掉的内置工具必须如实记下来"
        );
        let names: Vec<String> = plan
            .catalog
            .descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names.len(), 2, "同名只该留一份：{names:?}");
        // 留下来的那个是插件实现（来源可从 contributions 里看出来）
        assert!(plan.contributions.iter().any(|c| {
            c.kind == ContributionKind::Tool
                && c.id == "read_file"
                && matches!(&c.origin, ContributionOrigin::Plugin { id } if id == "p1")
        }));
    }

    /// 规则：插件之间重名 → **跳过后来的那个**（设计 §4.7"重名拒绝加载"，不是覆盖）。
    #[test]
    fn test_duplicate_plugin_tool_is_skipped_with_reason() {
        let mut b = AssemblyBuilder::new();
        b.add_plugin_tool("p1", tool("read_file"));
        b.add_plugin_tool("p2", tool("read_file"));
        let plan = b.build();
        assert_eq!(plan.skipped.len(), 1, "{:?}", plan.skipped);
        assert_eq!(plan.skipped[0].reason, "duplicate_tool");
        assert_eq!(plan.skipped[0].id, "read_file");
        assert_eq!(plan.catalog.descriptors().len(), 1);
    }

    /// 排序**必须稳定**：order 升序 → 内置优先 → id 兜底。顺序变了就是行为变更。
    #[test]
    fn test_prompt_fragments_sort_order_builtin_then_plugin_then_id() {
        let mut b = AssemblyBuilder::new();

        // 乱序加入，验证结果与加入顺序无关
        b.add_prompt_fragment(PromptFragment {
            id: "z_plugin_late".into(),
            source: "plugin:p1".to_string(),
            order: 20,
            content: "plugin late".into(),
        });
        b.add_prompt_fragment(PromptFragment {
            id: "b_plugin_same".into(),
            source: "plugin:p2".to_string(),
            order: 10,
            content: "plugin same".into(),
        });
        b.add_prompt_fragment(PromptFragment {
            id: "a_builtin_same".into(),
            source: "builtin:core".to_string(),
            order: 10,
            content: "builtin same".into(),
        });

        let plan = b.build();
        let ids: Vec<&str> = plan.prompt.fragments().iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a_builtin_same", "b_plugin_same", "z_plugin_late"],
            "同 order 时内置在前、其余按 id"
        );

        // 字节报告要能回答"这段是谁加的、占多少"
        let report = plan.prompt.byte_report();
        assert_eq!(report[0].source, "builtin:core");
        assert_eq!(report[0].bytes, "builtin same".len());
    }

    /// 渲染是**纯拼接**：分隔符归片段自己（这样"提示词一个字节不变"才是可验证的，
    /// 而不是靠拼接规则恰好等于老排版）。
    #[test]
    fn test_prompt_render_is_plain_concatenation_and_empty_fragments_add_nothing() {
        let mut b = AssemblyBuilder::new();
        // 第二段自己带 "\n\n" 前缀——空白由片段负责
        for (id, order, content) in [("one", 1, "A"), ("two", 2, "\n\nB"), ("empty", 3, "")] {
            b.add_prompt_fragment(PromptFragment {
                id: id.into(),
                source: "builtin:core".to_string(),
                order,
                content: content.into(),
            });
        }
        let plan = b.build();
        assert_eq!(plan.prompt.render(), "A\n\nB");
        assert_eq!(plan.prompt.total_bytes(), "A\n\nB".len(), "空片段零贡献");
    }

    /// 技能：同名按小写去重（与 `SkillManager` 的规则一致），后来的跳过并记理由。
    #[test]
    fn test_duplicate_skill_name_is_skipped_case_insensitively() {
        let mut b = AssemblyBuilder::new();
        let plug = || ContributionOrigin::Plugin { id: "p1".into() };

        b.add_skill(SkillEntry {
            id: "workspace:TDD".into(),
            name: "TDD".into(),
            origin: plug(),
            enabled: true,
            body: "A".into(),
        });
        b.add_skill(SkillEntry {
            id: "plugin:tdd".into(),
            name: "tdd".into(),
            origin: plug(),
            enabled: true,
            body: "B".into(),
        });
        let plan = b.build();
        assert_eq!(plan.skills.entries().len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, "duplicate_skill_name");
    }

    /// 钩子表：按点位分组、组内稳定排序（P3a 往里挂处理函数前，先把顺序定下来）。
    #[test]
    fn test_hook_table_groups_by_point_and_sorts_stably() {
        let mut b = AssemblyBuilder::new();
        for (point, order, origin) in [
            ("beforeToolCall", 20, ContributionOrigin::Plugin { id: "p1".into() }),
            ("beforeToolCall", 10, ContributionOrigin::Builtin { name: "core".into() }),
            ("afterToolCall", 10, ContributionOrigin::Plugin { id: "p2".into() }),
        ] {
            b.add_hook(HookRegistration {
                point: point.to_string(),
                origin,
                order,
            });
        }
        let plan = b.build();
        let before: Vec<String> = plan
            .hooks
            .registrations("beforeToolCall")
            .iter()
            .map(|r| r.origin.label())
            .collect();
        assert_eq!(
            before,
            vec!["builtin:core".to_string(), "plugin:p1".to_string()],
            "同点位内按 order 排序"
        );
        assert_eq!(plan.hooks.registrations("afterToolCall").len(), 1);
        assert!(plan.hooks.registrations("noSuchPoint").is_empty());
    }

    /// 装配产物要能回答"这轮到底装了什么"（埋点与诊断都靠它）。
    #[test]
    fn test_contributions_record_every_kind_with_origin() {
        let mut b = AssemblyBuilder::new();
        b.add_builtin_tool(tool("read_file")).unwrap();
        b.add_prompt_fragment(PromptFragment {
            id: "guidelines".into(),
            source: "builtin:core".to_string(),
            order: 1,
            content: "x".into(),
        });
        b.add_skill(SkillEntry {
            id: "s1".into(),
            name: "s1".into(),
            origin: ContributionOrigin::Builtin { name: "builtin".into() },
            enabled: true,
            body: "b".into(),
        });
        let plan = b.build();
        let kinds: Vec<&str> = plan
            .contributions
            .iter()
            .map(|c| c.kind.label())
            .collect();
        assert!(kinds.contains(&"tool") && kinds.contains(&"prompt") && kinds.contains(&"skill"));
        assert!(plan.contributions.iter().all(|c| !c.origin.label().is_empty()));
    }
}
