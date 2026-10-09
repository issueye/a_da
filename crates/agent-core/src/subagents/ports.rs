//! 子智能体隔离执行所需的**生产**端口实现（W4-T6）。
//!
//! 子智能体并入 `AgentRuntime::run_turn` 后，需要一套"隔离上下文"的端口。
//! 这三个实现都是**生产代码**（不是测试替身——INV-2 要求端口真接线）：
//!
//! | 端口 | 实现 | 为什么这样选 |
//! |---|---|---|
//! | `PromptSource` | [`ProfilePrompt`] | AGENTS.md §3：子智能体**只**拿到 `profile.system_prompt`，不继承主线程的 AGENTS.md / 系统提示词 |
//! | `SessionStore` | [`EphemeralSessionStore`] | 子智能体上下文**刻意是临时的**：它是一次性委派，不该在主会话的历史里留下痕迹（legacy 也是纯内存） |
//! | `ApprovalGate` | [`ReadonlyEnforcingGate`] | 子智能体**没有用户可以问**，所以不能"等批准"；只读档位下的写工具必须**直接拒绝**（失败安全） |

use std::collections::HashMap;
use std::sync::Mutex;

use agent_base::domain::{AgentError, AgentMessage, ApprovalPolicy, DenialKind, FailDirection};
use agent_base::ports::{
    ApprovalGate, ApprovalOutcome, ApprovalRequest, AnsweredBy, BoxFuture, CancelToken,
    PromptSource, SessionStore,
};

/// 只给出该 profile 的系统提示词（AGENTS.md §3：不继承主线程提示词）。
pub struct ProfilePrompt {
    prompt: String,
}

impl ProfilePrompt {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self { prompt: prompt.into() }
    }
}

impl PromptSource for ProfilePrompt {
    fn system_prompt(&self) -> String {
        self.prompt.clone()
    }
}

/// 一次性委派的临时会话存储（纯内存，进程内隔离）。
///
/// 刻意不落盘：子智能体的中间推理与工具输出属**过程数据**，主会话只需要最终摘要
/// （`SubagentRunResult::summary`）。legacy 也是纯内存，这里保持一致语义。
#[derive(Default)]
pub struct EphemeralSessionStore {
    sessions: Mutex<HashMap<String, Vec<AgentMessage>>>,
}

impl EphemeralSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已记录的消息数（诊断/测试）。
    pub fn message_count(&self, thread_id: &str) -> usize {
        self.sessions
            .lock()
            .expect("子智能体会话锁中毒")
            .get(thread_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

impl SessionStore for EphemeralSessionStore {
    fn load_messages<'a>(
        &'a self,
        thread_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AgentMessage>, AgentError>> {
        Box::pin(async move {
            Ok(self
                .sessions
                .lock()
                .expect("子智能体会话锁中毒")
                .get(thread_id)
                .cloned()
                .unwrap_or_default())
        })
    }

    fn append_message<'a>(
        &'a self,
        thread_id: &'a str,
        message: &'a AgentMessage,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            self.sessions
                .lock()
                .expect("子智能体会话锁中毒")
                .entry(thread_id.to_string())
                .or_default()
                .push(message.clone());
            Ok(())
        })
    }
}

/// 只读档位的**强制门禁**：写工具直接拒绝，而不是"问用户"。
///
/// 为什么需要它（双防线）：工具白名单裁切已经是第一道防线，但门禁是**运行期**的第二道——
/// 万一裁切逻辑出错（新工具没进只读名单、别名绕过等），这里还能兜住。
/// 子智能体没有交互通道，所以答案只能是"拒绝"，而且必须记 `by: Policy` 以便事后分辨来路。
pub struct ReadonlyEnforcingGate {
    readonly: bool,
}

impl ReadonlyEnforcingGate {
    pub fn new(readonly: bool) -> Self {
        Self { readonly }
    }
}

impl ApprovalGate for ReadonlyEnforcingGate {
    fn direction(&self) -> FailDirection {
        // 拿不到判定依据时拒绝（安全默认）
        FailDirection::Closed
    }

    fn needs_approval<'a>(
        &'a self,
        tool: &'a str,
        _policy: &'a ApprovalPolicy,
        _args: &'a serde_json::Value,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // 只读子智能体 + 非只读工具 → 需要"门禁裁决"（下面 decide 会直接拒绝）
            self.readonly && !agent_toolkit::is_readonly_tool(tool)
        })
    }

    fn decide<'a>(
        &'a self,
        req: ApprovalRequest,
        _cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, ApprovalOutcome> {
        Box::pin(async move {
            ApprovalOutcome::denied(
                AnsweredBy::Policy,
                format!(
                    "安全拦截：只读子智能体严禁调用写工具 [{}]",
                    req.tool
                ),
            )
        })
    }
}

/// 把门禁裁决转成给模型看的工具结果文本（与引擎的拒绝路径同口径）。
pub fn denial_text(tool: &str, outcome: &ApprovalOutcome) -> String {
    match outcome.denial(tool) {
        Some(DenialKind::Approval { reason, .. }) => reason.unwrap_or_else(|| "调用被策略拒绝".to_string()),
        Some(other) => format!("{other:?}"),
        None => "调用被拒绝".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::NeverCancel;

    #[test]
    fn test_profile_prompt_returns_only_the_profile_text() {
        let p = ProfilePrompt::new("只读调研员人格");
        assert_eq!(p.system_prompt(), "只读调研员人格");
        assert_eq!(p.system_prompt(), p.system_prompt(), "必须幂等");
    }

    #[tokio::test]
    async fn test_ephemeral_store_isolates_threads() {
        let store = EphemeralSessionStore::new();
        let msg = AgentMessage::User { content: "任务".into(), images: None, timestamp: None };
        store.append_message("sub_a", &msg).await.unwrap();
        store.append_message("sub_a", &msg).await.unwrap();
        store.append_message("sub_b", &msg).await.unwrap();

        assert_eq!(store.load_messages("sub_a").await.unwrap().len(), 2);
        assert_eq!(store.load_messages("sub_b").await.unwrap().len(), 1);
        assert_eq!(store.load_messages("never").await.unwrap().len(), 0, "未知会话返回空而不是报错");
        assert_eq!(store.message_count("sub_a"), 2);
    }

    #[tokio::test]
    async fn test_readonly_gate_denies_write_tools_by_policy() {
        let gate = ReadonlyEnforcingGate::new(true);
        assert_eq!(gate.direction(), FailDirection::Closed);

        // 只读工具不问
        assert!(!gate
            .needs_approval("read_file", &ApprovalPolicy::Never, &serde_json::json!({}))
            .await);
        // 写工具要问 → 而 decide 直接按策略拒绝
        assert!(gate
            .needs_approval("write_file", &ApprovalPolicy::Never, &serde_json::json!({}))
            .await);

        let outcome = gate
            .decide(
                ApprovalRequest {
                    call_id: "c1".into(),
                    thread_id: "t1".into(),
                    tool: "write_file".into(),
                    args: serde_json::json!({}),
                    is_write: true,
                    reason: None,
                },
                Some(&NeverCancel),
            )
            .await;
        assert!(!outcome.approved);
        assert_eq!(outcome.by, AnsweredBy::Policy, "来路必须是 Policy（子智能体没有用户可问）");
        let text = denial_text("write_file", &outcome);
        assert!(text.contains("只读子智能体"), "{text}");
    }

    #[tokio::test]
    async fn test_rw_gate_never_asks() {
        let gate = ReadonlyEnforcingGate::new(false);
        assert!(!gate
            .needs_approval("write_file", &ApprovalPolicy::Never, &serde_json::json!({}))
            .await);
        assert!(!gate
            .needs_approval("run_command", &ApprovalPolicy::Never, &serde_json::json!({}))
            .await);
    }
}
