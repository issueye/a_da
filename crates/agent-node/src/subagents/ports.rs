//! 子智能体隔离执行所需的**生产**端口实现（W4-T6）。
//!
//! 子智能体并入 `AgentRuntime::run_turn` 后，需要一套"隔离上下文"的端口。
//! 这两个实现都是**生产代码**（不是测试替身——INV-2 要求端口真接线）：
//!
//! | 端口 | 实现 | 为什么这样选 |
//! |---|---|---|
//! | `PromptSource` | [`ProfilePrompt`] | AGENTS.md §3：子智能体**只**拿到 `profile.system_prompt`，不继承主线程的 AGENTS.md / 系统提示词 |
//! | `SessionStore` | [`EphemeralSessionStore`] | 子智能体上下文**刻意是临时的**：它是一次性委派，不该在主会话的历史里留下痕迹（legacy 也是纯内存） |
//!
//! 只读子智能体的工具面在**装配期**按 `Access::ReadOnly` 裁切；核心不再有审批闸门。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use agent_base::domain::{AgentError, AgentMessage};
use agent_base::ports::{BoxFuture, PromptSource, SessionStore};

use crate::session::SessionManager;

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

    /// 子智能体 profile 就是**一块**正文：如实报一段（而不是假报拆不开）。
    fn prompt_segments(&self) -> Vec<agent_base::ports::PromptSegment> {
        vec![agent_base::ports::PromptSegment {
            id: "subagent_profile".to_string(),
            source: "builtin:subagent".to_string(),
            bytes: self.prompt.len(),
        }]
    }
}

/// 子智能体会话存储（纯内存隔离，可附加工作区磁盘持久化以便独立查看）。
#[derive(Default)]
pub struct EphemeralSessionStore {
    sessions: Mutex<HashMap<String, Vec<AgentMessage>>>,
    workspace: Option<PathBuf>,
    session_mgr: Option<SessionManager>,
}

impl EphemeralSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 启用工作区磁盘持久化同步，支持前端独立查看子智能体会话
    pub fn with_persistence(mut self, workspace: PathBuf, session_mgr: SessionManager) -> Self {
        self.workspace = Some(workspace);
        self.session_mgr = Some(session_mgr);
        self
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

            if let (Some(mgr), Some(ws)) = (&self.session_mgr, &self.workspace) {
                let _ = mgr.append_message(thread_id, message.clone(), Some(&ws.to_string_lossy()));
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
