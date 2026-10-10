use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuestionAnswer {
    pub choice: Option<String>,
    pub text: Option<String>,
    pub answered_by: String, // "user" | "aborted"
}

#[derive(Debug, Default)]
pub struct QuestionManager {
    waiters: Mutex<HashMap<String, oneshot::Sender<QuestionAnswer>>>,
}

static GLOBAL_QUESTION_MANAGER: OnceLock<QuestionManager> = OnceLock::new();

/// 初始化向 `agent-toolkit` 提问工具挂载等待器
pub fn init_question_registrar() {
    agent_toolkit::core::ask_user::set_question_registrar(std::sync::Arc::new(|call_id: &str| {
        let rx = global_question_manager().register_waiter(call_id);
        Box::pin(async move {
            match rx.await {
                Ok(ans) => Ok(agent_toolkit::core::ask_user::QuestionAnswerPayload {
                    choice: ans.choice,
                    text: ans.text,
                    answered_by: ans.answered_by,
                }),
                Err(_) => Err("提问通道已关闭或已被中止".to_string()),
            }
        })
    }));
}

pub fn global_question_manager() -> &'static QuestionManager {
    GLOBAL_QUESTION_MANAGER.get_or_init(|| {
        init_question_registrar();
        QuestionManager::new()
    })
}

impl QuestionManager {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// 注册等待前端用户答复的通道
    pub fn register_waiter(&self, call_id: &str) -> oneshot::Receiver<QuestionAnswer> {
        let (tx, rx) = oneshot::channel();
        let mut map = self.waiters.lock().unwrap();
        map.insert(call_id.to_string(), tx);
        rx
    }

    /// 接收到前端 `question.answer` 请求，派发用户答复
    pub fn resolve_answer(&self, call_id: &str, answer: QuestionAnswer) -> bool {
        let mut map = self.waiters.lock().unwrap();
        if let Some(tx) = map.remove(call_id) {
            let _ = tx.send(answer);
            true
        } else {
            false
        }
    }

    /// 检查某个提问是否在等待用户答复
    pub fn has_pending(&self, call_id: &str) -> bool {
        let map = self.waiters.lock().unwrap();
        map.contains_key(call_id)
    }

    /// 取消所有正在等待的提问（例如会话被中止时）
    pub fn cancel_all(&self) {
        let mut map = self.waiters.lock().unwrap();
        for (_, tx) in map.drain() {
            let _ = tx.send(QuestionAnswer {
                choice: None,
                text: None,
                answered_by: "aborted".to_string(),
            });
        }
    }

    /// 取消指定提问
    pub fn cancel(&self, call_id: &str) {
        let mut map = self.waiters.lock().unwrap();
        if let Some(tx) = map.remove(call_id) {
            let _ = tx.send(QuestionAnswer {
                choice: None,
                text: None,
                answered_by: "aborted".to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_question_manager_resolve_and_cancel() {
        let mgr = QuestionManager::new();
        assert!(!mgr.has_pending("call_01"));

        let rx = mgr.register_waiter("call_01");
        assert!(mgr.has_pending("call_01"));

        let resolved = mgr.resolve_answer(
            "call_01",
            QuestionAnswer {
                choice: Some("c1".to_string()),
                text: Some("使用方案 A".to_string()),
                answered_by: "user".to_string(),
            },
        );
        assert!(resolved);
        assert!(!mgr.has_pending("call_01"));

        let ans = rx.await.expect("应该收到答案");
        assert_eq!(ans.choice.as_deref(), Some("c1"));
        assert_eq!(ans.text.as_deref(), Some("使用方案 A"));
        assert_eq!(ans.answered_by, "user");

        // 测试取消
        let rx2 = mgr.register_waiter("call_02");
        assert!(mgr.has_pending("call_02"));
        mgr.cancel("call_02");
        assert!(!mgr.has_pending("call_02"));
        let ans2 = rx2.await.expect("应该收到取消消息");
        assert_eq!(ans2.answered_by, "aborted");
    }
}

