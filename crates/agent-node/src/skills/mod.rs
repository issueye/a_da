pub mod builtins;
pub mod manager;
pub mod types;

pub use builtins::{get_builtin_skills, BUILTIN_SKILLS};
pub use manager::SkillManager;
pub use types::SkillSummary;

/// 插件技能的**共享活体**（P2·SP-45 起；SP-46 的启停重装配原地换它）。
///
/// 定义在**这一层**而不是 `agent-host`：`agent-rpc` 的 `skill.list` 要读同一份，
/// 而 rpc 不认识 host（依赖方向是 host → rpc）。类型放这儿，两边拿得到同一个句柄。
///
/// 🔴 **必须只有一份**：早先 rpc 侧拿的是启动时的**快照**、host 侧拿的是活体，
/// 于是"停用插件"之后**引擎的提示词里已经没有那条技能，界面列表里还在**——
/// 实机验收（`tools/verify-plugin-platform-e2e.mjs`）就是这么抓到的。
pub type SkillStore = std::sync::Arc<std::sync::RwLock<Vec<SkillSummary>>>;

/// 造一个技能活体（装配期与重装配共用这一处）。
pub fn new_skill_store(skills: Vec<SkillSummary>) -> SkillStore {
    std::sync::Arc::new(std::sync::RwLock::new(skills))
}
