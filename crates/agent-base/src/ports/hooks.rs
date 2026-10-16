//! 钩子端口（P3a · SP-50）：**扩展点运行时**的引擎侧契约。
//!
//! # 为什么是端口
//!
//! 钩子处理器跑在插件沙箱里（`ada-ts-engine` → 进程内 Boa），而引擎在 `agent-base`：
//! 引擎**不认识沙箱、也不该认识**（INV-1 的唯一多轮循环不许长出对运行时的依赖）。
//! 所以引擎只见这个端口，实现（`agent-node::hooks::HookRuntime`）在节点层。
//!
//! # 契约要点（设计文档 §5）
//!
//! 1. **成对是硬约束**：每个点位都是 `before` / `after` 一对。引擎通过
//!    [`HookCtx::on_event`] 拿到**逐条**观测（一进一出），从而发出成对诊断——
//!    端口只报事实，**不造事件信封**（`seq` 与轮次上下文在引擎手里，伪造它们等于
//!    在"seq 严格递增"的账本里记下假违约，本仓已有先例）。
//! 2. **事前两种能力**：拦下（[`BeforeOutcome::Deny`]）与结束整轮
//!    （[`BeforeOutcome::AbortTurn`]，只有内置插件拿得到）。
//!    **P3a 刻意不实现"改写"**：`Replace` 需要审批重跑与路径重校验（§5.6），
//!    是安全敏感变更，属 P3b（SP-60/61）。在那之前插件声明 `replace` 会在
//!    **装配期被拒并产诊断**——不许"声明了却悄悄没生效"。
//! 3. **事后只有观察 + 追加**：`Note` 进的是**普通工具结果**（模型看得到），
//!    `Observe` 只进诊断面。事后不得改写已发生的事实。
//! 4. **超时视为 `Pass`**：插件是外部代码，它卡住不该让用户等。
//!
//! **端口没有默认方法体**（本仓硬约束）：默认实现会让"忘了实现"变成静默降级。

use serde::{Deserialize, Serialize};

use crate::ports::tools::BoxFuture;
use crate::ports::CancelToken;

/// 点位名（**单一真源**）：字符串常量集中在这里，别在各层手写同一串字面量。
pub mod points {
    pub const BEFORE_TOOL_CALL: &str = "beforeToolCall";
    pub const AFTER_TOOL_CALL: &str = "afterToolCall";
    pub const BEFORE_APPROVAL: &str = "beforeApproval";
    pub const AFTER_APPROVAL: &str = "afterApproval";
    pub const BEFORE_PLUGIN_TOGGLE: &str = "beforePluginToggle";
    pub const AFTER_PLUGIN_TOGGLE: &str = "afterPluginToggle";
    pub const BEFORE_THREAD_CREATE: &str = "beforeThreadCreate";
    pub const AFTER_THREAD_CREATE: &str = "afterThreadCreate";
    pub const BEFORE_TURN: &str = "beforeTurn";
    pub const AFTER_TURN: &str = "afterTurn";
    /// 待办清单被更新（P3b · SP-63）：载荷是**全量清单**。
    pub const BEFORE_TODO_UPDATE: &str = "beforeTodoUpdate";
    pub const AFTER_TODO_UPDATE: &str = "afterTodoUpdate";
    pub const BEFORE_AGENT_START: &str = "beforeAgentStart";
    pub const AFTER_AGENT_END: &str = "afterAgentEnd";
    pub const BEFORE_COMPACTION: &str = "beforeCompaction";
    pub const AFTER_COMPACTION: &str = "afterCompaction";
    pub const BEFORE_LLM_REQUEST: &str = "beforeLlmRequest";
    pub const AFTER_LLM_RESPONSE: &str = "afterLlmResponse";
    pub const BEFORE_THREAD_DELETE: &str = "beforeThreadDelete";
    pub const AFTER_THREAD_DELETE: &str = "afterThreadDelete";
    pub const BEFORE_SUBAGENT_START: &str = "beforeSubagentStart";
    pub const AFTER_SUBAGENT_END: &str = "afterSubagentEnd";
    pub const BEFORE_SYSTEM_PROMPT: &str = "beforeSystemPrompt";
    pub const AFTER_SYSTEM_PROMPT: &str = "afterSystemPrompt";
    pub const BEFORE_PERSIST: &str = "beforePersist";
    pub const AFTER_CHECKPOINT: &str = "afterCheckpoint";
    pub const BEFORE_SKILL_LOAD: &str = "beforeSkillLoad";
    pub const AFTER_SKILL_LOAD: &str = "afterSkillLoad";
    pub const BEFORE_PROVIDER_REQUEST: &str = "beforeProviderRequest";
    pub const BEFORE_PROVIDER_HEADERS: &str = "beforeProviderHeaders";
    pub const AFTER_PROVIDER_RESPONSE: &str = "afterProviderResponse";
    pub const BEFORE_CONTEXT: &str = "beforeContext";
    pub const AFTER_CONTEXT: &str = "afterContext";

    /// 事前实现范围（其余点位在装配期被拒：`point_not_implemented`）。
    pub const IMPLEMENTED: &[&str] = &[
        BEFORE_TOOL_CALL,
        AFTER_TOOL_CALL,
        BEFORE_TODO_UPDATE,
        AFTER_TODO_UPDATE,
        BEFORE_PLUGIN_TOGGLE,
        AFTER_PLUGIN_TOGGLE,
        BEFORE_THREAD_CREATE,
        AFTER_THREAD_CREATE,
        BEFORE_TURN,
        AFTER_TURN,
        BEFORE_APPROVAL,
        AFTER_APPROVAL,
        BEFORE_AGENT_START,
        AFTER_AGENT_END,
        BEFORE_COMPACTION,
        AFTER_COMPACTION,
        BEFORE_LLM_REQUEST,
        AFTER_LLM_RESPONSE,
        BEFORE_THREAD_DELETE,
        AFTER_THREAD_DELETE,
        BEFORE_SUBAGENT_START,
        AFTER_SUBAGENT_END,
        BEFORE_SYSTEM_PROMPT,
        AFTER_SYSTEM_PROMPT,
        BEFORE_PROVIDER_REQUEST,
        BEFORE_PROVIDER_HEADERS,
        AFTER_PROVIDER_RESPONSE,
        BEFORE_CONTEXT,
        AFTER_CONTEXT,
        BEFORE_PERSIST,
        AFTER_CHECKPOINT,
        BEFORE_SKILL_LOAD,
        AFTER_SKILL_LOAD,
    ];

    /// **支持改写**的点位（P3b · SP-60）。
    ///
    /// 只有载荷是"一份完整、可重新校验的数据"的点位才开放 `Replace`：
    /// 工具参数与待办清单都能重跑校验（schema / 路径 / 清单形状），
    /// 而"插件启停"这种**行为**没有可改写的载荷（它只有"允许/不允许"）。
    pub const REPLACE_SUPPORTED: &[&str] = &[BEFORE_TOOL_CALL, BEFORE_TODO_UPDATE];
}

/// 点位属于哪一级订阅面（设计 §5.9）。
///
/// 分级回答的是"**谁能拦**"：控制面的拦下权等于"谁能锁死系统"——插件若能否决自己被停用
/// （自我加冕）、或拒绝用户新建会话（拒绝服务），那这套扩展机制就是个后门。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookLevel {
    /// 行为面：第三方可以 `Replace` / `Deny`
    DataPlane,
    /// 控制面：只有内置插件能 `Replace` / `Deny`；第三方只能观察
    ControlPlane,
    /// 只读面：任何来源都只能观察 / 追加旁注
    ObserveOnly,
}

impl HookLevel {
    pub fn of(point: &str) -> Option<Self> {
        use points::*;
        match point {
            BEFORE_TOOL_CALL | AFTER_TOOL_CALL => Some(Self::DataPlane),
            BEFORE_APPROVAL | AFTER_APPROVAL => Some(Self::ControlPlane),
            BEFORE_PLUGIN_TOGGLE | AFTER_PLUGIN_TOGGLE => Some(Self::ControlPlane),
            BEFORE_THREAD_CREATE | AFTER_THREAD_CREATE => Some(Self::ControlPlane),
            BEFORE_TURN | AFTER_TURN => Some(Self::ControlPlane),
            BEFORE_AGENT_START | AFTER_AGENT_END => Some(Self::ControlPlane),
            BEFORE_COMPACTION | AFTER_COMPACTION => Some(Self::ControlPlane),
            BEFORE_LLM_REQUEST | AFTER_LLM_RESPONSE => Some(Self::ControlPlane),
            BEFORE_THREAD_DELETE | AFTER_THREAD_DELETE => Some(Self::ControlPlane),
            BEFORE_SUBAGENT_START | AFTER_SUBAGENT_END => Some(Self::ControlPlane),
            BEFORE_SYSTEM_PROMPT | AFTER_SYSTEM_PROMPT => Some(Self::ControlPlane),
            BEFORE_PERSIST | AFTER_CHECKPOINT => Some(Self::ObserveOnly),
            BEFORE_SKILL_LOAD | AFTER_SKILL_LOAD => Some(Self::ObserveOnly),
            BEFORE_PROVIDER_REQUEST | BEFORE_PROVIDER_HEADERS | AFTER_PROVIDER_RESPONSE => {
                Some(Self::ControlPlane)
            }
            BEFORE_CONTEXT | AFTER_CONTEXT => Some(Self::ControlPlane),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::DataPlane => "data-plane",
            Self::ControlPlane => "control-plane",
            Self::ObserveOnly => "observe-only",
        }
    }
}

/// 插件来源档位。**没有"受信"这一档**：本仓尚无"标记某插件为受信"的机制，
/// 变体先不加（加了就是死枚举——本项目明确要防的东西）。将来要做时，
/// 它应当由 `PluginManifest` 的一个显式字段驱动，而不是靠目录猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginTrust {
    /// 本项目自带（编译进来）
    Builtin,
    /// 第三方 / 工作区插件
    ThirdParty,
}

/// 插件在某个点位上要求的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookAction {
    /// 只观察（进诊断面）
    Observe,
    /// 往结果追一句旁注（进模型上下文，见 §5.3）
    Note,
    /// 改写载荷（**P3b 才实现**）
    Replace,
    /// 拦下这一次行为
    Deny,
    /// 结束整轮（比拦下更强）
    AbortTurn,
}

impl HookAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Note => "note",
            Self::Replace => "replace",
            Self::Deny => "deny",
            Self::AbortTurn => "abort",
        }
    }

    /// 从 manifest/SDK 的字符串解析（认不出的**返回 None**，由调用方产诊断）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "observe" => Some(Self::Observe),
            "note" => Some(Self::Note),
            "replace" => Some(Self::Replace),
            "deny" => Some(Self::Deny),
            "abort" | "abortturn" | "abort_turn" => Some(Self::AbortTurn),
            _ => None,
        }
    }
}

/// **动作级分级的唯一判据**（设计 §5.9）。
///
/// 返回 `Err(理由)` 时，装配期要用这条理由产诊断并**拒绝该声明**——
/// 静默忽略会让插件作者以为自己的拦截生效了（AGENTS §15 的那个坑）。
pub fn allows(level: HookLevel, trust: PluginTrust, action: HookAction) -> Result<(), String> {
    // 观察与旁注：任何面、任何来源都可以（旁注进的是模型上下文，但那不改任何行为）
    if matches!(action, HookAction::Observe | HookAction::Note) {
        return Ok(());
    }
    // 结束整轮：等于"拒绝服务"，只给内置
    if action == HookAction::AbortTurn {
        return match trust {
            PluginTrust::Builtin => Ok(()),
            PluginTrust::ThirdParty => {
                Err("abort_not_permitted:第三方插件不得结束整轮（拒绝服务）".to_string())
            }
        };
    }
    // 改写（P3b）：与拦下同一档权限——**行为面**人人可改（改完必被重新校验），
    // 控制面只有内置。为什么第三方也能改：`Replace` 不是授权，它只是"换个参数再走一遍
    // 同一套闸门"（schema / 路径 / 风险单调性 / 审批），最终仍由核心裁决。
    if action == HookAction::Replace {
        return match level {
            HookLevel::DataPlane => Ok(()),
            HookLevel::ControlPlane => match trust {
                PluginTrust::Builtin => Ok(()),
                PluginTrust::ThirdParty => Err(format!(
                    "control_plane_replace_not_permitted:第三方插件不得在 {} 点位上改写",
                    level.label()
                )),
            },
            HookLevel::ObserveOnly => {
                Err("observe_only_not_permitted:该点位只允许 observe / note".to_string())
            }
        };
    }
    // 拦下：行为面第三方可用；控制面只有内置可用
    match level {
        HookLevel::DataPlane => Ok(()),
        HookLevel::ControlPlane => match trust {
            PluginTrust::Builtin => Ok(()),
            PluginTrust::ThirdParty => Err(format!(
                "control_plane_deny_not_permitted:第三方插件不得在 {} 点位上拦下（那是能锁死系统的开关）",
                level.label()
            )),
        },
        HookLevel::ObserveOnly => Err("observe_only_not_permitted:该点位只允许 observe / note".to_string()),
    }
}

/// 事前调用的结果。
///
/// `Replace` 的载荷是**参数对象**（`{"args": {...}}` 里的那层），不是整个信封：
/// 工具名、读写性、描述符字段都**不属于可改写载荷**（设计 §5.6 第 2 条——
/// 改了它们就等于让插件把写工具伪装成只读工具）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeforeOutcome {
    /// 无意见，继续（默认，等价于没装插件）
    Pass,
    /// 拦下这一次行为：理由作为**普通工具结果**回模型（可纠正，不是"执行失败"）
    Deny { reason: String },
    /// 结束整轮（只有内置插件拿得到这个能力）
    AbortTurn { reason: String },
    /// **改写参数**（P3b · SP-60）。
    ///
    /// 🔴 改写**不是授权**（设计 §5.2）：核心拿到它之后要**重新校验**——
    /// schema / 路径沙箱 / 风险单调性——然后才轮到审批读改写后的 args。
    /// 任何一项不过，就退回原参数并把原因记进诊断（`hook.replace_rejected`）。
    Replace {
        args: serde_json::Value,
        /// 哪个插件改的（诊断要能归因到具体插件，否则"谁改的"答不出来）
        by: String,
    },
}

impl BeforeOutcome {
    /// 是否放行（`Pass` = 放行）。**注意**：放行不等于"插件同意了"，
    /// 它只是"没意见"——审批闸门随后照常按声明裁决（§5.6 的安全序）。
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// 事后回调能做的事（**不得改写已发生的事实**，§5.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterNote {
    /// 往工具结果**追加一句旁注**：模型看得到（这一点必须在 SDK 里写明，
    /// 否则插件作者会误以为旁注是"只给人看的"）
    Note { text: String },
    /// 只进诊断面，不进模型上下文
    Observe { data: serde_json::Value },
}

/// 一条钩子的**观测记录**：引擎据此发出成对诊断。
///
/// 为什么由端口报事实、由引擎发事件：`seq` 与轮次上下文在引擎手里；宿主伪造事件信封
/// 会让"seq 严格递增"的账本记下一堆假违约（本仓已有先例，见 `ports::scripts` 的说明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookObservation {
    /// 一次调用的**配对键**：同一对 entered / finished 用它对齐
    pub call_id: String,
    pub point: String,
    pub plugin_id: String,
    pub phase: HookPhase,
    /// 结束相位才有：这条钩子给出的结果
    pub entry_outcome: Option<HookEntryOutcome>,
    pub duration_ms: Option<u64>,
    pub reason: Option<String>,
    /// 改写**前**的内容指纹（P3b · SP-60；只在 `Replace` 时带）
    pub before_hash: Option<String>,
    /// 改写**后**的内容指纹
    pub after_hash: Option<String>,
    /// 改写前后**字节差**（正数 = 变长）
    pub bytes_delta: Option<i64>,
}

/// 一份内容的**短指纹**（8 字节 FNV-1a 十六进制）。
///
/// 为什么不是 sha8：本仓不引 sha 依赖，而这里要回答的问题只有两个——
/// "改没改"与"两次是不是同一份"。密码学强度在这里没有用武之地
/// （内容已经进了埋点所在的进程，摘要不是保密手段）。名字如实叫 `hash8`，
/// 不叫 `sha8`，免得有人把它当成校验和去用。
pub fn content_hash8(value: &serde_json::Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    Entered,
    Finished,
}

/// 单条钩子的结果（聚合结果的组成部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEntryOutcome {
    Passed,
    Denied,
    Aborted,
    TimedOut,
    /// 该插件声明了这个点位，但没有声明这个动作（只观察）
    Observed,
    /// 改写了载荷（P3b）——核心随后会**重新校验**它
    Replaced,
}

impl HookEntryOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Passed => "pass",
            Self::Denied => "deny",
            Self::Aborted => "abort",
            Self::TimedOut => "timed_out",
            Self::Observed => "observe",
            Self::Replaced => "replace",
        }
    }
}

/// 一次事前调用的请求。
#[derive(Debug, Clone)]
pub struct BeforeRequest {
    pub point: String,
    /// 相关工具/对象名（工具点位有点位名，插件启停没有）——只用于诊断与给插件看的载荷
    pub subject: Option<String>,
    /// 给插件的载荷（**args 原文**；P3a 只读）
    pub payload: serde_json::Value,
}

/// 一次事后调用的请求（带上事前的结果，供插件判断"这次到底发生了什么"）。
#[derive(Debug, Clone)]
pub struct AfterRequest {
    pub point: String,
    pub subject: Option<String>,
    pub payload: serde_json::Value,
}

/// Registration requests emitted by a hook context. These are data-only
/// requests; the host decides when and how to apply them.
#[derive(Debug, Clone, PartialEq)]
pub enum HookRegistration {
    Tool {
        plugin_id: String,
        name: String,
        description: String,
        parameters: serde_json::Value,
        execute_export: String,
    },
    SystemPrompt {
        plugin_id: String,
        id: String,
        content: String,
    },
    Skill {
        plugin_id: String,
        name: String,
        description: String,
        body: String,
        when_to_use: Option<String>,
        user_invocable: Option<bool>,
    },
}

/// 统一预算（来自产品声明的 `hookTimeoutMs`，见 SP-53）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookBudget {
    pub timeout_ms: u64,
}

impl Default for HookBudget {
    fn default() -> Self {
        // 设计 §5.5：默认 500ms + 超时即 Pass。绝不阻塞整轮。
        Self { timeout_ms: 500 }
    }
}

/// 调用上下文：归属、取消，以及**遥测出口**。
///
/// **预算不在这里**：单点位预算是宿主实现自己的配置（`hookTimeoutMs`，见 SP-53），
/// 实现持有它、也自己执行它。放进上下文就会变成"两处各配一次"——迟早不一致，
/// 而表现是"界面显示 500ms、实际等了 5 秒"这种查不出来的偏差。
pub struct HookCtx<'a> {
    pub thread_id: &'a str,
    /// 本轮 id（引擎给；插件启停那类"轮外点位"给空串）
    pub turn_id: &'a str,
    /// 当前工具调用 id（工具点位给；其它给 `None`）
    pub call_id: Option<&'a str>,
    pub cancel: &'a dyn CancelToken,
    /// **逐条**观测回调：一进一出各调一次，由**调用方**（引擎 / 宿主）转成诊断事件。
    ///
    /// 为什么不是"返回一个 Vec 让调用方事后发"：那样 `HookEntered` 会晚于钩子实际结束，
    /// 时间是假的。虽然只是诊断，但本项目对"看起来有数据其实是空的/错的"一律零容忍。
    pub on_event: &'a (dyn Fn(HookObservation) + Send + Sync),
}

/// 把一条钩子观测转成**诊断事件**（P3a · SP-52 的**唯一转换点**）。
///
/// 为什么放在端口层而不是各调用方：引擎（轮内）与宿主（轮外，管理面点位）都要发这几种
/// 事件，各写一份就会出现"引擎报的 outcome 叫 denied、宿主报的叫 deny"这种对不上的账，
/// 而账对不上的时候没人知道该信哪份。
///
/// 三种形态，各自的理由：
/// - `Entered` → `hook.entered`（配对键 `callId` 进 `data`，于是"哪一进没出"能查）；
/// - `Finished` → `hook.finished`（带 `outcome` / 耗时 / 理由）；
/// - **`plugin_id` 为空**的那些 → `hook.skipped`：那是"这个点位没人订阅"，
///   与"跑了一条钩子"是两种事实，混在一个点位上就没法机械验证成对性。
pub fn observation_to_diagnostic(obs: &HookObservation) -> crate::domain::DiagnosticEvent {
    use crate::domain::{
        DiagFamily, DiagOutcome, DiagnosticEvent, Origin, POINT_HOOK_ENTERED,
        POINT_HOOK_FINISHED, POINT_HOOK_SKIPPED,
    };

    if obs.plugin_id.is_empty() {
        return DiagnosticEvent::new(DiagFamily::Hook, POINT_HOOK_SKIPPED, Origin::Engine)
            .with_outcome(DiagOutcome::Skipped)
            .with_reason(obs.reason.clone().unwrap_or_else(|| "no_subscriber".into()))
            .with_data(serde_json::json!({ "point": obs.point }));
    }

    let origin = Origin::Plugin {
        id: obs.plugin_id.clone(),
    };
    match obs.phase {
        HookPhase::Entered => DiagnosticEvent::new(DiagFamily::Hook, POINT_HOOK_ENTERED, origin)
            .with_data(serde_json::json!({
                "point": obs.point,
                "callId": obs.call_id,
            })),
        HookPhase::Finished => {
            let outcome = match obs.entry_outcome.unwrap_or(HookEntryOutcome::Passed) {
                HookEntryOutcome::Passed
                | HookEntryOutcome::Observed
                | HookEntryOutcome::Replaced => DiagOutcome::Passed,
                HookEntryOutcome::Denied => DiagOutcome::Denied,
                HookEntryOutcome::TimedOut => DiagOutcome::TimedOut,
                HookEntryOutcome::Aborted => DiagOutcome::Aborted,
            };
            let mut ev = DiagnosticEvent::new(DiagFamily::Hook, POINT_HOOK_FINISHED, origin)
                .with_outcome(outcome)
                .with_data(serde_json::json!({
                    "point": obs.point,
                    "callId": obs.call_id,
                    "action": obs.entry_outcome.unwrap_or(HookEntryOutcome::Passed).label(),
                    // 改写必须可追溯（P3b·SP-60）：改前改后的指纹与字节差
                    "beforeHash": obs.before_hash,
                    "afterHash": obs.after_hash,
                    "bytesDelta": obs.bytes_delta,
                }));
            if let Some(ms) = obs.duration_ms {
                ev = ev.with_duration_ms(ms);
            }
            if let Some(r) = &obs.reason {
                ev = ev.with_reason(r.clone());
            }
            ev
        }
    }
}

/// **工具内部点位**的钩子入口（P3b · SP-63）：交给工具的上下文里带这一份。
///
/// # 为什么工具需要它，而不是由引擎代跑
///
/// `beforeTodoUpdate` 的载荷是"待办清单的**语义**"——它由工具自己解释（哪个字段是标题、
/// 什么算一次更新）。引擎代跑就只能靠"工具名叫 `todo`"这类硬编码，那是最脆的一种耦合。
/// 所以：引擎把**运行时 + 归属 + 遥测出口**交给工具，由工具在正确的时机调用自己的点位。
///
/// # 为什么遥测出口也给它
///
/// 成对埋点（`hook.entered` / `hook.finished`）是硬约束，而这些点位不在引擎的调用栈上。
/// 出口给的是 `HookObservation`（**不是事件信封**）：`seq` 与轮次上下文仍由引擎套上，
/// 所以"事件流单一真源"没有被破坏——工具只报事实。
pub struct ToolHooks<'a> {
    pub runner: &'a dyn HookRunner,
    /// 本轮 id（引擎给；归因要用）
    pub turn_id: &'a str,
    /// 当前工具调用 id（归因要用）
    pub call_id: Option<&'a str>,
    /// 逐条观测出口（引擎给，见上）。
    pub on_event: &'a (dyn Fn(HookObservation) + Send + Sync),
}

impl ToolHooks<'_> {
    /// 组一个给端口用的上下文（工具自己不方便拼 `HookCtx` 的三个借用字段）。
    pub fn ctx<'a>(
        &'a self,
        thread_id: &'a str,
        cancel: &'a dyn CancelToken,
        on_event: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> HookCtx<'a> {
        HookCtx {
            thread_id,
            turn_id: self.turn_id,
            call_id: self.call_id,
            cancel,
            on_event,
        }
    }
}

/// 钩子运行时：按点位把注册的钩子跑一遍。
///
/// 顺序（设计 §5.5）：**内置优先 → `order` 升序 → 插件 id 字典序兜底**；
/// 任一钩子 `Deny` / `AbortTurn` 即**定稿**（后续钩子不再被问）；
/// 单点位超时视为 `Pass`。
pub trait HookRunner: Send + Sync {
    /// Drain registration requests emitted by hook handlers.
    fn take_registrations(&self) -> Vec<HookRegistration> {
        Vec::new()
    }

    /// 事前：`Pass` / `Deny` / `AbortTurn`。
    ///
    /// 没有任何订阅者时**必须**通过 `ctx.on_event` 报一条 `reason = "no_subscriber"` 的
    /// 记录后返回 `Pass`——"没人管"与"没装过"在面板上必须分得出来（AGENTS §2.2 缺席可见）。
    fn before<'a>(
        &'a self,
        req: BeforeRequest,
        ctx: &'a HookCtx<'a>,
    ) -> BoxFuture<'a, BeforeOutcome>;

    /// 事后：返回旁注（可能多条，按注册顺序）。
    fn after<'a>(
        &'a self,
        req: AfterRequest,
        ctx: &'a HookCtx<'a>,
    ) -> BoxFuture<'a, Vec<AfterNote>>;
}
