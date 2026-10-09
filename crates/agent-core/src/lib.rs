pub mod ai;
pub mod checkpoint;
pub mod plugins;
pub mod protocol;
pub mod runner;
pub mod server;
pub mod session;
pub mod state;
pub mod tools;
pub mod subagents;
pub mod approval;
pub mod skills;

/// 组合根转发：支持新基座装配与产品规格解析
pub use agent_runtime as runtime;

pub use skills::{SkillManager, SkillSummary, get_builtin_skills};

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
        let t1 = store.create_thread(Some("我的测试会话".to_string()), None);
        assert_eq!(store.active_id, t1);
        assert!(store.threads.iter().any(|t| t.id == t1));

        let deleted = store.delete_thread(&t1);
        assert!(deleted);
        assert!(!store.threads.iter().any(|t| t.id == t1));
    }

    /// **W6-T2 出口判据**：`stats.promptChars` 必须返回**实测值**。
    ///
    /// 原先它返回硬编码的 `{systemChars: 1200, toolSpecsChars: 800}`——
    /// 界面拿到的"实测值"其实是两个常量，与当前提示词毫无关系。
    #[tokio::test]
    async fn test_stats_prompt_chars_are_measured_not_fabricated() {
        use agent_base::domain::{
            Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor,
            ToolReceipt,
        };
        use agent_base::engine::{AgentRuntime, RunPolicy};
        use agent_base::ports::ToolCatalog;
        use agent_base::testing::{
            FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, MockScope, MockTool,
            RecordingApprovalGate, ScriptedModelClient,
        };
        use std::sync::Arc;
        use tokio::sync::RwLock;

        const PROMPT: &str = "你是一个测试用系统提示词。";
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "probe": { "type": "string" } }
        });
        let expected_tool_chars = serde_json::to_string(&schema).unwrap().chars().count();

        let tool = Arc::new(MockTool::new(
            ToolDescriptor {
                name: "probe_tool".to_string(),
                summary: "探针工具".to_string(),
                schema: schema.clone(),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolReceipt::success("ok", 0, 1),
        ));
        let catalog: Arc<dyn ToolCatalog> =
            Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

        let rt = Arc::new(AgentRuntime::new(
            Arc::new(ScriptedModelClient::new(vec![])),
            catalog,
            Arc::new(RecordingApprovalGate::new(true)),
            Arc::new(InMemorySessionStore::new()),
            Arc::new(FixedPrompt::new(PROMPT)),
            Arc::new(MockScope::new("s")),
            Arc::new(FixedClock::new(1000)),
            RunPolicy::default(),
        ));

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let dispatcher = Dispatcher::new(
            store,
            Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(subagents::SubagentManager::new()),
            Arc::new(approval::ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        )
        .with_engine(rt);

        let res = dispatcher
            .dispatch("stats.promptChars", serde_json::json!({}))
            .await
            .expect("stats.promptChars 应可分发");

        let system_chars = res["systemChars"].as_u64().expect("systemChars") as usize;
        let tool_chars = res["toolSpecsChars"].as_u64().expect("toolSpecsChars") as usize;
        let total_chars = res["totalChars"].as_u64().expect("totalChars") as usize;

        assert_eq!(
            system_chars,
            PROMPT.chars().count(),
            "systemChars 必须是**实测**的系统提示词长度"
        );
        assert_ne!(system_chars, 1200, "不得再返回编造的常量");
        assert_eq!(
            tool_chars, expected_tool_chars,
            "toolSpecsChars 必须等于描述符 schema 的字符数之和"
        );
        assert_ne!(tool_chars, 800, "不得再返回编造的常量");
        assert_eq!(total_chars, system_chars + tool_chars);
    }

    /// 未装配引擎时**必须报错**，而不是退回编造的数字。
    #[tokio::test]
    async fn test_stats_prompt_chars_without_engine_is_an_error() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let dispatcher = Dispatcher::new(
            store,
            Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
            Arc::new(subagents::SubagentManager::new()),
            Arc::new(approval::ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        );

        let err = dispatcher
            .dispatch("stats.promptChars", serde_json::json!({}))
            .await;
        assert!(
            err.is_err(),
            "没有引擎就没有提示词可测——必须如实报错，不许编造数字"
        );
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
        let plugin_mgr = Arc::new(PluginManager::new());
        let skill_mgr = Arc::new(SkillManager::new());
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            None,
        );

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

        // 验证 plugin.builtinCatalog
        let catalog = dispatcher.dispatch("plugin.builtinCatalog", serde_json::json!({}))
            .await
            .expect("plugin.builtinCatalog 分发失败");
        assert!(catalog.is_array());
        // 目录必须与注册表**逐名相等**（第 6 张名单已消除，W2-T5）。
        // 这里刻意**不写死条数**——写死条数正是"第二张名单"的温床：
        // 加了工具忘记改这里，测试就会红在错误的地方（或更糟：悄悄放过）。
        assert_eq!(
            catalog.as_array().unwrap().len(),
            agent_toolkit::registry::standard_tool_descriptors().len(),
            "内置工具目录条数必须等于注册表描述符数"
        );

        // 验证 plugin.list
        let plugins = dispatcher.dispatch("plugin.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("plugin.list 分发失败");
        assert!(plugins.get("plugins").is_some());
        let plugins_arr = plugins.get("plugins").unwrap().as_array().unwrap();
        assert!(plugins_arr.len() >= 9);
        assert!(plugins_arr.iter().any(|p| p.get("id").and_then(|v| v.as_str()) == Some("builtin:git-tools")));
        assert!(plugins.get("capabilities").is_some());

        // 验证 skill.list
        let skills = dispatcher.dispatch("skill.list", serde_json::json!({ "workspace": "E:/test" }))
            .await
            .expect("skill.list 分发失败");
        assert!(skills.is_array());
        let skills_arr = skills.as_array().unwrap();
        assert!(skills_arr.len() >= 5);
        assert!(skills_arr.iter().any(|s| s.get("name").and_then(|v| v.as_str()) == Some("vibe-coding")));

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

    #[tokio::test]
    async fn test_dispatcher_mode_switching() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
        let subagent_mgr = Arc::new(subagents::SubagentManager::new());
        let approval_mgr = Arc::new(approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(PluginManager::new());
        let skill_mgr = Arc::new(SkillManager::new());
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            None,
        );

        // 初始状态默认为 Code 模式
        assert_eq!(store.read().await.config.mode, AgentMode::Code);

        // 1. 切换到 Create 创造模式
        dispatcher
            .dispatch("thread.setMode", serde_json::json!({ "mode": "create" }))
            .await
            .expect("切换到 create 模式失败");
        assert_eq!(store.read().await.config.mode, AgentMode::Create);
        let active_id = store.read().await.active_id.clone();
        let current_thread = store.read().await.threads.iter().find(|t| t.id == active_id).cloned().unwrap();
        assert_eq!(current_thread.mode, Some(AgentMode::Create));

        // 2. 切换到 Plan 规划模式
        dispatcher
            .dispatch("thread.setMode", serde_json::json!({ "mode": "plan" }))
            .await
            .expect("切换到 plan 模式失败");
        assert_eq!(store.read().await.config.mode, AgentMode::Plan);

        // 3. 切换回 Code 编码模式
        dispatcher
            .dispatch("thread.setMode", serde_json::json!({ "mode": "code" }))
            .await
            .expect("切换到 code 模式失败");
        assert_eq!(store.read().await.config.mode, AgentMode::Code);

        // 4. 创建新会话并直接指定 create 模式
        let create_res = dispatcher
            .dispatch("thread.create", serde_json::json!({ "mode": "create", "title": "创造测试" }))
            .await
            .expect("带模式创建新会话失败");
        let new_id = create_res.get("threadId").and_then(|v| v.as_str()).unwrap();
        let new_thread = store.read().await.threads.iter().find(|t| t.id == new_id).cloned().unwrap();
        assert_eq!(new_thread.mode, Some(AgentMode::Create));
        assert_eq!(store.read().await.config.mode, AgentMode::Create);
    }

    #[tokio::test]
    async fn test_plugin_and_skill_lifecycle() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/codes/rust_projects/a_da".to_string())));
        let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&test_dir);
        let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
        let subagent_mgr = Arc::new(subagents::SubagentManager::new());
        let approval_mgr = Arc::new(approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(PluginManager::new());
        let skill_mgr = Arc::new(SkillManager::new());
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr.clone(),
            skill_mgr.clone(),
            None,
        );

        // 1. 测试内置技能 5 种全覆盖
        let skills = skill_mgr.scan_skills(None);
        assert!(skills.len() >= 5);
        let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        assert!(skill_names.contains(&"vibe-coding".to_string()));
        assert!(skill_names.contains(&"code-review".to_string()));
        assert!(skill_names.contains(&"git-commit".to_string()));
        assert!(skill_names.contains(&"unit-test".to_string()));
        assert!(skill_names.contains(&"refactor-clean".to_string()));

        // 2. 测试技能启停切换
        let target_skill = &skills[0].id;
        dispatcher.dispatch("skill.setEnabled", serde_json::json!({
            "id": target_skill,
            "enabled": false
        })).await.unwrap();

        let skills_after = skill_mgr.scan_skills(None);
        let disabled_item = skills_after.iter().find(|s| &s.id == target_skill).unwrap();
        assert!(!disabled_item.enabled);

        // 恢复启用
        dispatcher.dispatch("skill.setEnabled", serde_json::json!({
            "id": target_skill,
            "enabled": true
        })).await.unwrap();

        // 3. 测试 9 大内置插件
        let plugins = plugin_mgr.scan_plugins(None);
        assert!(plugins.len() >= 9);
        let plugin_ids: Vec<String> = plugins.iter().map(|p| p.id.clone()).collect();
        assert!(plugin_ids.contains(&"builtin:git-tools".to_string()));
        assert!(plugin_ids.contains(&"builtin:code-outline".to_string()));
        assert!(plugin_ids.contains(&"builtin:project-inspector".to_string()));
        assert!(plugin_ids.contains(&"builtin:test-runner".to_string()));
        assert!(plugin_ids.contains(&"builtin:batch-ops".to_string()));
        assert!(plugin_ids.contains(&"builtin:decision".to_string()));
        assert!(plugin_ids.contains(&"builtin:approval-guard".to_string()));
        assert!(plugin_ids.contains(&"builtin:ask-user".to_string()));
        assert!(plugin_ids.contains(&"builtin:ponytail".to_string()));

        // 4. 测试插件能力开关持久化
        dispatcher.dispatch("plugin.capabilities.set", serde_json::json!({
            "patch": {
                "allowSystemPromptReplace": true,
                "hookTimeoutMs": 8888
            }
        })).await.unwrap();

        let caps = plugin_mgr.get_capabilities(None);
        assert!(caps.capabilities.allow_system_prompt_replace);
        assert_eq!(caps.capabilities.hook_timeout_ms, 8888);

        // 清理测试目录
        let _ = std::fs::remove_dir_all(&test_dir);
    }

    /// 会话与工作区严格绑定：指定工作区建会话 → 绑定它、落盘进它；
    /// 切换聚焦 → 全局当前工作区跟着会话走
    #[tokio::test]
    async fn test_thread_create_binds_requested_workspace() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/codes/default_ws".to_string())));
        let test_dir = std::env::temp_dir().join(format!("a_da_ws_bind_{}", uuid::Uuid::new_v4()));
        let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr.clone(),
            checkpoint_mgr,
            Arc::new(subagents::SubagentManager::new()),
            Arc::new(approval::ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        );

        let ws_a = "E:/codes/fpc_projects";
        let ws_b = "E:/codes/rust_projects/a_da/target/release";

        // 1. 指定工作区新建会话
        let created = dispatcher
            .dispatch("thread.create", serde_json::json!({ "workspace": ws_a, "title": "工作区绑定校验" }))
            .await
            .expect("thread.create 分发失败");
        let tid_a = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();

        {
            let s = store.read().await;
            let t = s.threads.iter().find(|t| t.id == tid_a).expect("会话未创建");
            assert_eq!(t.workspace, ws_a, "会话必须绑定到调用方指定的工作区，而不是全局当前工作区");
            assert_eq!(s.workspace.project, ws_a, "新建会话后当前工作区应随之切换");
        }
        assert!(
            session_mgr.get_session_path(ws_a, &tid_a).exists(),
            "会话文件应落在指定工作区的目录下"
        );

        // 2. 换一个工作区再建一个会话，此时全局当前工作区已变成 ws_b
        let created_b = dispatcher
            .dispatch("thread.create", serde_json::json!({ "workspace": ws_b }))
            .await
            .expect("thread.create 分发失败");
        let tid_b = created_b.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();
        assert_eq!(store.read().await.workspace.project, ws_b);

        // 3. 聚焦回第一个会话：全局当前工作区跟着它回到 ws_a
        dispatcher
            .dispatch("thread.focus", serde_json::json!({ "threadId": tid_a }))
            .await
            .expect("thread.focus 分发失败");
        assert_eq!(store.read().await.workspace.project, ws_a);

        // 4. 删除 ws_b 的会话：会话文件按会话自己的工作区定位，能被真正删掉
        dispatcher
            .dispatch("thread.delete", serde_json::json!({ "threadId": tid_b }))
            .await
            .expect("thread.delete 分发失败");
        assert!(
            !session_mgr.get_session_path(ws_b, &tid_b).exists(),
            "删除会话必须落在会话自己的工作区目录，而不是当前工作区"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_dispatcher_provider_management() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&test_dir);
        unsafe {
            std::env::set_var("A_DA_HOME", &test_dir);
        }

        let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
        let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
        let subagent_mgr = Arc::new(subagents::SubagentManager::new());
        let approval_mgr = Arc::new(approval::ApprovalManager::new());
        let plugin_mgr = Arc::new(PluginManager::new());
        let skill_mgr = Arc::new(SkillManager::new());
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            None,
        );

        // 1. 获取列表
        let list_res = dispatcher
            .dispatch("provider.list", serde_json::json!({}))
            .await
            .expect("provider.list 失败");
        let providers = list_res.get("providers").and_then(|v| v.as_array()).unwrap();
        assert!(!providers.is_empty(), "默认应有至少一个初始 provider");

        // 2. 添加 Anthropic 供应商
        let anthropic_provider = serde_json::json!({
            "id": "anthropic-claude",
            "name": "Anthropic Official",
            "protocol": "anthropic",
            "baseUrl": "https://api.anthropic.com",
            "apiKey": "sk-ant-test",
            "models": [
                {
                    "id": "claude-3-5-sonnet-20241022",
                    "name": "Claude 3.5 Sonnet",
                    "contextWindow": 200000,
                    "maxOutputTokens": 8192,
                    "supportsImages": true
                }
            ]
        });

        dispatcher
            .dispatch("provider.save", serde_json::json!({ "provider": anthropic_provider }))
            .await
            .expect("provider.save 失败");

        // 3. 激活新供应商
        dispatcher
            .dispatch("provider.setActive", serde_json::json!({ "id": "anthropic-claude", "model": "claude-3-5-sonnet-20241022" }))
            .await
            .expect("provider.setActive 失败");

        {
            let s = store.read().await;
            assert_eq!(s.active_provider_id, "anthropic-claude");
            assert_eq!(s.provider.protocol, crate::ai::ModelProtocol::Anthropic);
            assert_eq!(s.provider.model, "claude-3-5-sonnet-20241022");
            assert_eq!(s.config.context_window, 200000);
            assert_eq!(s.config.max_output_tokens, Some(8192));
            assert_eq!(s.config.supports_images, true);
        }

        // 4. 快照广播包含供应商数据
        {
            let s = store.read().await;
            let snap = generate_snapshot(&s);
            assert_eq!(snap.active_provider_id, "anthropic-claude");
            assert!(snap.providers.iter().any(|p| p.id == "anthropic-claude"));
        }

        // 5. 删除非激活供应商
        //
        // ⚠️ 刻意**不删** `providers[0]`：共享配置里的供应商数量与身份取决于
        // 同进程内其他用例（`app_home()` 是进程级单例）。当列表只剩 1 个时，
        // `provider.delete` 会按"至少保留一个供应商"的规则拒绝——这条断言曾**偶发红**
        // （单独跑必过、全量跑偶发失败），根因是共享全局态（INV-8），不是被测逻辑错。
        // 自己造一个专用非激活项再删它，测试才与执行顺序和共享状态无关。
        let scratch_id = "scratch-provider-for-delete";
        dispatcher
            .dispatch(
                "provider.save",
                serde_json::json!({
                    "provider": {
                        "id": scratch_id,
                        "name": "Scratch",
                        "protocol": "openai_chat",
                        "baseUrl": "https://example.invalid",
                        "apiKey": "sk-scratch",
                        "models": [{ "id": "scratch-model", "name": "Scratch Model" }]
                    }
                }),
            )
            .await
            .expect("provider.save(scratch) 失败");

        dispatcher
            .dispatch("provider.delete", serde_json::json!({ "id": scratch_id }))
            .await
            .expect("provider.delete 失败");

        {
            let s = store.read().await;
            assert!(
                !s.providers.iter().any(|p| p.id == scratch_id),
                "专用供应商应已被删除"
            );
        }

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_conversation_queue_lifecycle() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let store = Arc::new(RwLock::new(AgentStore::new("E:/codes/rust_projects/a_da".to_string())));
        let test_dir = std::env::temp_dir().join(format!("a_da_queue_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&test_dir);
        let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
        let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
        let dispatcher = Dispatcher::new(
            store.clone(),
            session_mgr.clone(),
            checkpoint_mgr,
            Arc::new(subagents::SubagentManager::new()),
            Arc::new(approval::ApprovalManager::new()),
            Arc::new(PluginManager::new()),
            Arc::new(SkillManager::new()),
            None,
        );

        // 1. 创建测试会话
        let created = dispatcher
            .dispatch("thread.create", serde_json::json!({ "title": "队列排队测试" }))
            .await
            .unwrap();
        let tid = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();

        // 2. 模拟该会话当前处于运行态
        {
            let mut s = store.write().await;
            s.set_thread_running(&tid, true);
        }

        // 3. 在运行中发送第 1 条后续指令，预期进入队列
        let res1 = dispatcher
            .dispatch("thread.send", serde_json::json!({ "threadId": tid, "text": "第 1 条后续指令" }))
            .await
            .unwrap();
        assert_eq!(res1.get("queued").and_then(|v| v.as_bool()), Some(true));

        // 4. 在运行中发送第 2 条后续指令，预期进入队尾
        let res2 = dispatcher
            .dispatch("thread.send", serde_json::json!({ "threadId": tid, "text": "第 2 条后续指令" }))
            .await
            .unwrap();
        assert_eq!(res2.get("queued").and_then(|v| v.as_bool()), Some(true));

        {
            let s = store.read().await;
            assert_eq!(s.queue.len(), 2);
            assert_eq!(s.queue[0].text, "第 1 条后续指令");
            assert_eq!(s.queue[1].text, "第 2 条后续指令");
        }

        // 5. 插队：调用 queue.promote 将第 2 条（索引 1）提升至队首
        dispatcher
            .dispatch("queue.promote", serde_json::json!({ "index": 1, "threadId": tid }))
            .await
            .unwrap();

        {
            let s = store.read().await;
            assert_eq!(s.queue.len(), 2);
            assert_eq!(s.queue[0].text, "第 2 条后续指令");
            assert_eq!(s.queue[1].text, "第 1 条后续指令");
        }

        // 6. 移出队列：调用 queue.remove 移除第 1 项
        let removed = dispatcher
            .dispatch("queue.remove", serde_json::json!({ "index": 0, "threadId": tid }))
            .await
            .unwrap();
        assert_eq!(removed.get("text").and_then(|v| v.as_str()), Some("第 2 条后续指令"));

        {
            let s = store.read().await;
            assert_eq!(s.queue.len(), 1);
            assert_eq!(s.queue[0].text, "第 1 条后续指令");
        }

        // 7. 用户中止会话：调用 thread.abort，预期队列中属于该会话的条目被全部清空
        dispatcher
            .dispatch("thread.abort", serde_json::json!({ "threadId": tid }))
            .await
            .unwrap();

        {
            let s = store.read().await;
            assert_eq!(s.queue.len(), 0);
            assert!(!s.is_thread_running(&tid));
        }

        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
