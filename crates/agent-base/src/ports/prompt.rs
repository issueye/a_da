//! 系统提示词端口：为人格包与提示词生成提供来源。
//!
//! 现状：`build_system_prompt(&ws_str)` 将工作区路径与硬编码人格写死在代码里；
//! 端口化之后，不同产品（编码、生活类、通用助手）只需注入各自的人格源。

pub trait PromptSource: Send + Sync {
    /// 获取当前人格/模式对应的系统提示词。
    fn system_prompt(&self) -> String;
}
