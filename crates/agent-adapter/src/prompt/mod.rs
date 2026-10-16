//! 系统提示词适配器（`PromptSource` 端口的真实实现）。
//!
//! 每个产品可以注入自己的人格源；当前落地的是编码助手形态
//! [`CodingPromptSource`]（`docs/agent-base-wiring-plan.md` §5 W1-T3）。
//!
//! [`plan`] 是 P2·SP-41 加的**片段化**：提示词不再是一串 `push_str`，而是有序片段
//! （每段有 id / 来源 / 字节数）；`compose_system_prompt_full` 就是它的 `render()`。

pub mod coding;
pub mod plan;

pub use coding::{
    build_prompt_plan, compose_system_prompt, compose_system_prompt_full,
    compose_system_prompt_with_identity, new_plugin_fragment_store, CodingPromptSource,
    PluginFragmentStore, ProductIdentity, SkillPrompt, SkillResolver,
};
pub use plan::{PromptFragment, PromptPlan, PromptSegmentReport};
