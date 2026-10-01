pub mod ai;
pub mod checkpoint;
pub mod plugins;
pub mod protocol;
pub mod runner;
pub mod server;
pub mod session;
pub mod state;
pub mod tools;
pub mod compiler;
pub mod kernel;
pub mod hermes_host;
pub mod subagents;
pub mod approval;
pub mod desktop_ui;
pub mod native_ws;

pub use hermes_host::HermesHost;
pub use desktop_ui::run_desktop_mode;

pub use ai::{
    stream_model_chat, ChatCompletionMessage, ChatCompletionTool, ChatCompletionToolFunction,
    ModelChatOptions, ProviderConfig, StreamDelta, ThinkFilterPart, ThinkTagFilter, TokenUsage,
    ToolCallInfo,
};
pub use plugins::{
    LoadedPlugin, PluginManager, PluginManifest, PluginSandbox, PluginScope, PluginToolDeclaration,
};
pub use runner::*;





pub use checkpoint::{
    CheckpointEntry, CheckpointFile, CheckpointManager, CheckpointRecord, RevertOutcome,
    RevertRecord,
};
pub use protocol::*;
pub use server::*;
pub use session::{
    get_app_home, safe_id, workspace_slug, AgentMessage, SessionCompactEntry, SessionEntry,
    SessionHeader, SessionManager, SessionMessageEntry, SessionNoticeEntry, SessionSummary,
};
pub use state::*;
pub use tools::*;





#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_snapshot_serialization() {
        let store = AgentStore::new("E:/codes/rust_projects/a_da".to_string());
        let snapshot = generate_snapshot(&store);
        let json = serde_json::to_string(&snapshot).expect("序列化快照失败");
        assert!(json.contains("activeThreadId"));
        assert!(json.contains("runningThreadIds"));
    }

    #[test]
    fn test_fs_roots() {
        let extra = vec!["E:/codes/rust_projects/a_da".to_string()];
        let roots = fs_service::list_roots(&extra);
        assert!(!roots.is_empty());
        assert!(roots.iter().any(|r| r.kind == "workspace" || r.kind == "home" || r.kind == "drive"));
    }

    #[test]
    fn test_thread_lifecycle() {
        let mut store = AgentStore::new("E:/test".to_string());
        let t1 = store.create_thread(Some("我的测试会话".to_string()));
        assert_eq!(store.active_id, t1);
        assert!(store.threads.iter().any(|t| t.id == t1));

        let deleted = store.delete_thread(&t1);
        assert!(deleted);
        assert!(!store.threads.iter().any(|t| t.id == t1));
    }

    #[tokio::test]
    async fn test_dispatcher_extension_methods() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let session_mgr = Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home"))));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home"))));
        let subagent_mgr = Arc::new(subagents::SubagentManager::new());
        let approval_mgr = Arc::new(approval::ApprovalManager::new());
        let dispatcher = Dispatcher::new(store.clone(), session_mgr, checkpoint_mgr, subagent_mgr, approval_mgr, None);

        // 验证 approval.decide
        let decide_res = dispatcher.dispatch("approval.decide", serde_json::json!({
            "toolItemId": "item_123",
            "approved": true
        }))
        .await
        .expect("approval.decide 分发失败");
        assert_eq!(decide_res.get("toolItemId").and_then(|v| v.as_str()), Some("item_123"));
        assert_eq!(decide_res.get("approved").and_then(|v| v.as_bool()), Some(true));

        // 验证 subagentProfile.list
        let subagents = dispatcher.dispatch("subagentProfile.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("subagentProfile.list 分发失败");
        assert!(subagents.is_array());
        let subagents_arr = subagents.as_array().unwrap();
        assert!(subagents_arr.len() >= 4);
        assert!(subagents_arr.iter().any(|s| s.get("id").and_then(|v| v.as_str()) == Some("researcher")));

        // 验证 prompt.list
        let prompts = dispatcher.dispatch("prompt.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("prompt.list 分发失败");
        assert!(prompts.is_array());
        assert!(!prompts.as_array().unwrap().is_empty());

        // 验证 plugin.list
        let plugins = dispatcher.dispatch("plugin.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("plugin.list 分发失败");
        assert!(plugins.get("plugins").is_some());
        assert!(plugins.get("capabilities").is_some());

        // 验证 skill.list
        let skills = dispatcher.dispatch("skill.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("skill.list 分发失败");
        assert!(skills.is_array());

        // 验证 subagentProfile.list
        let profiles = dispatcher.dispatch("subagentProfile.list", serde_json::json!({}))
            .await
            .expect("subagentProfile.list 分发失败");
        assert!(profiles.is_array());

        // 验证 debug.trace
        let trace = dispatcher.dispatch("debug.trace", serde_json::json!({ "text": "测试跟踪信息" }))
            .await
            .expect("debug.trace 分发失败");
        assert_eq!(trace, serde_json::Value::Null);

        // 验证 thread.compact
        let active_id = store.read().await.active_id.clone();
        let compact_res = dispatcher.dispatch("thread.compact", serde_json::json!({ "threadId": active_id }))
            .await
            .expect("thread.compact 分发失败");
        assert!(compact_res.get("success").is_some());

        // 验证 change.count
        let change_cnt = dispatcher.dispatch("change.count", serde_json::json!({ "threadId": active_id }))
            .await
            .expect("change.count 分发失败");
        assert!(change_cnt.get("count").is_some());
    }
}
