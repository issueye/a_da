//! 诊断事件族（P0 埋点地基）：面向机器与诊断面板的**扩展点遥测**。
//!
//! 与面向会话流的卡片事件（`ToolCallStarted` / `ApprovalRequested` …）刻意分开：
//! 诊断**只给人看，永不进模型上下文、不进会话 items 的正文**——沿用 `Item::Notice`
//! 已经立好的口径（`fold_messages` 明确跳过它）。
//!
//! # 为什么是"一个变体 + 结构化载荷"，而不是二十多个变体
//!
//! 扩展点的点位会随分期增长（P0 三个 → P1 脚本 → P3 钩子 → …）。如果每个点位都是
//! `AgentEventBody` 的一个变体，那么每次加点位都要在每个消费者里补一条分支
//! （`project_to_loop_event`、dispatch 的 UI 投影、`kind()`、契约测试的穷尽匹配……），
//! 加一个点位要动五个文件。所以这里用**一个** `Diagnostic(DiagnosticEvent)` 变体承载全部
//! 遥测，点位名放进 `point` 字段——`repo` 的既有设施（check G：每个变体必须有生产发射者）
//! 仍然管得住"声明了没人发"。
//!
//! # 点位词汇表随**实现**增长，不许先声明后实现
//!
//! [`DIAG_POINTS`] 只登记**当天真的有发射方**的点位。这是把"未实现的能力位不得声明为 true"
//! （`docs/plugin-sdk/v1.md` §6）同一条规矩用在埋点词汇表上：先有发射方，再登记点位；
//! `cargo xtask verify-wiring` 的 check K 会盯着"登记了的点位必须有生产引用"。

use crate::domain::trace::Origin;

/// 诊断族：按"哪个子系统"分组，供诊断面板过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagFamily {
    /// 插件生命周期（发现/加载/启停/释放）
    Plugin,
    /// 脚本调用（沙箱进出、耗时、预算）
    Script,
    /// 钩子点位（进出与结果）
    Hook,
    /// 提示词组装
    Prompt,
    /// 技能加载
    Skill,
    /// 决策与门禁
    Decision,
    /// 审批等待
    Approval,
    /// 轮次级（中止、预算）
    Turn,
}

impl DiagFamily {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Script => "script",
            Self::Hook => "hook",
            Self::Prompt => "prompt",
            Self::Skill => "skill",
            Self::Decision => "decision",
            Self::Approval => "approval",
            Self::Turn => "turn",
        }
    }
}

/// 点位的**结果**。成对约束靠它判定：每个 `entered` 必须恰好对应一个终结结果
/// （`passed` / `replaced` / `denied` / `aborted` / `timed_out`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagOutcome {
    /// 无意见/正常通过（扩展点缺席时的默认语义）
    Passed,
    /// 改写了内容（事前点位）
    Replaced,
    /// 拦下（事前点位）
    Denied,
    /// 结束整轮（事前点位，比 Denied 强）
    Aborted,
    /// 超预算/超时（一律视为"无意见"，绝不阻塞整轮）
    TimedOut,
    /// 缺席：没有插件/未声明该点位（**必须发出来**，否则回到"静默失效"）
    Skipped,
    /// 出错（加载失败、脚本异常）
    Failed,
    /// 变更已生效（启停类点位）
    Applied,
}

impl DiagOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Replaced => "replaced",
            Self::Denied => "denied",
            Self::Aborted => "aborted",
            Self::TimedOut => "timed_out",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Applied => "applied",
        }
    }
}

/// 一条诊断遥测。
///
/// **默认只记元数据**：名称、来源、耗时、结果、字节数。内容（提示词正文、工具参数、
/// 脚本 stdout）默认**不记**；确需内容时由专门的开关 + 截断 + 脱敏负责，不放在这里。
#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticEvent {
    pub family: DiagFamily,
    /// 点位名，取值来自 [`DIAG_POINTS`]（单一真源）
    pub point: String,
    /// 谁产生的这件事（由产生方填写）
    pub origin: Origin,
    /// 结果；观察类点位可以没有"结果"这一维
    pub outcome: Option<DiagOutcome>,
    pub duration_ms: Option<u64>,
    /// 缺席/失败/拦下的理由（给人看的短句）
    pub reason: Option<String>,
    /// 与本次遥测相关的字节数（提示词长度、技能正文长度、stdout 长度…）
    pub bytes: Option<u64>,
    /// 附加结构化细节（分段来源、变更差异…）。默认空对象。
    pub data: serde_json::Value,
}

impl DiagnosticEvent {
    pub fn new(family: DiagFamily, point: impl Into<String>, origin: Origin) -> Self {
        Self {
            family,
            point: point.into(),
            origin,
            outcome: None,
            duration_ms: None,
            reason: None,
            bytes: None,
            data: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn with_outcome(mut self, outcome: DiagOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    pub fn with_duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_bytes(mut self, bytes: u64) -> Self {
        self.bytes = Some(bytes);
        self
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }
}

// ── 点位单一真源 ─────────────────────────────────────────────────────────────
//
// 命名口径：`<族>.<动作>`；成对的点位用 `entered` / 终结结果区分（见 DiagOutcome）。

/// 提示词组装完成（每轮一次）。P2 起 `data.segments` 会带上各片段的来源与字节数。
pub const POINT_PROMPT_COMPOSED: &str = "prompt.composed";
/// 审批等待：等到回答或超时（回答的**内容**是人的决定，不在这里，见 §2.2 只记元数据）。
pub const POINT_APPROVAL_WAITED: &str = "approval.waited";
/// 轮次被中止（取消/预算/上下文撞顶之外的显式中止）。
pub const POINT_TURN_ABORTED: &str = "turn.aborted";
/// **没问就放行**了一次本可能需要审批的调用（P1b · SP-36 的审计留痕）。
///
/// 判据是"声明侧要求审批（非 `Never`）+ 最终没问"——那正是被**档位**放行的那一类
/// （`auto`/`full`/免问白名单）。刻意不记 `Never` 的工具：`read_file` 这种本来就免问的
/// 每次都记，只会把 60 条环形缓冲刷满，真正的审计信息反而被淹掉。
///
/// ⚠️ 记的是"**没问**"这件事与声明侧策略，不记档位——按 AGENTS §14，引擎不解释档位
/// （策略归端口）；档位本身界面已经在选择器上显示着。
pub const POINT_APPROVAL_SKIPPED: &str = "approval.skipped";
/// 一次脚本调用开始（P1 · SP-11：端口把事件出口带进来了，就得真的用上）。
pub const POINT_SCRIPT_INVOKED: &str = "script.invoked";
/// 一次脚本调用结束（带耗时与结果——`outcome` 用 `passed` / `failed` / `timed_out`）。
pub const POINT_SCRIPT_FINISHED: &str = "script.finished";
/// 插件工具**顶掉了同名的内置工具**（P1 · SP-16）。
///
/// 这条必须报：决策 1 的口径下插件只能重实现**已注册**的工具名，于是"同名"是常态而非异常，
/// 而用户必须知道自己的 `read_file` 已经不是 Rust 里那个了——静默替换等于把信任的工具悄悄换掉。
pub const POINT_PLUGIN_REPLACED_BUILTIN: &str = "plugin.replaced_builtin";
/// 插件被扫到（按插件 id 归因）。`outcome = skipped` 且 `reason = no_plugin` 表示
/// **一个都没有**——缺席也要发，否则"没扫到"与"没扫过"在面板上分不出来。
pub const POINT_PLUGIN_DISCOVERED: &str = "plugin.discovered";
/// 插件被跳过（`disabled`；装配期另有按贡献逐条给出的原因，如 `tool_not_registered`）。
///
/// ⚠️ 早先这里还有一条 `execution_not_wired`：那是 P0 的实话（执行面还没接），
/// P1 接上之后它就是对正常插件的**假警报**，已删除。
pub const POINT_PLUGIN_SKIPPED: &str = "plugin.skipped";
/// 技能扫描结果（聚合一条，避免每步扫盘把 60 条环形缓冲刷爆）。
pub const POINT_SKILL_LOADED: &str = "skill.loaded";
/// 没有**自定义**技能（`no_custom_skill`）。注意内置技能总是存在，
/// 所以缺席信号针对的是用户自己放进去的那部分——那才是会让人疑惑"怎么没生效"的。
pub const POINT_SKILL_SKIPPED: &str = "skill.skipped";
/// 技能文件的 frontmatter 里有**认不出的键**（P2 · SP-45）。
///
/// 以前这些键被**静默忽略**（`manager.rs` 的 `_ => {}`），于是作者把 `whenToUse` 拼成
/// `when_to_use_typo`、技能就"看起来装上了却不生效"——正是 AGENTS §15 的那个坑。
/// `data.keys` 带上认不出的键名清单，让人看一眼就知道该改哪一处。
pub const POINT_SKILL_INVALID: &str = "skill.invalid";
/// 一条钩子被调用（P3a · SP-52）。
///
/// **成对是硬约束**：每条 `hook.entered` 必须有且只有一个终结记录
/// （`hook.finished` 或 `hook.skipped`）。契约测试直接数这两个计数。
pub const POINT_HOOK_ENTERED: &str = "hook.entered";
/// 一条钩子跑完（带 `outcome`：`passed` / `denied` / `timed_out` / `aborted` 与耗时）。
pub const POINT_HOOK_FINISHED: &str = "hook.finished";
/// **没有任何插件订阅这个点位**（P3a · SP-52 的缺席可见）。
///
/// 这条必须发：面板上"没人管这个点位"与"这个点位压根没被问过"长得一模一样，
/// 而作者最常问的正是"我的钩子为什么没被调用"。
pub const POINT_HOOK_SKIPPED: &str = "hook.skipped";
/// 一次参数改写**真的生效了**（P3b · SP-60）。
///
/// 带改前/改后的内容指纹与字节差（`hook.finished` 里也有，这里再单独记一条是因为
/// 改写是安全敏感动作：面板上要能一眼筛出"这次调用被谁改过"）。
pub const POINT_HOOK_REPLACE_APPLIED: &str = "hook.replace";
/// 一次参数改写被**重新校验拒了**（P3b · SP-61）。
///
/// 拒绝的理由（形状 / schema / 路径越界 / 风险降级）必须留下：
/// 这是"插件想做但没做成"的记录，插件作者与排查者都靠它。
pub const POINT_HOOK_REPLACE_REJECTED: &str = "hook.replace_rejected";

/// 已实现的点位词汇表（**先有发射方再登记**，不许先声明后实现）。
///
/// 这张表与上面的 `POINT_*` 常量是**互相约束**的：`verify-wiring` 的 check K 会断言
/// 两者一一对应、且每个常量在生产代码里都有引用（= 真有发射方）。所以这里不重复一份
/// 字符串清单——那是第三份名单的开始。
pub const DIAG_POINTS: &[&str] = &[
    POINT_PROMPT_COMPOSED,
    POINT_APPROVAL_WAITED,
    POINT_APPROVAL_SKIPPED,
    POINT_TURN_ABORTED,
    POINT_SCRIPT_INVOKED,
    POINT_SCRIPT_FINISHED,
    POINT_PLUGIN_REPLACED_BUILTIN,
    POINT_PLUGIN_DISCOVERED,
    POINT_PLUGIN_SKIPPED,
    POINT_SKILL_LOADED,
    POINT_SKILL_SKIPPED,
    POINT_SKILL_INVALID,
    POINT_HOOK_ENTERED,
    POINT_HOOK_FINISHED,
    POINT_HOOK_SKIPPED,
    POINT_HOOK_REPLACE_APPLIED,
    POINT_HOOK_REPLACE_REJECTED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_points_are_unique_and_well_formed() {
        let mut sorted = DIAG_POINTS.to_vec();
        sorted.sort_unstable();
        let mut unique = sorted.clone();
        unique.dedup();
        assert_eq!(unique.len(), DIAG_POINTS.len(), "点位名不得重复：{sorted:?}");
        for p in DIAG_POINTS {
            assert!(!p.trim().is_empty(), "点位名不得为空");
            let (family, action) = p.split_once('.').unwrap_or_else(|| {
                panic!("点位名必须是 `<族>.<动作>` 形式，实际 {p}")
            });
            assert!(!family.is_empty() && !action.is_empty(), "点位名两段都不得为空：{p}");
            assert!(
                p.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "点位名只允许小写、点与下划线：{p}"
            );
        }
    }

    #[test]
    fn test_family_and_outcome_labels_are_stable() {
        assert_eq!(DiagFamily::Plugin.label(), "plugin");
        assert_eq!(DiagFamily::Turn.label(), "turn");
        assert_eq!(DiagOutcome::TimedOut.label(), "timed_out");
        assert_eq!(DiagOutcome::Skipped.label(), "skipped");
    }

    #[test]
    fn test_builder_defaults_are_metadata_only() {
        let d = DiagnosticEvent::new(DiagFamily::Prompt, POINT_PROMPT_COMPOSED, Origin::Engine);
        assert!(d.outcome.is_none());
        assert!(d.duration_ms.is_none());
        assert!(d.reason.is_none());
        assert!(d.bytes.is_none());
        assert_eq!(d.data, serde_json::json!({}), "默认不携带任何内容字段");
    }
}
