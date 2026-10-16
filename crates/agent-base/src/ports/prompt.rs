//! 系统提示词端口：为人格包与提示词生成提供来源。
//!
//! 现状：`build_system_prompt(&ws_str)` 将工作区路径与硬编码人格写死在代码里；
//! 端口化之后，不同产品（编码、生活类、通用助手）只需注入各自的人格源。

/// 提示词的一段（**给埋点与诊断用**：谁加的、占多少字节）。
///
/// 定义在领域层（而不是适配器）的理由：引擎要发 `prompt.composed` 遥测，而引擎只认识
/// `agent-base`。`source` 是**标签**（`builtin:core` / `plugin:<id>`），与贡献来源同一套写法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSegment {
    pub id: String,
    pub source: String,
    pub bytes: usize,
}

pub trait PromptSource: Send + Sync {
    /// 获取当前人格/模式对应的系统提示词。
    fn system_prompt(&self) -> String;

    /// **逐段**报告提示词的构成（**空 = 这一份不细分**，例如测试替身与子智能体 profile）。
    ///
    /// 为什么要它：`prompt.composed` 遥测要回答"这段提示词是谁加的、占多少字节"。
    /// 只有总长度时，"这轮多了 200 字节"是没法排查的——不知道是技能正文还是某个插件加的。
    ///
    /// 契约（`agent-conformance` 会断言）：非空时，各段字节数之和必须等于
    /// `system_prompt().len()`——**不自洽的拆分比不拆更坏**：它会让排查者去追一个不存在的差异。
    fn prompt_segments(&self) -> Vec<PromptSegment>;
}
