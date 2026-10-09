//! `FsSessionStore`：`SessionStore` 端口的真实实现（JSONL 落盘）。
//!
//! 设计口径（`docs/agent-base-design.md` §9：`store-fs` 属适配器；
//! `docs/agent-base-wiring-plan.md` §5 W1-T2b）：
//!
//! - 端口的签名只有 `thread_id`（[`agent_base::ports::SessionStore`]），而落盘布局是
//!   `sessions/<workspace 散列>/<session id>.jsonl`。这层映射**在适配器内部解决**：
//!   先查内存索引，再扫各工作区目录；新会话落在构造时给定的工作区目录下。
//! - 行级读写与折叠语义走 [`super::jsonl`]，与 `agent-core` 的 `SessionManager` **同源**（R2）。
//! - 时间取自 `Clock` 端口（[`crate::clock::SystemClock`]）——适配器是全仓唯一读系统时间的地方。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_base::domain::{AgentError, AgentMessage};
use agent_base::ports::{BoxFuture, Clock, SessionStore};

use super::jsonl;
use super::slug::{safe_id, workspace_slug};

pub struct FsSessionStore {
    /// 会话持久化根目录（通常是 `~/.a-da/sessions`）
    sessions_root: PathBuf,
    /// 新会话默认落到哪个工作区的目录下
    workspace: String,
    /// `thread_id` → 会话文件路径的缓存（避免每次都扫盘）
    index: Mutex<HashMap<String, PathBuf>>,
}

impl FsSessionStore {
    pub fn new(sessions_root: impl Into<PathBuf>, workspace: impl Into<String>) -> Self {
        Self {
            sessions_root: sessions_root.into(),
            workspace: workspace.into(),
            index: Mutex::new(HashMap::new()),
        }
    }

    pub fn sessions_root(&self) -> &Path {
        &self.sessions_root
    }

    /// 新会话的落点：`<root>/<workspace 散列>/<safe thread_id>.jsonl`
    fn default_path(&self, thread_id: &str) -> PathBuf {
        self.sessions_root
            .join(workspace_slug(&self.workspace))
            .join(format!("{}.jsonl", safe_id(thread_id)))
    }

    /// 解析已有会话文件：缓存 → 扫盘。
    fn resolve(&self, thread_id: &str) -> Option<PathBuf> {
        if let Some(p) = self.index.lock().expect("会话索引锁中毒").get(thread_id) {
            return Some(p.clone());
        }

        let file_name = format!("{}.jsonl", safe_id(thread_id));
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(&self.sessions_root) {
            for e in entries.flatten() {
                let dir = e.path();
                if !dir.is_dir() {
                    continue;
                }
                let candidate = dir.join(&file_name);
                if candidate.is_file() {
                    found = Some(candidate);
                    break;
                }
            }
        }

        if let Some(p) = &found {
            self.index
                .lock()
                .expect("会话索引锁中毒")
                .insert(thread_id.to_string(), p.clone());
        }
        found
    }

    /// 写入时要用的路径：已有就复用，没有就用默认落点。
    fn write_path(&self, thread_id: &str) -> PathBuf {
        let path = self
            .resolve(thread_id)
            .unwrap_or_else(|| self.default_path(thread_id));
        self.index
            .lock()
            .expect("会话索引锁中毒")
            .insert(thread_id.to_string(), path.clone());
        path
    }
}

impl SessionStore for FsSessionStore {
    fn load_messages<'a>(
        &'a self,
        thread_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AgentMessage>, AgentError>> {
        Box::pin(async move {
            let Some(path) = self.resolve(thread_id) else {
                // 端口契约：会话不存在返回**空列表**（不是错误）
                return Ok(Vec::new());
            };
            let outcome = jsonl::read_entries(&path)
                .map_err(|e| AgentError::Store(format!("读取会话失败: {e}")))?;
            if outcome.skipped_lines > 0 {
                // 坏行不致命，但必须可观测——静默跳过就是"看起来读到了其实少了东西"
                tracing::warn!(
                    "会话 {} 有 {} 行无法解析，已跳过（文件：{}）",
                    thread_id,
                    outcome.skipped_lines,
                    path.display()
                );
            }
            let (_, messages) = jsonl::fold_messages(&outcome.entries);
            Ok(messages)
        })
    }

    fn append_message<'a>(
        &'a self,
        thread_id: &'a str,
        message: &'a AgentMessage,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            let path = self.write_path(thread_id);
            let entry =
                jsonl::new_message_entry(crate::clock::SystemClock.now_ms(), message.clone());
            jsonl::append_message(&path, &entry)
                .map_err(|e| AgentError::Store(format!("追加会话消息失败: {e}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("a_da_fs_store_{tag}_{}_{}", std::process::id(), nanos))
    }

    fn user(text: &str) -> AgentMessage {
        AgentMessage::User {
            content: text.to_string(),
            images: None,
            timestamp: Some(1),
        }
    }

    #[tokio::test]
    async fn test_append_then_load_roundtrip() {
        let root = tmp_root("roundtrip");
        let store = FsSessionStore::new(&root, "E:/proj");

        store.append_message("thread_a", &user("你好")).await.expect("追加失败");
        store
            .append_message(
                "thread_a",
                &AgentMessage::Assistant {
                    content: "你好，有什么可以帮你".to_string(),
                    thinking: None,
                    tool_calls: None,
                    stop_reason: Some("stop".to_string()),
                    error_message: None,
                    timestamp: Some(2),
                    usage: None,
                    duration_ms: None,
                    turn_duration_ms: None,
                },
            )
            .await
            .expect("追加失败");

        let loaded = store.load_messages("thread_a").await.expect("加载失败");
        assert_eq!(loaded.len(), 2, "两条消息都应读回");
        assert!(matches!(&loaded[0], AgentMessage::User { content, .. } if content == "你好"));
        assert!(matches!(&loaded[1], AgentMessage::Assistant { content, .. } if content.contains("有什么可以帮你")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn test_missing_session_returns_empty_not_error() {
        let root = tmp_root("missing");
        let store = FsSessionStore::new(&root, "E:/proj");
        let loaded = store.load_messages("nope").await.expect("不存在不应是错误");
        assert!(loaded.is_empty(), "端口契约要求返回空列表");
    }

    /// 会话 id 里带非法字符时必须被清洗后落盘（防路径穿越）。
    #[tokio::test]
    async fn test_unsafe_thread_id_is_sanitized() {
        let root = tmp_root("unsafe");
        let store = FsSessionStore::new(&root, "E:/proj");
        store
            .append_message("thread:../../evil", &user("x"))
            .await
            .expect("追加失败");

        // 落盘文件必须还在 root 之内
        let mut found = Vec::new();
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            if e.path().is_dir() {
                for f in std::fs::read_dir(e.path()).unwrap().flatten() {
                    found.push(f.file_name().to_string_lossy().to_string());
                }
            }
        }
        assert_eq!(found.len(), 1, "应恰好一个会话文件");
        assert_eq!(found[0], "thread_______evil.jsonl", "非法字符必须被清洗: {found:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 换一个 store 实例（模拟重启）也要能找到已落盘的会话。
    #[tokio::test]
    async fn test_reopen_finds_existing_session_without_index() {
        let root = tmp_root("reopen");
        {
            let store = FsSessionStore::new(&root, "E:/proj");
            store.append_message("thread_x", &user("重启前")).await.expect("追加失败");
        }
        let reopened = FsSessionStore::new(&root, "E:/proj");
        let loaded = reopened.load_messages("thread_x").await.expect("加载失败");
        assert_eq!(loaded.len(), 1, "新实例必须能靠扫盘找到旧会话");
        assert!(matches!(&loaded[0], AgentMessage::User { content, .. } if content == "重启前"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 追加走的是同一套折叠语义：compact 之后上下文被收敛。
    #[tokio::test]
    async fn test_compact_semantics_shared_with_manager() {
        let root = tmp_root("compact");
        let store = FsSessionStore::new(&root, "E:/proj");
        store.append_message("t", &user("旧上下文")).await.expect("追加失败");

        let path = store.write_path("t");
        jsonl::append_compact(
            &path,
            &super::super::types::SessionCompactEntry {
                entry_type: "compact".to_string(),
                id: "c1".to_string(),
                timestamp: 5,
                summary: "摘要".to_string(),
                pre_tokens: 1,
                post_tokens: 1,
                saved_tokens: 0,
                turns_summarized: 1,
                custom_instructions: None,
            },
        )
        .expect("追加失败");

        let loaded = store.load_messages("t").await.expect("加载失败");
        assert_eq!(loaded.len(), 1, "compact 之后应只剩续接消息");
        assert!(matches!(&loaded[0], AgentMessage::User { content, .. } if content.contains("摘要")));

        let _ = std::fs::remove_dir_all(&root);
    }
}
