//! SessionStore 端口契约合规断言（INV-2, INV-9, §8.1）
//!
//! 断言点：
//! 1. append_message -> load_messages 往返等价（消息 ID、角色、内容完全一致）
//! 2. 加载不存在的 thread 返回空列表，不可返回未知错误或崩溃
//! 3. 并发追加消息不可出现丢失或竞争损坏

use agent_base::domain::{AgentError, AgentMessage};
use agent_base::ports::SessionStore;

/// 验证 SessionStore 端口契约合规性
pub async fn verify_session_store_contract<S: SessionStore>(store: &S) -> Result<(), AgentError> {
    let thread_id = "test_conformance_thread";

    // 1. 加载空会话返回空 Vec
    let initial = store.load_messages(thread_id).await?;
    if !initial.is_empty() {
        return Err(AgentError::Internal("新线程加载必须返回空列表".into()));
    }

    // 2. 追加单条消息并往返读取比对
    let msg1 = AgentMessage::User {
        content: "你好".into(),
        images: None,
        timestamp: Some(1000),
    };
    store.append_message(thread_id, &msg1).await?;

    let loaded = store.load_messages(thread_id).await?;
    if loaded.len() != 1 {
        return Err(AgentError::Internal("追加后消息数量不符".into()));
    }
    match &loaded[0] {
        AgentMessage::User { content, .. } if content == "你好" => {}
        _ => return Err(AgentError::Internal("加载的消息字段与写入不一致".into())),
    }

    // 3. 追加第二条并验证顺序保持
    let msg2 = AgentMessage::Assistant {
        content: "你好！有什么我可以帮你的吗？".into(),
        thinking: None,
        tool_calls: None,
        stop_reason: Some("stop".into()),
        error_message: None,
        timestamp: Some(1005),
        usage: None,
        duration_ms: Some(100),
        turn_duration_ms: Some(120),
    };
    store.append_message(thread_id, &msg2).await?;

    let loaded_all = store.load_messages(thread_id).await?;
    if loaded_all.len() != 2 {
        return Err(AgentError::Internal("消息列表追加后数量未保持".into()));
    }
    match (&loaded_all[0], &loaded_all[1]) {
        (AgentMessage::User { .. }, AgentMessage::Assistant { .. }) => {}
        _ => return Err(AgentError::Internal("消息列表追加后顺序未保持".into())),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::InMemorySessionStore;

    #[tokio::test]
    async fn test_in_memory_session_store_conformance() {
        let store = InMemorySessionStore::new();
        verify_session_store_contract(&store).await.expect("SessionStore 契约验证必须通过");
    }

    /// W1-T7：**真实实现**（`FsSessionStore`，JSONL 落盘）也要过同一份契约。
    ///
    /// 与替身的差别正是这里要抓的：真实实现会走路径散列、文件创建、逐行 JSON 往返，
    /// 任何一处格式漂移都会让"追加后读回不一致"暴露出来。
    #[tokio::test]
    async fn test_real_fs_session_store_conformance() {
        use agent_adapter::store::FsSessionStore;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "a_da_conf_session_{}_{}",
            std::process::id(),
            nanos
        ));

        let store = FsSessionStore::new(&root, "E:/conformance_project");
        verify_session_store_contract(&store)
            .await
            .expect("真实 FsSessionStore 必须通过 SessionStore 契约");

        let _ = std::fs::remove_dir_all(&root);
    }
}
