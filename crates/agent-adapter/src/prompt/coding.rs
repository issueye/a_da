//! 系统提示词适配器：把"人格文本怎么排版"收敛到唯一真源。
//!
//! 设计口径（`docs/plans/wiring.md` §5 W1-T3）：
//! 人格文本原先硬编码在 `agent_core::runner::prompt::build_system_prompt`，
//! 且**与插件扫描纠缠在一起**——直接整段搬进适配器会形成
//! `agent-core → agent-adapter → agent-core` 的循环依赖。
//!
//! 因此这里按"**排版归适配器、数据归调用方**"拆开：
//! - [`compose_system_prompt`]：纯函数，输入（工作区 + 已启用扩展列表），输出提示词；
//! - [`CodingPromptSource`]：实现 `PromptSource` 端口，持有这两项数据。
//!
//! "哪些扩展启用了"仍由 `agent-core` 侧采集（`PluginManager::scan_plugins`），
//! 通过兼容 shim 传进来——这样**行为与搬运前逐字节一致**，
//! 而提示词的排版规则从此只有一处（R2）。

use agent_base::ports::PromptSource;

use super::plan::{PromptFragment, PromptPlan};

/// 产品身份（来自 `agent.spec.json` 的 `identity`，W5-T4）。
///
/// 这是 `identity` 声明的**真实消费者**：产品声明的名字/人格/语言必须真的进入系统提示词，
/// 否则"声明了一个产品人格，模型却看不到"——正是本计划要清掉的那类脱钩。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductIdentity {
    pub name: String,
    /// 人格文本（spec 里的 `persona`）
    pub persona: String,
    /// BCP-47 语言标签（如 `zh-CN`）
    pub locale: String,
}

impl ProductIdentity {
    pub fn new(
        name: impl Into<String>,
        persona: impl Into<String>,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            persona: persona.into(),
            locale: locale.into(),
        }
    }

    /// 是否是可用的身份声明（名字为空视为未声明）。
    pub fn is_declared(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// 未声明身份时的默认首行（与搬运前逐字节一致）。
const DEFAULT_INTRO: &str = "你是 a-da，一个由 Rust 原生核心驱动的高性能 AI 编程智能体。";
/// 未声明身份/中文环境时的默认语言指引（与搬运前逐字节一致）。
const DEFAULT_LANG_LINE: &str = "4. 所有的回复都使用清晰、专业的中文表达。";

/// 人格为空时给一句兜底描述（避免出现"你是 X，"这种半截句子）。
const FALLBACK_PERSONA: &str = "一个由 Rust 原生核心驱动的高性能 AI 编程智能体";

fn intro_for(identity: Option<&ProductIdentity>) -> String {
    match identity {
        Some(id) if id.is_declared() => {
            let persona = if id.persona.trim().is_empty() {
                FALLBACK_PERSONA
            } else {
                id.persona.trim()
            };
            format!("你是 {}，{}", id.name.trim(), persona)
        }
        _ => DEFAULT_INTRO.to_string(),
    }
}

fn language_line_for(identity: Option<&ProductIdentity>) -> String {
    match identity {
        Some(id) if id.is_declared() && !id.locale.trim().is_empty() => {
            let locale = id.locale.trim();
            if locale.to_ascii_lowercase().starts_with("zh") {
                // 中文环境：保持与搬运前逐字节一致
                DEFAULT_LANG_LINE.to_string()
            } else {
                format!("4. 请使用与语言环境 `{locale}` 一致的语言回复。")
            }
        }
        _ => DEFAULT_LANG_LINE.to_string(),
    }
}

/// 组合编码助手的系统提示词。
///
/// `extensions` 是"已启用的扩展工具"列表 `(name, description)`；
/// 注入提示词的技能项（名称、描述、适用场景、正文规范）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPrompt {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub content: String,
    /// 这条技能**是谁带来的**（P2 · SP-45）：`None` = 内置排版口径，`Some("plugin:<id>")` =
    /// 插件声明。
    ///
    /// 它只影响 `PromptPlan` 里的来源标注（埋点归因），**不影响渲染出来的字节**——
    /// 逐字节黄金夹具因此不受影响。
    pub source: Option<String>,
}

impl SkillPrompt {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            when_to_use: None,
            content: content.into(),
            source: None,
        }
    }

    pub fn with_when_to_use(mut self, when: impl Into<String>) -> Self {
        self.when_to_use = Some(when.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// 为空时不追加扩展段落——这与搬运前的行为一致。
pub fn compose_system_prompt(workspace: &str, extensions: &[(String, String)]) -> String {
    compose_system_prompt_with_identity(workspace, extensions, None)
}

/// 同上，但**把产品声明的身份接进提示词**（W5-T4）。
///
/// `identity` 为 `None` / 未声明名字时，输出与 [`compose_system_prompt`] **逐字节一致**
/// （所以既有断言不需要放宽）。
pub fn compose_system_prompt_with_identity(
    workspace: &str,
    extensions: &[(String, String)],
    identity: Option<&ProductIdentity>,
) -> String {
    compose_system_prompt_full(workspace, extensions, identity, &[])
}

/// 完整系统提示词拼接函数（= [`build_prompt_plan`] + `render()`）。
///
/// 保留这个入口是为了**行为不变**：它在 `build_prompt_plan` 之前就存在，既有调用点与
/// 逐字节黄金夹具都按它写。片段化的收益（来源、字节数、插件可追加）走 `build_prompt_plan`。
pub fn compose_system_prompt_full(
    workspace: &str,
    extensions: &[(String, String)],
    identity: Option<&ProductIdentity>,
    skills: &[SkillPrompt],
) -> String {
    build_prompt_plan(workspace, extensions, identity, skills).render()
}

/// 把系统提示词拆成**有序片段**（SP-41）。
///
/// 每段自带分隔符（见 [`plan`] 模块说明），所以 `render()` 是纯拼接、结果与
/// `compose_system_prompt_full` **逐字节一致**（由 `tests/prompt_golden.rs` 的黄金夹具守着）。
///
/// 片段的 `source` 一律标 `builtin:core`：这几段是编码助手的固有排版；
/// 插件追加的片段（SP-44）会带 `plugin:<id>`，于是埋点能回答"这段是谁加的"。
pub fn build_prompt_plan(
    workspace: &str,
    extensions: &[(String, String)],
    identity: Option<&ProductIdentity>,
    skills: &[SkillPrompt],
) -> PromptPlan {
    const SOURCE: &str = "builtin:core";
    let frag = |id: &str, order: i32, content: String| PromptFragment {
        id: id.to_string(),
        source: SOURCE.to_string(),
        order,
        content,
    };

    let mut plan = PromptPlan::new();
    plan.push(frag("persona", 0, format!("{}\n", intro_for(identity))));
    plan.push(frag(
        "workspace",
        10,
        format!("当前工作区根目录为：{workspace}\n"),
    ));
    plan.push(frag(
        "guidelines",
        20,
        "请遵循以下指引：\n\
         1. 谨慎修改代码。在修改未知文件前，先用 list_files 或 read_file 确认文件结构。\n\
         2. 对于局部小修改，优先使用 edit_file 工具以保持代码精准并生成 Unified Diff。\n\
         3. 执行终端命令时注意避免执行可能导致死循环的阻塞指令。\n"
            .to_string(),
    ));
    plan.push(frag("language", 30, language_line_for(identity)));

    if !extensions.is_empty() {
        let mut block = String::from("\n\n当前已启用的扩展工具与额外能力：\n");
        for (name, description) in extensions {
            block.push_str(&format!("- {}: {}\n", name, description));
        }
        block.push_str("当用户询问你的能力或需要相关操作时，你具备上述扩展工具所赋予的能力（如联网搜索等）。");
        plan.push(frag("extensions", 100, block));
    }

    if !skills.is_empty() {
        plan.push(frag(
            "skills_header",
            200,
            "\n\n## 当前已启用的专业技能与工作指引\n\n\
             当面临对应场景或用户请求时，请遵循以下已启用的技能规范：\n"
                .to_string(),
        ));
        // 技能**逐条**成片段（而不是拼成一大块）：这样埋点能回答"哪个技能占了多少字节"，
        // 也让将来的"技能索引 + 按需加载"有地方下手。
        for (idx, skill) in skills.iter().enumerate() {
            let mut block = format!("\n### 技能: {}\n", skill.name);
            if let Some(ref when) = skill.when_to_use {
                if !when.trim().is_empty() {
                    block.push_str(&format!("- **适用场景**: {}\n", when.trim()));
                }
            } else if !skill.description.trim().is_empty() {
                block.push_str(&format!("- **描述**: {}\n", skill.description.trim()));
            }
            block.push_str("- **指引规范**:\n");
            block.push_str(skill.content.trim());
            block.push('\n');
            // SP-45：片段来源跟着技能来源走——插件带来的技能在这张字节报表里标成
            // `plugin:<id>`，否则"这段是谁加的"在插件技能上就又答不出来了。
            plan.push(PromptFragment {
                id: format!("skill:{}", skill.name),
                source: skill.source.clone().unwrap_or_else(|| SOURCE.to_string()),
                order: 210 + idx as i32,
                content: block,
            });
        }
    }

    plan
}

use std::sync::Arc;

pub type SkillResolver = Arc<dyn Fn() -> Vec<SkillPrompt> + Send + Sync>;

/// 编码助手的人格源（`PromptSource` 端口的真实实现）。
#[derive(Clone, Default)]
pub struct CodingPromptSource {
    workspace: String,
    extensions: Vec<(String, String)>,
    identity: Option<ProductIdentity>,
    skills: Vec<SkillPrompt>,
    skill_resolver: Option<SkillResolver>,
    /// 插件声明的提示词片段：`(plugin_id, 片段 id, 正文)`（P2 · SP-44）。
    ///
    /// SP-46 起是**共享句柄**（而不是一份快照）：插件启停会真的改这份内容，而
    /// 提示词每一步都重新拼一遍——于是"关掉插件 → 下一轮提示词里就没有它了"。
    plugin_fragments: PluginFragmentStore,
}

/// 插件提示词片段的**共享活体**（SP-46）：装配期填一次，重装配时原地替换。
pub type PluginFragmentStore = std::sync::Arc<std::sync::RwLock<Vec<(String, String, String)>>>;

/// 造一个空的片段活体（`CodingPromptSource::new` 与组合根共用这一处）。
pub fn new_plugin_fragment_store() -> PluginFragmentStore {
    std::sync::Arc::new(std::sync::RwLock::new(Vec::new()))
}

impl std::fmt::Debug for CodingPromptSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodingPromptSource")
            .field("workspace", &self.workspace)
            .field("extensions", &self.extensions)
            .field("identity", &self.identity)
            .field("skills", &self.skills)
            .field("has_skill_resolver", &self.skill_resolver.is_some())
            .finish()
    }
}

impl CodingPromptSource {
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            extensions: Vec::new(),
            identity: None,
            skills: Vec::new(),
            skill_resolver: None,
            plugin_fragments: new_plugin_fragment_store(),
        }
    }

    /// 注入"已启用的扩展工具"列表（数据由调用方采集，见模块文档）。
    pub fn with_extensions(mut self, extensions: Vec<(String, String)>) -> Self {
        self.extensions = extensions;
        self
    }

    /// 注入产品身份（来自 `spec.identity`，W5-T4）。
    pub fn with_identity(mut self, identity: ProductIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// 注入静态技能列表。
    pub fn with_skills(mut self, skills: Vec<SkillPrompt>) -> Self {
        self.skills = skills;
        self
    }

    /// 注入动态技能提供器（每次取提示词时实时求值，及时反映用户启停）。
    /// 追加**插件声明的提示词片段**（SP-44）：。
    ///
    /// 排在固有段落与技能之后（order 300+），来源标 `plugin:<id>`——
    /// 于是提示词埋点能回答"这段话是谁加的"。
    pub fn with_plugin_fragments(
        mut self,
        fragments: Vec<(String, String, String)>,
    ) -> Self {
        self.plugin_fragments = std::sync::Arc::new(std::sync::RwLock::new(fragments));
        self
    }

    /// 注入**共享的**片段活体（SP-46）：重装配时原地替换内容，下一次取提示词即可见。
    pub fn with_shared_plugin_fragments(mut self, store: PluginFragmentStore) -> Self {
        self.plugin_fragments = store;
        self
    }

    pub fn with_skill_resolver(mut self, resolver: SkillResolver) -> Self {
        self.skill_resolver = Some(resolver);
        self
    }
}

impl PromptSource for CodingPromptSource {
    fn system_prompt(&self) -> String {
        self.build_plan().render()
    }

    fn prompt_segments(&self) -> Vec<agent_base::ports::PromptSegment> {
        // 走**同一个计划**（不是另算一份）：埋点报的字节数必须与真正发出去的提示词自洽
        self.build_plan()
            .byte_report()
            .into_iter()
            .map(|r| agent_base::ports::PromptSegment {
                id: r.id,
                source: r.source,
                bytes: r.bytes,
            })
            .collect()
    }
}

impl CodingPromptSource {
    /// 取当前（技能解析器每次实时求值）的提示词计划——`system_prompt` 与
    /// `prompt_segments` 共用它，避免"报的字节数与发的文本不是同一份"。
    fn build_plan(&self) -> PromptPlan {
        let skills = if let Some(ref resolver) = self.skill_resolver {
            resolver()
        } else {
            self.skills.clone()
        };
        let mut plan = build_prompt_plan(
            &self.workspace,
            &self.extensions,
            self.identity.as_ref(),
            &skills,
        );
        // SP-44：插件片段排在固有段落与技能之后；**内容为空的不追加**（空片段只会多出空行）
        //
        // SP-46：先取一份快照再拼——拼的过程中不该占着写锁（重装配那边等着它）。
        // 快照是**每次**取的，所以插件启停对下一轮提示词立即生效。
        let fragments = self
            .plugin_fragments
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        for (idx, (plugin_id, id, content)) in fragments.iter().enumerate() {
            let content = content.trim();
            if content.is_empty() {
                continue;
            }
            plan.push(PromptFragment {
                id: format!("plugin:{plugin_id}:{id}"),
                source: format!("plugin:{plugin_id}"),
                order: 300 + idx as i32,
                content: format!("

{content}"),
            });
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_prompt_contains_workspace_and_guidance() {
        let p = compose_system_prompt("/tmp/ws", &[]);
        assert!(p.contains("/tmp/ws"), "提示词必须写明工作区根目录");
        assert!(p.contains("你是 a-da"), "必须包含人格声明");
        assert!(p.contains("list_files"), "必须包含第 1 条指引");
        assert!(p.contains("edit_file"), "必须包含第 2 条指引");
        assert!(
            !p.contains("当前已启用的扩展工具与额外能力"),
            "没有扩展时不得出现扩展段落"
        );
    }

    #[test]
    fn test_extension_section_is_appended_verbatim() {
        let ext = vec![
            ("web_search".to_string(), "联网搜索".to_string()),
            ("rss".to_string(), "订阅 RSS".to_string()),
        ];
        let p = compose_system_prompt("/tmp/ws", &ext);
        assert!(p.contains("\n\n当前已启用的扩展工具与额外能力：\n"), "扩展段落标题必须一致");
        assert!(p.contains("- web_search: 联网搜索\n"), "扩展条目格式必须是 `- name: desc`");
        assert!(p.contains("- rss: 订阅 RSS\n"));
        assert!(
            p.contains("你具备上述扩展工具所赋予的能力"),
            "扩展段落结尾说明必须保留"
        );
    }

    #[test]
    fn test_port_implementation_matches_pure_function() {
        let src = CodingPromptSource::new("/tmp/ws")
            .with_extensions(vec![("a".to_string(), "b".to_string())]);
        assert_eq!(
            src.system_prompt(),
            compose_system_prompt("/tmp/ws", &[("a".to_string(), "b".to_string())]),
            "端口实现必须与纯函数同源（不许各写一份排版）"
        );
    }

    /// W5-T4：产品声明的 `identity` 必须真的进提示词。
    #[test]
    fn test_declared_identity_reaches_the_prompt() {
        let id = ProductIdentity::new("a_da 编程助手", "你是一个严谨且专业的 AI 编程助手", "zh-CN");
        let p = compose_system_prompt_with_identity("/tmp/ws", &[], Some(&id));
        assert!(
            p.contains("你是 a_da 编程助手，你是一个严谨且专业的 AI 编程助手"),
            "产品名字与人格必须出现在首行：{p}"
        );
        assert!(!p.contains("你是 a-da，"), "声明了身份就不该再用默认首行");
        assert!(p.contains("/tmp/ws"), "工作区仍在");
    }

    /// 未声明身份时必须与旧输出**逐字节一致**（既有断言不许被放宽）。
    #[test]
    fn test_absent_identity_is_byte_identical_to_legacy() {
        let legacy = compose_system_prompt("/tmp/ws", &[("a".to_string(), "b".to_string())]);
        let none = compose_system_prompt_with_identity(
            "/tmp/ws",
            &[("a".to_string(), "b".to_string())],
            None,
        );
        assert_eq!(legacy, none, "无身份声明时输出必须逐字节一致");

        // 空名字等同于未声明
        let empty = ProductIdentity::new("   ", "人格", "zh-CN");
        assert!(!empty.is_declared());
        assert_eq!(
            compose_system_prompt_with_identity("/tmp/ws", &[], Some(&empty)),
            compose_system_prompt("/tmp/ws", &[]),
            "空名字不得改变提示词"
        );
    }

    /// `locale` 必须真的有作用：非中文环境换掉语言指引，中文环境保持原样。
    #[test]
    fn test_locale_changes_the_language_guidance() {
        let zh = ProductIdentity::new("助手", "人格", "zh-CN");
        let zh_prompt = compose_system_prompt_with_identity("/tmp/ws", &[], Some(&zh));
        assert!(
            zh_prompt.contains("所有的回复都使用清晰、专业的中文表达。"),
            "中文环境保持原指引：{zh_prompt}"
        );

        let en = ProductIdentity::new("Assistant", "persona", "en-US");
        let en_prompt = compose_system_prompt_with_identity("/tmp/ws", &[], Some(&en));
        assert!(
            en_prompt.contains("en-US"),
            "非中文环境必须体现语言环境：{en_prompt}"
        );
        assert!(!en_prompt.contains("所有的回复都使用清晰、专业的中文表达。"));
    }

    /// 人格为空时要有兜底，不能出现半截句子。
    #[test]
    fn test_empty_persona_falls_back() {
        let id = ProductIdentity::new("助手", "", "zh-CN");
        let p = compose_system_prompt_with_identity("/tmp/ws", &[], Some(&id));
        assert!(p.contains("你是 助手，一个由 Rust"), "不得出现半截句子：{p}");
    }

    #[test]
    fn test_source_carries_identity() {
        let src = CodingPromptSource::new("/tmp/ws")
            .with_identity(ProductIdentity::new("产品甲", "人格甲", "zh-CN"));
        assert!(src.system_prompt().contains("产品甲"));
    }

    #[test]
    fn test_skills_reach_the_system_prompt() {
        let skill = SkillPrompt::new("vibe-coding", "极速编码规范", "# Vibe Coding 规范内容")
            .with_when_to_use("快速构建 MVP 原型时使用");
        let src = CodingPromptSource::new("/tmp/ws").with_skills(vec![skill]);
        let prompt = src.system_prompt();

        assert!(prompt.contains("## 当前已启用的专业技能与工作指引"));
        assert!(prompt.contains("### 技能: vibe-coding"));
        assert!(prompt.contains("- **适用场景**: 快速构建 MVP 原型时使用"));
        assert!(prompt.contains("# Vibe Coding 规范内容"));
    }

    #[test]
    fn test_skill_resolver_dynamically_evaluates() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let resolver = Arc::new(move || {
            if flag_clone.load(Ordering::SeqCst) {
                vec![SkillPrompt::new("dyn-skill", "动态描述", "动态内容")]
            } else {
                Vec::new()
            }
        });

        let src = CodingPromptSource::new("/tmp/ws").with_skill_resolver(resolver);
        assert!(!src.system_prompt().contains("dyn-skill"), "未启用时不应包含");

        flag.store(true, Ordering::SeqCst);
        assert!(src.system_prompt().contains("dyn-skill"), "启用后下次取提示词必须立刻包含");
    }
}
