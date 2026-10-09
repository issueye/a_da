//! 系统提示词适配器：把"人格文本怎么排版"收敛到唯一真源。
//!
//! 设计口径（`docs/agent-base-wiring-plan.md` §5 W1-T3）：
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
    let mut prompt = format!(
        "{}\n\
         当前工作区根目录为：{}\n\
         请遵循以下指引：\n\
         1. 谨慎修改代码。在修改未知文件前，先用 list_files 或 read_file 确认文件结构。\n\
         2. 对于局部小修改，优先使用 edit_file 工具以保持代码精准并生成 Unified Diff。\n\
         3. 执行终端命令时注意避免执行可能导致死循环的阻塞指令。\n\
         {}",
        intro_for(identity),
        workspace,
        language_line_for(identity)
    );

    if !extensions.is_empty() {
        prompt.push_str("\n\n当前已启用的扩展工具与额外能力：\n");
        for (name, description) in extensions {
            prompt.push_str(&format!("- {}: {}\n", name, description));
        }
        prompt.push_str("当用户询问你的能力或需要相关操作时，你具备上述扩展工具所赋予的能力（如联网搜索等）。");
    }

    prompt
}

/// 编码助手的人格源（`PromptSource` 端口的真实实现）。
#[derive(Debug, Clone, Default)]
pub struct CodingPromptSource {
    workspace: String,
    extensions: Vec<(String, String)>,
    identity: Option<ProductIdentity>,
}

impl CodingPromptSource {
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            extensions: Vec::new(),
            identity: None,
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
}

impl PromptSource for CodingPromptSource {
    fn system_prompt(&self) -> String {
        compose_system_prompt_with_identity(&self.workspace, &self.extensions, self.identity.as_ref())
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
}
