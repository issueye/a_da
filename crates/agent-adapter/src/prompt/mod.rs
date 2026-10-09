//! 系统提示词适配器（`PromptSource` 端口的真实实现）。
//!
//! 每个产品可以注入自己的人格源；当前落地的是编码助手形态
//! [`CodingPromptSource`]（`docs/agent-base-wiring-plan.md` §5 W1-T3）。

pub mod coding;

pub use coding::{
    compose_system_prompt, compose_system_prompt_with_identity, CodingPromptSource,
    ProductIdentity,
};
