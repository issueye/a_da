//! 脚本运行时端口（P1 · SP-10）。
//!
//! # 为什么要有这个端口
//!
//! 引擎（`agent-base`）**不认识 ts-engine，也不该认识**：INV-1 要求唯一多轮循环在基座里，
//! 而脚本运行时是"外部世界"的东西（进程内 Boa、将来的子进程、更远的 WASM）。所以它只能以
//! **端口**的形式被引擎认识，实现在 `agent-node`（[`crate::ports::Tool`] 之外的第二个
//! "引擎调外部能力"的口子）。
//!
//! # 三条硬约束（见 `docs/script-plugin-system-design.md` §3.2）
//!
//! 1. **预算归端口**：`timeout` / 指令上限 / 输出上限由调用方给，宿主执行，**只有一处判定超时**。
//!    现状是三套互不相干的超时（沙箱 `inspect` 硬编码 15s、`call_tool` 由调用方给、
//!    引擎另有 `RunPolicy::tool_timeout`），三者对不上就会出"界面显示还在跑、其实早放弃了"。
//! 2. **取消必须贯通**：脚本宿主拿得到引擎的 `CancelToken`，而不是自己造一个
//!    （沙箱内那个自造 `AbortSignal` 与引擎取消没有任何关系）。
//! 3. **埋点要过同一个出口**：宿主把脚本生命周期写成诊断遥测（宿主级的出口，
//!    由实现在构造时接），由拿得到事件信封的人统一套上信封（P0 的事件流单一真源仍成立）。
//!
//! **端口没有默认方法体**（本仓硬约束）：默认实现会让"忘了实现"变成静默降级。

use std::path::PathBuf;

use crate::ports::tools::BoxFuture;
use crate::ports::CancelToken;

/// 一次脚本调用的预算。
///
/// `loop_iterations = 0` 表示不限——**默认不该用它**：同步死循环期间取消与墙钟超时都排不上队，
/// 只有进程内的指令上限能让同步段返回（见 `ada-ts-engine` 的 `Limits`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptBudget {
    /// 墙钟上限（毫秒）。宿主用它兜住"等待型"的超时。
    pub timeout_ms: u64,
    /// 进程内循环迭代上限（0 = 不限）。
    pub loop_iterations: u64,
    /// 单次调用输出的字节上限（超出由宿主截断并如实标注）。
    pub max_output_bytes: u64,
}

impl Default for ScriptBudget {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            // 与 `ada-ts-engine::Limits::default()` 同量级：宽但有限
            loop_iterations: 10_000_000,
            max_output_bytes: 64 * 1024,
        }
    }
}

/// 脚本来源：路径（宿主读盘）或已读到手的源码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSource {
    Path(PathBuf),
    Inline { filename: Option<String>, source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    pub source: ScriptSource,
    /// 工作区根（脚本的路径沙箱以它为准）。
    pub workspace: Option<PathBuf>,
}

/// 一次调用的上下文：预算 + 取消 + 归属。
///
/// `origin_id` 是**来源标注**（插件 id / `builtin:...`），埋点与诊断按它归因。
///
/// **遥测的出口不在这里**：它是**宿主级**的事实（谁在跑脚本），由 [`ScriptHost`] 的实现
/// 在构造时接。放在每次调用的上下文里只会逼每个调用方去凑一个出口——而它们大多凑不出来：
/// 事件信封里的 `seq` 与轮次上下文都在引擎手里（本仓已有工具为发一条事件写 `seq = 0`
/// 的先例，那会让"seq 严格递增"的账本记下假违约）。
pub struct ScriptCtx<'a> {
    pub budget: ScriptBudget,
    pub cancel: &'a dyn CancelToken,
    pub thread_id: &'a str,
    pub origin_id: Option<&'a str>,
}

/// 插件声明的一段**提示词片段**（P2 · SP-44）：插件给模型"加话"的唯一入口。
///
/// 装配期它进 `PromptPlan`、来源标成 `plugin:<id>`，于是提示词的埋点能回答
/// "这段是谁加的"，而不是只看到一个总数莫名膨胀。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPromptFragment {
    pub id: String,
    pub content: String,
}

/// 插件被授予的**能力**（P4 · SP-71）。
///
/// 端口层的抽象：`agent-base` 不认识 ts-engine 的 `Needs`，只表达"这个插件要不要
/// 文件系统 / 网络"。映射到具体运行时是**实现**的事（`TsScriptHost` 一处）。
///
/// **未声明即不注入**：声明面是插件源码里的指令注释（`// @ada-needs fs,net`），
/// 它在**执行之前**就能静态读到——这一点是刻意的：如果能力要等插件跑起来才知道，
/// 那就得先给它能力再收回，而"注入后再拦"正是绕过的温床（见设计 §3.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptNeeds {
    /// 工作区文件读写
    pub fs: bool,
    /// 网络（`fetch` 与原生桥）
    pub net: bool,
}

impl ScriptNeeds {
    /// 什么都不给。
    pub fn none() -> Self {
        Self {
            fs: false,
            net: false,
        }
    }

    /// **宿主地板**（没有声明时的默认）：`fs` 开、`net` 关。
    ///
    /// 为什么地板不是"全关"：在这个产品里，读写工作区**就是**插件存在的意义
    /// （它是个编码助手），把它默认关掉只会逼每个插件都写声明，而声明一旦变成仪式，
    /// 就没人认真读了。真正需要显式声明的是**网络**——那是插件唯一能把数据送出去的口子。
    pub fn plugin_floor() -> Self {
        Self {
            fs: true,
            net: false,
        }
    }
}

/// 已装载的模块句柄（对引擎不透明；实现自己解释它的 `id`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHandle {
    pub id: String,
}

/// 插件声明的一个**技能**（P2 · SP-45）。
///
/// 与 [`ScriptPromptFragment`] 是两件事：片段是"无条件加到提示词里的一段话"，
/// 技能是"按需取用的一份规范"——它要能被列出来、能被停用、能报出正文占多少字节。
/// 端口这一层只搬运，不解释（`scope` / 归因由装配层贴）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSkillDeclaration {
    pub name: String,
    pub description: String,
    /// 正文（规范本体）。空正文的技能由装配层丢弃——它进提示词只会白占字节。
    pub body: String,
    pub when_to_use: Option<String>,
    pub user_invocable: Option<bool>,
}

/// 插件声明的一条**钩子注册**（P3a · SP-50）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptHookDeclaration {
    pub point: String,
    /// 声明的动作（`observe` / `note` / `replace` / `deny` / `abort`）。
    ///
    /// **声明即授权**：运行期只认这里列出的动作，插件返回未声明的动作一律忽略并记理由
    /// （见 `HookRuntime`）。分级（谁能拦什么）由装配层判，端口不管。
    pub actions: Vec<String>,
    pub order: i32,
}

/// 一次钩子调用的**原始结果**（端口只搬运，语义解释留给调用方）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptHookOutcome {
    /// 插件要求的动作；`None` / 空串 = 无意见（等价于 `Pass`）
    pub action: Option<String>,
    /// 拦下 / 结束整轮的理由
    pub reason: Option<String>,
    /// 旁注（`note` 动作的正文）
    pub note: Option<String>,
    /// 只进诊断面的结构化数据
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvokeRequest {
    /// 入口名（插件 SDK 里就是工具名）。
    pub export: String,
    pub args: serde_json::Value,
}

/// 脚本失败。**分类是有意做细的**：调用方要能把"预算超限/超时"映射成 `Timeout` 回执
/// （让模型换路子），把"脚本自己抛错"映射成普通错误，两者对模型的含义完全不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// 读不到 / 转译失败 / 环境不满足——**装载期**就该发现的问题
    Load { message: String },
    /// 入口不存在（插件里没有这个工具）
    UnknownExport { export: String },
    /// 撞上进程内预算（循环/递归/栈）——**可纠正**
    BudgetExceeded { kind: String },
    /// 墙钟超时——**可纠正**
    TimedOut { limit_ms: u64 },
    /// 被取消
    Aborted,
    /// 脚本自己抛的错
    Script { message: String },
    /// 运行时不可用（已释放 / 宿主内部故障）
    Unavailable { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptOutcome {
    pub ok: bool,
    /// 脚本回给调用方的文本（已按预算截断）
    pub output: String,
    /// 结构化附加数据（插件 SDK 的 `data` / `details`）
    pub data: Option<serde_json::Value>,
    pub duration_ms: u64,
    /// 输出是否因为超限被截断（**不许静默截断**）
    pub truncated: bool,
}

/// 脚本宿主：装载、调用、释放。
///
/// 实例策略（每调用新建 vs 每会话常驻）**不由端口规定**——那是宿主实现与产品预算的事
/// （见设计文档 §3.3 与决策 2）；端口只要求"句柄可释放"。
pub trait ScriptHost: Send + Sync {
    fn load<'a>(
        &'a self,
        req: &'a LoadRequest,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ModuleHandle, ScriptError>>;

    /// 这个模块提供哪些入口（插件 SDK 里就是工具名）。
    ///
    /// 为什么端口要有它：调用方**本来就得知道** export 名才能 `invoke`；而装配期
    /// （把插件工具装进 catalog）需要的正是这份清单。没有这个方法，装配层就只能退回去
    /// 文本扫描源码——那正是这套设计要替掉的东西。
    fn exports(&self, handle: &ModuleHandle) -> Result<Vec<String>, ScriptError>;

    /// 这个模块声明的**提示词片段**（没有就返回空——空不是错误）。
    ///
    /// 装配期把它们并进 `PromptPlan`，来源标成 `plugin:<id>`：于是提示词的埋点能回答
    /// "这段是谁加的"，而不是只看到一个总数膨胀。
    fn prompt_fragments(&self, handle: &ModuleHandle) -> Result<Vec<ScriptPromptFragment>, ScriptError>;

    /// 这个模块声明的**技能**（没有就返回空——空不是错误）。
    ///
    /// 与 [`Self::exports`] 同理：清单必须在**装载期**探明并挂在句柄上。装配层据此把
    /// 插件技能并进技能扫描结果（第三个来源），而不是让技能模块自己去执行 JS。
    fn skills(&self, handle: &ModuleHandle) -> Result<Vec<ScriptSkillDeclaration>, ScriptError>;

    /// 这个模块声明的**钩子注册**（没有就返回空——空不是错误）。
    ///
    /// 同样是装载期探明：装配层要用它做**分级校验**（谁能在哪个点位上拦下），
    /// 而分级校验必须在装配期完成（运行期才拒 = 插件作者以为自己的拦截生效了）。
    fn hooks(&self, handle: &ModuleHandle) -> Result<Vec<ScriptHookDeclaration>, ScriptError>;

    /// 调用这个模块在 `point` 上注册的钩子（P3a · SP-50）。
    ///
    /// 与 [`Self::invoke`] 的区别：`invoke` 调的是**工具**（按 export 名），这里调的是
    /// **点位处理器**（按点位名），入参是载荷、出参是动作。分开两个方法而不是加一个枚举参数：
    /// 两者的失败语义与预算口径不同（工具失败是"这个工具坏了"，钩子失败是"这条意见作废"）。
    fn invoke_hook<'a>(
        &'a self,
        handle: &'a ModuleHandle,
        point: &'a str,
        payload: serde_json::Value,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ScriptHookOutcome, ScriptError>>;

    fn invoke<'a>(
        &'a self,
        handle: &'a ModuleHandle,
        call: &'a InvokeRequest,
        ctx: &'a ScriptCtx<'a>,
    ) -> BoxFuture<'a, Result<ScriptOutcome, ScriptError>>;

    /// 释放（**幂等**：重复释放不报错——调用方不该为了"放两次"写额外判断）。
    fn dispose<'a>(&'a self, handle: ModuleHandle) -> BoxFuture<'a, ()>;
}
