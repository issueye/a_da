//! 钩子运行时（P3a · SP-50）：按点位把注册的钩子跑一遍。
//!
//! # 这一层负责什么
//!
//! 1. **装配期分级校验**（SP-54）：谁能在这个点位上拦下，由 [`agent_base::ports::hooks::allows`]
//!    一处判；越级/未实现的声明在这里被**拒并留下理由**（`HookRejection`），而不是运行期静默忽略。
//! 2. **运行期执行**（SP-50/52/53）：内置优先 → `order` 升序 → 插件 id 兜底；链式；拦下即定稿；
//!    超时即 `Pass`。
//! 3. **逐条观测**：一进一出通过 `HookCtx::on_event` 报给调用方（引擎 / 宿主），由它转成诊断
//!    ——这样事件信封（`seq`、轮次上下文）只在引擎手里，不会被伪造。
//!
//! # 三条失败安全口径（与设计 §5.5 一致）
//!
//! - **超时 = 无意见**：插件是外部代码，它卡住不该让用户等（默认 500ms，来自声明）。
//! - **脚本抛错 = 无意见**：钩子自己能坏，但它没有资格因此拦住工具（拦住才是"有意见"）。
//! - **没订阅 = 如实报 `no_subscriber`**：面板上"没人管"与"没装过"必须分得出来。

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_base::ports::hooks::{
    allows, points, AfterNote, AfterRequest, BeforeOutcome, BeforeRequest, HookAction, HookBudget,
    HookCtx, HookEntryOutcome, HookLevel, HookObservation, HookPhase, HookRegistration,
    HookRunner, PluginTrust,
};
use agent_base::ports::{
    BoxFuture, ModuleHandle, ScriptBudget, ScriptCtx, ScriptError, ScriptHost,
};

use crate::plugins::PluginHookDeclaration;

/// 一条被接受并已排好序的钩子注册。
#[derive(Debug, Clone)]
pub struct RegisteredHook {
    pub plugin_id: String,
    pub trust: PluginTrust,
    pub point: String,
    pub actions: Vec<HookAction>,
    pub order: i32,
    handle: ModuleHandle,
}

impl RegisteredHook {
    /// 声明了某个动作吗（**声明即授权**：没声明的一律不生效）。
    pub fn declares(&self, action: HookAction) -> bool {
        self.actions.contains(&action)
    }
}

/// 一条被拒的声明（装配期发现，交给调用方产诊断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRejection {
    pub plugin_id: String,
    pub point: String,
    /// 为什么没生效：`unknown_point` / `point_not_implemented` / `unknown_action:xx` /
    /// `duplicate_hook_point` / 分级函数给的理由（如 `control_plane_deny_not_permitted`）
    pub reason: String,
}

/// 钩子运行时（[`HookRunner`] 的生产实现）。
pub struct HookRuntime {
    script_host: Arc<dyn ScriptHost>,
    /// 注册表用内部可变性（P2·SP-46 的插件启停会**原地重装配**它）。
    ///
    /// 为什么不是"装配期定死"：引擎持的是 `Arc<dyn HookRunner>`（换不掉），
    /// 而"停用插件之后它的钩子还在拦"正是 SP-46 要消灭的那类半截状态。
    registrations: std::sync::RwLock<Vec<RegisteredHook>>,
    pending_registrations: std::sync::Mutex<Vec<HookRegistration>>,
    budget: HookBudget,
}

impl HookRuntime {
    pub fn new(script_host: Arc<dyn ScriptHost>, budget: HookBudget) -> Self {
        Self {
            script_host,
            registrations: std::sync::RwLock::new(Vec::new()),
            pending_registrations: std::sync::Mutex::new(Vec::new()),
            budget,
        }
    }

    pub fn take_registrations(&self) -> Vec<HookRegistration> {
        self.pending_registrations
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    /// 空运行时（没有脚本宿主，也没有任何注册）。
    ///
    /// 用途只有一个：产品声明 `capabilities.hooks == false` 时，组合根**如实不装钩子**，
    /// 但引擎仍然拿到一个运行时——于是"没装钩子"与"钩子全都没生效"在诊断上分得开
    /// （后者会有 `no_subscriber` 记录）。
    pub fn disabled() -> Self {
        /// 一个**如实拒绝**一切调用的脚本宿主：不会有人调到它（注册表是空的）。
        struct NoScriptHost;

        impl ScriptHost for NoScriptHost {
            fn load<'a>(
                &'a self,
                _req: &'a agent_base::ports::LoadRequest,
                _ctx: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<ModuleHandle, ScriptError>> {
                Box::pin(async {
                    Err(ScriptError::Unavailable {
                        message: "该运行时未接脚本宿主（声明未开钩子）".to_string(),
                    })
                })
            }
            fn exports(&self, _h: &ModuleHandle) -> Result<Vec<String>, ScriptError> {
                Err(ScriptError::Unavailable {
                    message: "该运行时未接脚本宿主".to_string(),
                })
            }
            fn prompt_fragments(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<agent_base::ports::ScriptPromptFragment>, ScriptError> {
                Ok(Vec::new())
            }
            fn skills(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<agent_base::ports::ScriptSkillDeclaration>, ScriptError> {
                Ok(Vec::new())
            }
            fn hooks(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<agent_base::ports::ScriptHookDeclaration>, ScriptError> {
                Ok(Vec::new())
            }
            fn invoke<'a>(
                &'a self,
                _h: &'a ModuleHandle,
                _c: &'a agent_base::ports::InvokeRequest,
                _ctx: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<agent_base::ports::ScriptOutcome, ScriptError>> {
                Box::pin(async {
                    Err(ScriptError::Unavailable {
                        message: "该运行时未接脚本宿主".to_string(),
                    })
                })
            }
            fn invoke_hook<'a>(
                &'a self,
                _h: &'a ModuleHandle,
                _p: &'a str,
                _payload: serde_json::Value,
                _ctx: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<agent_base::ports::ScriptHookOutcome, ScriptError>> {
                Box::pin(async {
                    Err(ScriptError::Unavailable {
                        message: "该运行时未接脚本宿主".to_string(),
                    })
                })
            }
            fn dispose<'a>(&'a self, _h: ModuleHandle) -> BoxFuture<'a, ()> {
                Box::pin(async {})
            }
        }

        Self {
            script_host: Arc::new(NoScriptHost),
            registrations: std::sync::RwLock::new(Vec::new()),
            pending_registrations: std::sync::Mutex::new(Vec::new()),
            budget: HookBudget::default(),
        }
    }

    /// 注册一条钩子（装配期逐步调用；拒绝理由由调用方收集，见 [`collect_hooks`]）。
    ///
    /// 校验顺序刻意是"先点位、再动作、最后分级"：这样"这个点位根本不存在"与
    /// "你在控制面上越级了"是两条不同的理由，作者一眼能看出该改哪里。
    pub fn register(
        &self,
        plugin_id: &str,
        trust: PluginTrust,
        handle: ModuleHandle,
        decl: &PluginHookDeclaration,
        rejections: &mut Vec<HookRejection>,
    ) {
        let point = decl.point.trim().to_string();
        let reject = |rejections: &mut Vec<HookRejection>, reason: String| {
            rejections.push(HookRejection {
                plugin_id: plugin_id.to_string(),
                point: point.clone(),
                reason,
            });
        };

        let level = match HookLevel::of(&point) {
            Some(l) => l,
            None => {
                reject(rejections, "unknown_point".to_string());
                return;
            }
        };
        if !points::IMPLEMENTED.contains(&point.as_str()) {
            // 点位在分级表里有、但本批还没接（例如 beforeTurn）——**如实说**，
            // 而不是收下之后永不调用（那正是 AGENTS §15 的坑）
            reject(rejections, "point_not_implemented".to_string());
            return;
        }
        if self
            .registrations
            .read()
            .map(|r| r.iter().any(|h| h.plugin_id == plugin_id && h.point == point))
            .unwrap_or(false)
        {
            // 一个插件在一个点位上只留一条：多条处理器会让"谁拦的"变成猜谜
            reject(rejections, "duplicate_hook_point".to_string());
            return;
        }

        if decl.actions.is_empty() {
            // 只声明了"我在这个点位上存在"却没说能做什么：没有可授权的动作，
            // 但**不重复报**（下面那条 `no_permitted_action` 说的是"一个都没留下"）
            reject(rejections, "no_actions_declared".to_string());
            return;
        }

        let mut accepted: Vec<HookAction> = Vec::new();
        let mut reported: usize = 0;
        for raw in &decl.actions {
            let Some(action) = HookAction::parse(raw) else {
                reject(rejections, format!("unknown_action:{raw}"));
                reported += 1;
                continue;
            };
            match allows(level, trust, action) {
                Ok(()) => {
                    // 分级放行之后还有一道**点位**判定（P3b）：不是每个点位都有可改写的载荷。
                    // 放在装配期（而不是运行期静默忽略）是因为"声明了却永远不生效"
                    // 必须在装配期就说清楚——那正是 AGENTS §15 警告的那个坑。
                    if action == HookAction::Replace
                        && !points::REPLACE_SUPPORTED.contains(&point.as_str())
                    {
                        reject(rejections, "replace_not_supported_for_point".to_string());
                        reported += 1;
                        continue;
                    }
                    if !accepted.contains(&action) {
                        accepted.push(action);
                    }
                }
                // 越级 / 未实现：**逐条动作**拒（同一条注册里的其它动作仍可生效）
                Err(reason) => {
                    reject(rejections, reason);
                    reported += 1;
                }
            }
        }
        if accepted.is_empty() {
            // 逐条理由已经报过了就不再补一条笼统的——同一个问题报两遍只会稀释信号；
            // 只有"每条都被认出来但没有一条被拒"（理论上不会发生）才需要兜底。
            if reported == 0 {
                reject(rejections, "no_permitted_action".to_string());
            }
            return;
        }

        if let Ok(mut regs) = self.registrations.write() {
            regs.push(RegisteredHook {
                plugin_id: plugin_id.to_string(),
                trust,
                point,
                actions: accepted,
                order: decl.order,
                handle,
            });
        }
    }

    /// **受控重装配**（P2·SP-46 的插件启停会调它）：清空注册，按新收集到的插件重装。
    ///
    /// 为什么要原地换而不是新建一个运行时：引擎持的是 `Arc<dyn HookRunner>`，换不掉。
    /// 新建一个的结果是"界面上关掉了插件，它的钩子还在拦"——正是 SP-46 要消灭的半截状态。
    pub fn reassemble(
        &self,
        plugins: &[(String, PluginTrust, ModuleHandle)],
        rejections: &mut Vec<HookRejection>,
    ) {
        if let Ok(mut regs) = self.registrations.write() {
            regs.clear();
        }
        for (plugin_id, trust, handle) in plugins {
            let decls = match self.script_host.hooks(handle) {
                Ok(d) => d,
                Err(e) => {
                    rejections.push(HookRejection {
                        plugin_id: plugin_id.clone(),
                        point: String::new(),
                        reason: format!("declarations_unreadable:{e:?}"),
                    });
                    continue;
                }
            };
            for decl in decls {
                self.register(
                    plugin_id,
                    *trust,
                    handle.clone(),
                    &PluginHookDeclaration {
                        point: decl.point,
                        actions: decl.actions,
                        order: decl.order,
                    },
                    rejections,
                );
            }
        }
    }

    /// 某点位上的注册（**已排序**：内置优先 → `order` → 插件 id 兜底）。
    ///
    /// 返回**克隆**而不是引用：注册表在 `RwLock` 后面（要支持原地重装配），
    /// 引用活不过那把锁的作用域。注册条数是个位数，克隆的代价可以忽略。
    pub fn registrations(&self, point: &str) -> Vec<RegisteredHook> {
        let mut regs: Vec<RegisteredHook> = self
            .registrations
            .read()
            .map(|r| r.iter().filter(|r| r.point == point).cloned().collect())
            .unwrap_or_default();
        regs.sort_by(|a, b| {
            // 内置优先：它代表"本项目自己的规矩"，第三方插件不该插到它前面
            let trust_rank = |t: PluginTrust| match t {
                PluginTrust::Builtin => 0,
                PluginTrust::ThirdParty => 1,
            };
            trust_rank(a.trust)
                .cmp(&trust_rank(b.trust))
                .then_with(|| a.order.cmp(&b.order))
                .then_with(|| a.plugin_id.cmp(&b.plugin_id))
        });
        regs
    }

    pub fn len(&self) -> usize {
        self.registrations.read().map(|r| r.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn budget(&self) -> HookBudget {
        self.budget
    }

    /// 跑一个点位的一条注册；返回这一条钩子的**完整结果**（含可能的改写）。
    async fn run_one(
        &self,
        reg: &RegisteredHook,
        point: &str,
        payload: serde_json::Value,
        ctx: &HookCtx<'_>,
    ) -> HookEntryResult {
        let script_ctx = ScriptCtx {
            budget: ScriptBudget {
                timeout_ms: self.budget.timeout_ms,
                ..Default::default()
            },
            cancel: ctx.cancel,
            thread_id: ctx.thread_id,
            origin_id: Some(reg.plugin_id.as_str()),
        };
        let script_payload = serde_json::json!({
            "__ada_hook_event": payload,
            "__ada_hook_ctx": {
                "threadId": ctx.thread_id,
                "turnId": ctx.turn_id,
                "callId": ctx.call_id,
                "pluginId": reg.plugin_id,
                "point": point,
                "timeoutMs": self.budget.timeout_ms,
            },
        });
        let call = self
            .script_host
            .invoke_hook(&reg.handle, point, script_payload, &script_ctx);

        // 单点位预算：**超时即无意见**（§5.5）。这里再兜一层墙钟——
        // 沙箱自己也有一层，但那是"这个脚本跑太久"，这里是"这个点位的预算"，
        // 两者语义不同（前者是脚本的错，后者是插件的错）。
        let res = tokio::time::timeout(Duration::from_millis(self.budget.timeout_ms), call).await;

        let raw = match res {
            Err(_) => return HookEntryResult::simple(HookEntryOutcome::TimedOut, "timed_out"),
            Ok(Err(e)) => {
                let (outcome, reason) = match e {
                    ScriptError::TimedOut { .. } | ScriptError::BudgetExceeded { .. } => {
                        (HookEntryOutcome::TimedOut, "timed_out".to_string())
                    }
                    ScriptError::Aborted => (HookEntryOutcome::Aborted, "aborted".to_string()),
                    // 钩子自己坏了：**无意见**（它没有资格因为自己坏而拦住工具）
                    other => (HookEntryOutcome::Passed, format!("hook_failed:{other:?}")),
                };
                return HookEntryResult::simple(outcome, &reason);
            }
            Ok(Ok(raw)) => raw,
        };

        self.collect_registrations(reg, &raw);

        match raw.action.as_deref().map(str::trim) {
            None | Some("") => HookEntryResult::simple(HookEntryOutcome::Passed, ""),
            Some("observe") => HookEntryResult::new(HookEntryOutcome::Observed).with_note(
                AfterNote::Observe {
                    data: raw.data.clone().unwrap_or(serde_json::Value::Null),
                },
            ),
            Some("note") => HookEntryResult::new(HookEntryOutcome::Observed).with_note(
                AfterNote::Note {
                    text: raw
                        .note
                        .clone()
                        .or_else(|| raw.reason.clone())
                        .unwrap_or_default(),
                },
            ),
            Some("deny") => {
                if reg.declares(HookAction::Deny) {
                    HookEntryResult::simple(
                        HookEntryOutcome::Denied,
                        &raw.reason
                            .clone()
                            .unwrap_or_else(|| "插件拦下了这次调用（未说明理由）".to_string()),
                    )
                } else {
                    // 声明即授权：没声明 deny 却返回 deny → 不生效，但**说清楚**
                    HookEntryResult::simple(HookEntryOutcome::Passed, "action_not_declared:deny")
                }
            }
            Some("abort") | Some("abortturn") => {
                if reg.declares(HookAction::AbortTurn) {
                    HookEntryResult::simple(
                        HookEntryOutcome::Aborted,
                        &raw.reason
                            .clone()
                            .unwrap_or_else(|| "插件结束了整轮（未说明理由）".to_string()),
                    )
                } else {
                    HookEntryResult::simple(HookEntryOutcome::Passed, "action_not_declared:abort")
                }
            }
            // ── 改写（P3b · SP-60）───────────────────────────────────────────
            Some("replace") | Some("replaceargs") => {
                if !reg.declares(HookAction::Replace) {
                    return HookEntryResult::simple(
                        HookEntryOutcome::Passed,
                        "action_not_declared:replace",
                    );
                }
                if !points::REPLACE_SUPPORTED.contains(&point) {
                    // 装配期就该拦（`replace_not_supported_for_point`）；这里兜底
                    return HookEntryResult::simple(
                        HookEntryOutcome::Passed,
                        "replace_not_supported_for_point",
                    );
                }
                // 载荷契约：`{ action: 'replace', args: { ... } }`。
                // 只接受**对象**：数组/标量/字符串都没法再走 schema 校验，
                // 与其猜，不如如实拒掉并说清契约。
                match raw.data.as_ref().and_then(|d| d.get("args")) {
                    Some(serde_json::Value::Object(_)) => {
                        let new_args = raw.data.clone().unwrap_or(serde_json::Value::Null);
                        let new = new_args.get("args").cloned().unwrap_or_default();
                        let before_hash = agent_base::ports::hooks::content_hash8(&payload);
                        let after_hash = agent_base::ports::hooks::content_hash8(&new);
                        let bytes_before =
                            serde_json::to_string(&payload).map(|s| s.len()).unwrap_or(0) as i64;
                        let bytes_after =
                            serde_json::to_string(&new).map(|s| s.len()).unwrap_or(0) as i64;
                        HookEntryResult::replacement(new, before_hash, after_hash, bytes_after - bytes_before)
                    }
                    _ => HookEntryResult::simple(
                        HookEntryOutcome::Passed,
                        "replace_payload_invalid:期望 { action: 'replace', args: {...} }",
                    ),
                }
            }
            Some(other) => HookEntryResult::simple(
                HookEntryOutcome::Passed,
                &format!("unknown_action_returned:{other}"),
            ),
        }
    }

    fn collect_registrations(&self, reg: &RegisteredHook, raw: &agent_base::ports::ScriptHookOutcome) {
        let Some(items) = raw
            .data
            .as_ref()
            .and_then(|data| data.get("registrations"))
            .and_then(|items| items.as_array())
        else {
            return;
        };

        let mut pending = match self.pending_registrations.lock() {
            Ok(pending) => pending,
            Err(_) => return,
        };

        for item in items {
            let Some(kind) = item.get("kind").and_then(|value| value.as_str()) else {
                continue;
            };
            match kind {
                "tool" => {
                    let (Some(name), Some(execute_export)) = (
                        item.get("name").and_then(|value| value.as_str()),
                        item.get("executeExport").and_then(|value| value.as_str()),
                    ) else {
                        continue;
                    };
                    pending.push(HookRegistration::Tool {
                        plugin_id: reg.plugin_id.clone(),
                        name: name.to_string(),
                        description: item
                            .get("description")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                        parameters: item
                            .get("parameters")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({
                                "type": "object",
                                "properties": {}
                            })),
                        execute_export: execute_export.to_string(),
                    });
                }
                "systemPrompt" => {
                    let (Some(id), Some(content)) = (
                        item.get("id").and_then(|value| value.as_str()),
                        item.get("content").and_then(|value| value.as_str()),
                    ) else {
                        continue;
                    };
                    pending.push(HookRegistration::SystemPrompt {
                        plugin_id: reg.plugin_id.clone(),
                        id: id.to_string(),
                        content: content.to_string(),
                    });
                }
                "skill" => {
                    let (Some(name), Some(path)) = (
                        item.get("name").and_then(|value| value.as_str()),
                        item.get("path").and_then(|value| value.as_str()),
                    ) else {
                        continue;
                    };
                    pending.push(HookRegistration::Skill {
                        plugin_id: reg.plugin_id.clone(),
                        name: name.to_string(),
                        description: item
                            .get("description")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                        path: path.to_string(),
                        body: item
                            .get("body")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        when_to_use: item
                            .get("whenToUse")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        user_invocable: item.get("userInvocable").and_then(|value| value.as_bool()),
                    });
                }
                _ => {}
            }
        }
    }
}

/// 一条钩子跑完的完整结果（P3b 起含改写载荷与改写痕迹）。
#[derive(Debug)]
pub(crate) struct HookEntryResult {
    /// 这一条钩子的结果（**没有 Default**：默认一个"通过"会让忘记赋值的分支静默变成放行）
    pub outcome: HookEntryOutcome,
    pub reason: Option<String>,
    pub note: Option<AfterNote>,
    /// 改写后的**参数对象**（只有 `replace` 动作会带）
    pub replacement: Option<serde_json::Value>,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub bytes_delta: Option<i64>,
}

impl HookEntryResult {
    /// 只有结果、没有别的（放行 / 拒绝 / 超时 / 观察）。
    ///
    /// 刻意**不给 `Default`**：默认一个 `Passed` 会让"忘了赋值"的分支静默变成放行——
    /// 这套机制里最不该有的默认值就是它。
    fn new(outcome: HookEntryOutcome) -> Self {
        Self {
            outcome,
            reason: None,
            note: None,
            replacement: None,
            before_hash: None,
            after_hash: None,
            bytes_delta: None,
        }
    }

    fn simple(outcome: HookEntryOutcome, reason: &str) -> Self {
        let mut r = Self::new(outcome);
        if !reason.is_empty() {
            r.reason = Some(reason.to_string());
        }
        r
    }

    fn with_note(mut self, note: AfterNote) -> Self {
        self.note = Some(note);
        self
    }

    fn replacement(
        args: serde_json::Value,
        before_hash: String,
        after_hash: String,
        bytes_delta: i64,
    ) -> Self {
        let mut r = Self::new(HookEntryOutcome::Replaced);
        r.replacement = Some(args);
        r.before_hash = Some(before_hash);
        r.after_hash = Some(after_hash);
        r.bytes_delta = Some(bytes_delta);
        r
    }
}

impl HookRunner for HookRuntime {
    fn take_registrations(&self) -> Vec<HookRegistration> {
        HookRuntime::take_registrations(self)
    }

    fn before<'a>(
        &'a self,
        req: BeforeRequest,
        ctx: &'a HookCtx<'a>,
    ) -> BoxFuture<'a, BeforeOutcome> {
        Box::pin(async move {
            let regs = self.registrations(&req.point);
            if regs.is_empty() {
                // 缺席可见：没人订阅这个点位也要报一条，否则"没人管"与"没装过"分不出来
                (ctx.on_event)(HookObservation {
                    call_id: format!("{}#none#0", req.point),
                    point: req.point.clone(),
                    plugin_id: String::new(),
                    phase: HookPhase::Finished,
                    entry_outcome: Some(HookEntryOutcome::Passed),
                    duration_ms: Some(0),
                    reason: Some("no_subscriber".to_string()),
                    before_hash: None,
                    after_hash: None,
                    bytes_delta: None,
                });
                return BeforeOutcome::Pass;
            }

            // 链式（§5.5）：前一个插件的改写结果就是后一个插件的输入——
            // "多个插件共同完善一份参数"只有这样才能自然成立。
            let mut current_payload = req.payload.clone();
            // 最后一个改写者（改写不是授权，但"谁改的"必须说得出来）
            let mut last_replacer: Option<(String, serde_json::Value)> = None;

            for (idx, reg) in regs.iter().enumerate() {
                let call_id = format!("{}#{}#{}", req.point, reg.plugin_id, idx);
                (ctx.on_event)(HookObservation {
                    call_id: call_id.clone(),
                    point: req.point.clone(),
                    plugin_id: reg.plugin_id.clone(),
                    phase: HookPhase::Entered,
                    entry_outcome: None,
                    duration_ms: None,
                    reason: None,
                    before_hash: None,
                    after_hash: None,
                    bytes_delta: None,
                });
                let started = Instant::now();
                let entry = self.run_one(reg, &req.point, current_payload.clone(), ctx).await;
                let elapsed = started.elapsed().as_millis() as u64;
                (ctx.on_event)(HookObservation {
                    call_id,
                    point: req.point.clone(),
                    plugin_id: reg.plugin_id.clone(),
                    phase: HookPhase::Finished,
                    entry_outcome: Some(entry.outcome),
                    duration_ms: Some(elapsed),
                    reason: entry.reason.clone(),
                    before_hash: entry.before_hash.clone(),
                    after_hash: entry.after_hash.clone(),
                    bytes_delta: entry.bytes_delta,
                });

                // 改写：把新参数交给下一个插件（链式只往前走，不允许改回来）
                if let Some(new_args) = entry.replacement {
                    current_payload = replace_args_in_payload(&current_payload, new_args.clone());
                    last_replacer = Some((reg.plugin_id.clone(), new_args));
                }

                // **定稿**：拦下 / 结束整轮之后，后续插件不再被问（§5.5）
                match entry.outcome {
                    HookEntryOutcome::Denied => {
                        return BeforeOutcome::Deny {
                            reason: entry
                                .reason
                                .unwrap_or_else(|| "操作被插件拦下".to_string()),
                        }
                    }
                    HookEntryOutcome::Aborted => {
                        return BeforeOutcome::AbortTurn {
                            reason: entry
                                .reason
                                .unwrap_or_else(|| "本轮被插件结束".to_string()),
                        }
                    }
                    _ => {}
                }
            }
            match last_replacer {
                Some((by, args)) => BeforeOutcome::Replace { args, by },
                None => BeforeOutcome::Pass,
            }
        })
    }

    fn after<'a>(&'a self, req: AfterRequest, ctx: &'a HookCtx<'a>) -> BoxFuture<'a, Vec<AfterNote>> {
        Box::pin(async move {
            let regs = self.registrations(&req.point);
            let mut notes = Vec::new();
            if regs.is_empty() {
                (ctx.on_event)(HookObservation {
                    call_id: format!("{}#none#0", req.point),
                    point: req.point.clone(),
                    plugin_id: String::new(),
                    phase: HookPhase::Finished,
                    entry_outcome: Some(HookEntryOutcome::Passed),
                    duration_ms: Some(0),
                    reason: Some("no_subscriber".to_string()),
                    before_hash: None,
                    after_hash: None,
                    bytes_delta: None,
                });
                return notes;
            }
            for (idx, reg) in regs.iter().enumerate() {
                let call_id = format!("{}#{}#{}", req.point, reg.plugin_id, idx);
                (ctx.on_event)(HookObservation {
                    call_id: call_id.clone(),
                    point: req.point.clone(),
                    plugin_id: reg.plugin_id.clone(),
                    phase: HookPhase::Entered,
                    entry_outcome: None,
                    duration_ms: None,
                    reason: None,
                    before_hash: None,
                    after_hash: None,
                    bytes_delta: None,
                });
                let started = Instant::now();
                let entry = self.run_one(reg, &req.point, req.payload.clone(), ctx).await;
                (ctx.on_event)(HookObservation {
                    call_id,
                    point: req.point.clone(),
                    plugin_id: reg.plugin_id.clone(),
                    phase: HookPhase::Finished,
                    entry_outcome: Some(entry.outcome),
                    duration_ms: Some(started.elapsed().as_millis() as u64),
                    reason: entry.reason.clone(),
                    before_hash: entry.before_hash.clone(),
                    after_hash: entry.after_hash.clone(),
                    bytes_delta: entry.bytes_delta,
                });
                // 事后**没有否决权**（§5.3）：只收旁注，且不改变任何已发生的事实
                if let Some(n) = entry.note {
                    let keep = match &n {
                        AfterNote::Note { text } => !text.trim().is_empty(),
                        AfterNote::Observe { data } => !data.is_null(),
                    };
                    if keep {
                        notes.push(n);
                    }
                }
            }
            notes
        })
    }
}

/// 把改写后的参数**换进载荷**（`beforeToolCall` 的载荷形状是
/// `{ tool, args, isWrite }`）。
///
/// 只动 `args`：工具名与读写性**不属于可改写载荷**（设计 §5.6 第 2 条）——
/// 插件改不了"这次调的是哪个工具、它是不是写工具"。
fn replace_args_in_payload(payload: &serde_json::Value, new_args: serde_json::Value) -> serde_json::Value {
    let mut out = payload.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("args".to_string(), new_args);
    }
    out
}

/// 装配期收集（与 `collect_plugin_tools` 同形）：把插件的钩子声明装进运行时，
/// **逐条**如实回报被拒的声明。
///
/// `level`/`trust` 的判定只在 [`agent_base::ports::hooks::allows`] 一处——
/// 各层各判一遍，迟早出现"运行期能拦、装配期说不行"这种自相矛盾。
pub fn collect_hooks(
    script_host: Arc<dyn ScriptHost>,
    budget: HookBudget,
    #[allow(clippy::type_complexity)]
    plugins: &[(String, PluginTrust, ModuleHandle)],
) -> (HookRuntime, Vec<HookRejection>) {
    let runtime = HookRuntime::new(script_host, budget);
    let mut rejections = Vec::new();
    for (plugin_id, trust, handle) in plugins {
        let decls = match runtime.script_host.hooks(handle) {
            Ok(d) => d,
            Err(e) => {
                rejections.push(HookRejection {
                    plugin_id: plugin_id.clone(),
                    point: String::new(),
                    reason: format!("declarations_unreadable:{e:?}"),
                });
                continue;
            }
        };
        for decl in decls {
            runtime.register(
                plugin_id,
                *trust,
                handle.clone(),
                &PluginHookDeclaration {
                    point: decl.point,
                    actions: decl.actions,
                    order: decl.order,
                },
                &mut rejections,
            );
        }
    }
    (runtime, rejections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::ports::hooks::HookPhase;
    use agent_base::ports::{InvokeRequest, LoadRequest, ScriptOutcome, ScriptPromptFragment};
    use agent_base::testing::NeverCancel;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// 假脚本宿主：按 `(handle.id, point)` 查预置的返回；可选"睡一会儿"制造超时。
    #[derive(Default)]
    struct FakeHost {
        /// `handle.id` → 该模块声明的点位
        decls: HashMap<String, Vec<PluginHookDeclaration>>,
        /// `handle.id#point` → 处理器返回的动作
        returns: HashMap<String, serde_json::Value>,
        /// `handle.id#point` → 睡多久（毫秒），用于超时用例
        delays: HashMap<String, u64>,
        /// 真实调用顺序（断言"内置优先 / 定稿 / 链式"用）
        calls: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn declare(&mut self, handle: &str, point: &str, actions: &[&str], order: i32) {
            self.decls.entry(handle.to_string()).or_default().push(
                PluginHookDeclaration {
                    point: point.to_string(),
                    actions: actions.iter().map(|s| s.to_string()).collect(),
                    order,
                },
            );
        }

        fn ret(&mut self, handle: &str, point: &str, value: serde_json::Value) {
            self.returns.insert(format!("{handle}#{point}"), value);
        }

        fn delay(&mut self, handle: &str, point: &str, ms: u64) {
            self.delays.insert(format!("{handle}#{point}"), ms);
        }

        fn called(&self) -> Vec<String> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl ScriptHost for FakeHost {
        fn load<'a>(
            &'a self,
            _req: &'a LoadRequest,
            _ctx: &'a ScriptCtx<'a>,
        ) -> BoxFuture<'a, Result<ModuleHandle, ScriptError>> {
            Box::pin(async {
                Err(ScriptError::Unavailable {
                    message: "用例不走 load".into(),
                })
            })
        }
        fn exports(&self, _h: &ModuleHandle) -> Result<Vec<String>, ScriptError> {
            Ok(Vec::new())
        }
        fn prompt_fragments(
            &self,
            _h: &ModuleHandle,
        ) -> Result<Vec<ScriptPromptFragment>, ScriptError> {
            Ok(Vec::new())
        }
        fn skills(
            &self,
            _h: &ModuleHandle,
        ) -> Result<Vec<agent_base::ports::ScriptSkillDeclaration>, ScriptError> {
            Ok(Vec::new())
        }
        fn hooks(
            &self,
            h: &ModuleHandle,
        ) -> Result<Vec<agent_base::ports::ScriptHookDeclaration>, ScriptError> {
            Ok(self
                .decls
                .get(&h.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|d| agent_base::ports::ScriptHookDeclaration {
                    point: d.point,
                    actions: d.actions,
                    order: d.order,
                })
                .collect())
        }
        fn invoke<'a>(
            &'a self,
            _h: &'a ModuleHandle,
            _c: &'a InvokeRequest,
            _ctx: &'a ScriptCtx<'a>,
        ) -> BoxFuture<'a, Result<ScriptOutcome, ScriptError>> {
            Box::pin(async {
                Err(ScriptError::Unavailable {
                    message: "用例不走 invoke".into(),
                })
            })
        }
        fn invoke_hook<'a>(
            &'a self,
            h: &'a ModuleHandle,
            point: &'a str,
            _payload: serde_json::Value,
            _ctx: &'a ScriptCtx<'a>,
        ) -> BoxFuture<'a, Result<agent_base::ports::ScriptHookOutcome, ScriptError>> {
            let key = format!("{}#{}", h.id, point);
            let delay = self.delays.get(&key).copied().unwrap_or(0);
            let value = self
                .returns
                .get(&key)
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let Ok(mut c) = self.calls.lock() {
                c.push(key.clone());
            }
            Box::pin(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                Ok(agent_base::ports::ScriptHookOutcome {
                    action: value.get("action").and_then(|v| v.as_str()).map(String::from),
                    reason: value.get("reason").and_then(|v| v.as_str()).map(String::from),
                    note: value.get("note").and_then(|v| v.as_str()).map(String::from),
                    data: value.get("data").cloned(),
                })
            })
        }
        fn dispose<'a>(&'a self, _h: ModuleHandle) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }

    fn handle_for(id: &str) -> ModuleHandle {
        ModuleHandle { id: id.to_string() }
    }

    /// 把声明装进运行时（走真实注册路径，包含分级校验）。
    fn runtime_with(
        host: Arc<FakeHost>,
        plugins: &[(&str, PluginTrust, &[(&str, &[&str], i32)])],
        budget_ms: u64,
    ) -> (HookRuntime, Vec<HookRejection>) {
        let mut runtime = HookRuntime::new(
            host,
            HookBudget {
                timeout_ms: budget_ms,
            },
        );
        let mut rejections = Vec::new();
        for (pid, trust, points) in plugins {
            for (point, actions, order) in *points {
                runtime.register(
                    pid,
                    *trust,
                    handle_for(pid),
                    &PluginHookDeclaration {
                        point: point.to_string(),
                        actions: actions.iter().map(|s| s.to_string()).collect(),
                        order: *order,
                    },
                    &mut rejections,
                );
            }
        }
        (runtime, rejections)
    }

    /// 逐条观测的收集器（引擎那边会把它转成成对诊断）。
    struct Observers(Mutex<Vec<HookObservation>>);

    impl Observers {
        fn new() -> Self {
            Self(Mutex::new(Vec::new()))
        }
        fn sink(&self) -> impl Fn(HookObservation) + Send + Sync + '_ {
            move |o: HookObservation| {
                if let Ok(mut v) = self.0.lock() {
                    v.push(o);
                }
            }
        }
        fn all(&self) -> Vec<HookObservation> {
            self.0.lock().map(|v| v.clone()).unwrap_or_default()
        }
    }

    fn ctx<'a>(
        on_event: &'a (dyn Fn(HookObservation) + Send + Sync),
    ) -> HookCtx<'a> {
        HookCtx {
            thread_id: "t1",
            turn_id: "turn1",
            call_id: Some("call1"),
            cancel: &NeverCancel,
            on_event,
        }
    }

    fn before_req(point: &str) -> BeforeRequest {
        BeforeRequest {
            point: point.to_string(),
            subject: Some("run_command".to_string()),
            payload: serde_json::json!({ "name": "run_command", "args": { "command": "rm -rf /" } }),
        }
    }

    /// SP-50：**调用顺序**是"内置优先 → order → 插件 id 兜底"。
    #[tokio::test]
    async fn test_order_is_builtin_first_then_order_then_id() {
        let mut host = FakeHost::default();
        for id in ["zzz-plugin", "aaa-plugin", "builtin-guard"] {
            host.declare(id, "beforeToolCall", &["observe"], 0);
        }
        let host = Arc::new(host);
        let (runtime, rejections) = runtime_with(
            host.clone(),
            &[
                ("zzz-plugin", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 0)]),
                ("aaa-plugin", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 0)]),
                ("builtin-guard", PluginTrust::Builtin, &[("beforeToolCall", &["observe"], 0)]),
            ],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");

        let obs = Observers::new();
        let sink = obs.sink();
        let outcome = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        assert!(outcome.is_pass());
        assert_eq!(
            host.called(),
            vec![
                "builtin-guard#beforeToolCall".to_string(),
                "aaa-plugin#beforeToolCall".to_string(),
                "zzz-plugin#beforeToolCall".to_string(),
            ],
            "内置优先，其余按插件 id 兜底（order 都是 0）"
        );
    }

    /// `order` 小的先跑（同一档位内）。
    #[tokio::test]
    async fn test_order_field_decides_within_the_same_trust_tier() {
        let host = Arc::new(FakeHost::default());
        let (runtime, _) = runtime_with(
            host.clone(),
            &[
                ("late", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 50)]),
                ("early", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 10)]),
            ],
            500,
        );
        let obs = Observers::new();
        let sink = obs.sink();
        let _ = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        // returns 里没有预置，fake 会返回 {}（无意见），但调用顺序仍然记录
        assert_eq!(
            host.called(),
            vec!["early#beforeToolCall".to_string(), "late#beforeToolCall".to_string()]
        );
    }

    /// SP-50：**拦下即定稿**——后面的插件不再被问，且理由原样带出去。
    #[tokio::test]
    async fn test_deny_finalizes_and_stops_asking_later_plugins() {
        let mut host = FakeHost::default();
        host.ret(
            "guard",
            "beforeToolCall",
            serde_json::json!({ "action": "deny", "reason": "这条命令会删库" }),
        );
        let host = Arc::new(host);
        let (runtime, _) = runtime_with(
            host.clone(),
            &[
                ("guard", PluginTrust::ThirdParty, &[("beforeToolCall", &["deny"], 0)]),
                ("later", PluginTrust::ThirdParty, &[("beforeToolCall", &["deny"], 10)]),
            ],
            500,
        );
        let obs = Observers::new();
        let sink = obs.sink();
        let outcome = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        match outcome {
            BeforeOutcome::Deny { reason } => assert_eq!(reason, "这条命令会删库"),
            other => panic!("应当被拦下，实际 {other:?}"),
        }
        assert_eq!(
            host.called(),
            vec!["guard#beforeToolCall".to_string()],
            "定稿之后不许再问别人（否则'谁拦的'就说不清了）"
        );
    }

    /// SP-54：**声明即授权**——没声明 `deny` 却返回 `deny` 不生效，但要留痕。
    #[tokio::test]
    async fn test_action_must_be_declared_to_take_effect() {
        let mut host = FakeHost::default();
        host.ret(
            "sneaky",
            "beforeToolCall",
            serde_json::json!({ "action": "deny", "reason": "我没声明过这个" }),
        );
        let host = Arc::new(host);
        let (runtime, _) = runtime_with(
            host.clone(),
            &[("sneaky", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 0)])],
            500,
        );
        let obs = Observers::new();
        let sink = obs.sink();
        let outcome = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        assert!(outcome.is_pass(), "未声明的动作不得生效：{outcome:?}");
        let finished = obs
            .all()
            .into_iter()
            .find(|o| o.phase == HookPhase::Finished && !o.plugin_id.is_empty())
            .expect("要有结束记录");
        assert_eq!(
            finished.reason.as_deref(),
            Some("action_not_declared:deny"),
            "要留痕说清为什么不生效"
        );
    }

    /// SP-53：**超时即 `Pass`**——插件卡住不该让用户等。
    #[tokio::test]
    async fn test_timeout_is_treated_as_no_opinion() {
        let mut host = FakeHost::default();
        host.delay("slow", "beforeToolCall", 300);
        host.ret(
            "slow",
            "beforeToolCall",
            serde_json::json!({ "action": "deny", "reason": "本来想拦" }),
        );
        let host = Arc::new(host);
        // 预算 50ms，插件要睡 300ms
        let (runtime, _) = runtime_with(
            host,
            &[("slow", PluginTrust::ThirdParty, &[("beforeToolCall", &["deny"], 0)])],
            50,
        );
        let obs = Observers::new();
        let sink = obs.sink();
        let started = std::time::Instant::now();
        let outcome = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        assert!(outcome.is_pass(), "超时必须视为无意见，实际 {outcome:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "必须在预算内返回（不能等插件睡完）"
        );
        let finished = obs
            .all()
            .into_iter()
            .find(|o| o.phase == HookPhase::Finished)
            .expect("要有结束记录");
        assert_eq!(finished.entry_outcome, Some(HookEntryOutcome::TimedOut));
    }

    /// 钩子自己抛错 / 沙箱失败：**无意见**（它没资格因为自己坏而拦住工具）。
    #[tokio::test]
    async fn test_broken_hook_does_not_block_the_tool() {
        /// 一调就报错的宿主（模拟转译失败 / 脚本抛错）。
        struct BrokenHost;
        impl ScriptHost for BrokenHost {
            fn load<'a>(
                &'a self,
                _r: &'a LoadRequest,
                _c: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<ModuleHandle, ScriptError>> {
                Box::pin(async { Err(ScriptError::Unavailable { message: "x".into() }) })
            }
            fn exports(&self, _h: &ModuleHandle) -> Result<Vec<String>, ScriptError> {
                Ok(Vec::new())
            }
            fn prompt_fragments(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<ScriptPromptFragment>, ScriptError> {
                Ok(Vec::new())
            }
            fn skills(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<agent_base::ports::ScriptSkillDeclaration>, ScriptError> {
                Ok(Vec::new())
            }
            fn hooks(
                &self,
                _h: &ModuleHandle,
            ) -> Result<Vec<agent_base::ports::ScriptHookDeclaration>, ScriptError> {
                Ok(Vec::new())
            }
            fn invoke<'a>(
                &'a self,
                _h: &'a ModuleHandle,
                _c: &'a InvokeRequest,
                _x: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<ScriptOutcome, ScriptError>> {
                Box::pin(async { Err(ScriptError::Unavailable { message: "x".into() }) })
            }
            fn invoke_hook<'a>(
                &'a self,
                _h: &'a ModuleHandle,
                _p: &'a str,
                _v: serde_json::Value,
                _x: &'a ScriptCtx<'a>,
            ) -> BoxFuture<'a, Result<agent_base::ports::ScriptHookOutcome, ScriptError>> {
                Box::pin(async {
                    Err(ScriptError::Script {
                        message: "插件处理器抛了".into(),
                    })
                })
            }
            fn dispose<'a>(&'a self, _h: ModuleHandle) -> BoxFuture<'a, ()> {
                Box::pin(async {})
            }
        }

        let mut runtime = HookRuntime::new(
            Arc::new(BrokenHost),
            HookBudget { timeout_ms: 500 },
        );
        let mut rejections = Vec::new();
        runtime.register(
            "bad",
            PluginTrust::ThirdParty,
            handle_for("bad"),
            &PluginHookDeclaration {
                point: "beforeToolCall".to_string(),
                actions: vec!["deny".to_string()],
                order: 0,
            },
            &mut rejections,
        );
        assert!(rejections.is_empty(), "{rejections:?}");

        let obs = Observers::new();
        let sink = obs.sink();
        let outcome = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        assert!(outcome.is_pass(), "坏掉的钩子不得拦住工具：{outcome:?}");
        let finished = obs
            .all()
            .into_iter()
            .find(|o| o.phase == HookPhase::Finished)
            .expect("要有结束记录");
        assert!(
            finished
                .reason
                .as_deref()
                .unwrap_or("")
                .starts_with("hook_failed"),
            "要留痕说清是钩子自己坏了：{finished:?}"
        );
    }

    /// SP-52：**成对性**——每条 entered 恰好一个终结记录；缺席也要报（`no_subscriber`）。
    #[tokio::test]
    async fn test_observations_are_paired_and_absence_is_visible() {
        let host = Arc::new(FakeHost::default());
        let (runtime, _) = runtime_with(
            host,
            &[("p1", PluginTrust::ThirdParty, &[("beforeToolCall", &["observe"], 0)])],
            500,
        );
        let obs = Observers::new();
        let sink = obs.sink();
        let _ = runtime.before(before_req("beforeToolCall"), &ctx(&sink)).await;
        let all = obs.all();
        let entered: Vec<_> = all.iter().filter(|o| o.phase == HookPhase::Entered).collect();
        let finished: Vec<_> = all.iter().filter(|o| o.phase == HookPhase::Finished).collect();
        assert_eq!(entered.len(), 1);
        assert_eq!(finished.len(), 1, "一进必须恰好一出：{all:?}");
        assert_eq!(entered[0].call_id, finished[0].call_id, "配对键必须一致");

        // 没人订阅的点位：报 `no_subscriber`（"没人管"与"没装过"要分得出来）
        let obs2 = Observers::new();
        let sink2 = obs2.sink();
        let outcome = runtime.before(before_req("afterToolCall"), &ctx(&sink2)).await;
        assert!(outcome.is_pass());
        let all2 = obs2.all();
        assert_eq!(all2.len(), 1);
        assert_eq!(all2[0].reason.as_deref(), Some("no_subscriber"));
    }

    /// SP-54 的正反两面：越级被拒 + 同点位内置可用。
    #[tokio::test]
    async fn test_grading_rejects_privilege_escalation_at_assembly_time() {
        let host = Arc::new(FakeHost::default());

        // ① 第三方想在控制面点位上拦下 → 拒
        let (_, rejections) = runtime_with(
            host.clone(),
            &[(
                "third-party",
                PluginTrust::ThirdParty,
                &[("beforePluginToggle", &["deny"], 0)],
            )],
            500,
        );
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert!(
            rejections[0].reason.contains("control_plane_deny_not_permitted"),
            "理由要说清是分级问题：{rejections:?}"
        );

        // ② 同一个点位，内置插件可以拦（分级不是"一律禁止"）
        let (runtime, rejections) = runtime_with(
            host.clone(),
            &[("builtin-guard", PluginTrust::Builtin, &[("beforePluginToggle", &["deny"], 0)])],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(runtime.len(), 1);

        // ③ 数据面：第三方可以拦
        let (runtime, rejections) = runtime_with(
            host.clone(),
            &[("third-party", PluginTrust::ThirdParty, &[("beforeToolCall", &["deny"], 0)])],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(runtime.len(), 1);

        // ④ 第三方要"结束整轮" → 拒（那是拒绝服务）
        let (_, rejections) = runtime_with(
            host.clone(),
            &[("third-party", PluginTrust::ThirdParty, &[("beforeToolCall", &["abort"], 0)])],
            500,
        );
        assert!(
            rejections[0].reason.contains("abort_not_permitted"),
            "{rejections:?}"
        );

        // ⑤ 改写（P3b 已开）：数据面上的第三方**可以**改写——改写不是授权，
        //    改完还要过 schema / 路径 / 风险单调性（引擎侧，见 `hook_guard`）
        let (runtime, rejections) = runtime_with(
            host.clone(),
            &[(
                "third-party",
                PluginTrust::ThirdParty,
                &[("beforeToolCall", &["replace"], 0)],
            )],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(runtime.len(), 1);

        // ⑤b 但**没有可改写载荷**的点位照样拒（控制面那几条只有"允许/不允许"）
        let (_, rejections) = runtime_with(
            host.clone(),
            &[(
                "builtin-guard",
                PluginTrust::Builtin,
                &[("beforePluginToggle", &["replace"], 0)],
            )],
            500,
        );
        assert_eq!(
            rejections[0].reason, "replace_not_supported_for_point",
            "{rejections:?}"
        );

        // ⑥ 本批还没接的点位（beforeTurn 在分级表里有、但没实现）→ 如实说
        let (_, rejections) = runtime_with(
            host.clone(),
            &[("p", PluginTrust::ThirdParty, &[("beforeTurn", &["deny"], 0)])],
            500,
        );
        assert!(rejections[0].reason.starts_with("control_plane_deny_not_permitted"));

        // ⑦ 根本不存在的点位 → 另一个理由
        let (_, rejections) = runtime_with(
            host,
            &[("p", PluginTrust::ThirdParty, &[("afterDeploy", &["observe"], 0)])],
            500,
        );
        assert_eq!(rejections[0].reason, "unknown_point");
    }

    /// 一个插件在一个点位上只留一条注册（多条会让"谁拦的"变成猜谜）。
    #[tokio::test]
    async fn test_duplicate_registration_for_the_same_point_is_rejected() {
        let host = Arc::new(FakeHost::default());
        let (runtime, rejections) = runtime_with(
            host,
            &[(
                "p1",
                PluginTrust::ThirdParty,
                &[("beforeToolCall", &["deny"], 0), ("beforeToolCall", &["deny"], 0)],
            )],
            500,
        );
        assert_eq!(runtime.len(), 1);
        assert_eq!(rejections.len(), 1);
        assert_eq!(rejections[0].reason, "duplicate_hook_point");
    }

    /// 事后只收旁注：`note` 进模型上下文（通过返回值交回调用方），空旁的丢掉。
    #[tokio::test]
    async fn test_after_collects_notes_and_drops_empty_ones() {
        let mut host = FakeHost::default();
        host.ret(
            "talker",
            "afterToolCall",
            serde_json::json!({ "action": "note", "note": "记得跑一遍测试" }),
        );
        host.ret("mute", "afterToolCall", serde_json::json!({ "action": "note", "note": "  " }));
        let host = Arc::new(host);
        let (runtime, rejections) = runtime_with(
            host,
            &[
                ("talker", PluginTrust::ThirdParty, &[("afterToolCall", &["note"], 0)]),
                ("mute", PluginTrust::ThirdParty, &[("afterToolCall", &["note"], 10)]),
            ],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        let obs = Observers::new();
        let sink = obs.sink();
        let notes = runtime
            .after(
                AfterRequest {
                    point: "afterToolCall".to_string(),
                    subject: Some("run_command".to_string()),
                    payload: serde_json::json!({ "status": "success" }),
                },
                &ctx(&sink),
            )
            .await;
        assert_eq!(
            notes,
            vec![AfterNote::Note {
                text: "记得跑一遍测试".to_string()
            }],
            "只有非空旁注进模型"
        );
        assert_eq!(
            obs.all().iter().filter(|o| o.phase == HookPhase::Entered).count(),
            2,
            "两条钩子都要被调到（事后没有否决权，不因前一条而短路）"
        );
    }

    #[tokio::test]
    async fn test_hook_registration_requests_are_drained_by_host() {
        let mut host = FakeHost::default();
        host.ret(
            "registrar",
            "afterTurn",
            serde_json::json!({
                "action": "observe",
                "data": {
                    "registrations": [
                        { "kind": "tool", "name": "dynamic_tool", "description": "dynamic", "parameters": { "type": "object" }, "executeExport": "dynamic_tool" },
                        { "kind": "systemPrompt", "id": "dynamic-prompt", "content": "Use dynamic prompt" },
                        { "kind": "skill", "name": "dynamic-skill", "description": "dynamic skill", "path": ".ada/agent-coding/skills/dynamic-skill.md" }
                    ]
                }
            }),
        );
        let host = Arc::new(host);
        let (runtime, rejections) = runtime_with(
            host,
            &[("registrar", PluginTrust::ThirdParty, &[("afterTurn", &["observe"], 0)])],
            500,
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        let observations = Observers::new();
        let sink = observations.sink();
        let _ = runtime
            .after(
                AfterRequest {
                    point: "afterTurn".to_string(),
                    subject: None,
                    payload: serde_json::json!({ "step": 1 }),
                },
                &ctx(&sink),
            )
            .await;
        let registrations = runtime.take_registrations();
        assert_eq!(registrations.len(), 3);
        assert!(matches!(registrations[0], HookRegistration::Tool { ref name, ref execute_export, .. } if name == "dynamic_tool" && execute_export == "dynamic_tool"));
        assert!(matches!(registrations[1], HookRegistration::SystemPrompt { ref id, .. } if id == "dynamic-prompt"));
        assert!(matches!(registrations[2], HookRegistration::Skill { ref name, ref path, .. } if name == "dynamic-skill" && path.ends_with("dynamic-skill.md")));
    }
}
