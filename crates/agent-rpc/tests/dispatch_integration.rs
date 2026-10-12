use std::sync::Arc;
use tokio::sync::RwLock;

use agent_node::approval::ApprovalManager;
use agent_node::checkpoint::CheckpointManager;
use agent_node::plugins::PluginManager;
use agent_node::session::SessionManager;
use agent_node::skills::SkillManager;
use agent_node::subagents::SubagentManager;
use agent_proto::*;
use agent_rpc::server::dispatch::Dispatcher;
use agent_rpc::server::fs_service;
use agent_rpc::state::{generate_snapshot, AgentStore};

/// 造一个**注入式应用目录**的 store —— 集成测试绝不碰用户真实的 `~/.a-da`。
///
/// 🔴 为什么不能写 `AgentStore::new(..)` + `std::env::set_var("A_DA_HOME", ..)`：
/// 应用目录端口是 `OnceLock`，同一测试进程里只要**任何一个**用例先构造过 `AgentStore::new`，
/// 端口就已经钉死在真实用户目录上，后面的 `set_var` 完全无效（`port 是 OnceLock，初始化后
/// 改环境变量无效`——正是 AGENTS 里记着的那条）。于是 `provider.save` / `provider.delete`
/// 这些会落盘的 dispatch 动作直接**覆盖用户真实的 `~/.a-da/config.json`**。
/// 2026-10-12 真事故：跑一次 `cargo test -p agent-rpc`，用户配置里的激活供应商被改成了
/// 测试用例里的 anthropic-claude，顶层 baseUrl/apiKey/model 一起被改。
fn store_for_test(workspace: &str) -> AgentStore {
    let dir = std::env::temp_dir().join(format!("a_da_it_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    AgentStore::with_home(
        workspace.to_string(),
        &agent_base::testing::TempAppHome::at(&dir),
    )
}

/// 造一个一次性的临时应用目录（给 plugin/skill 管理器用）。
///
/// 同样是为了**不碰用户真实的 `~/.a-da`**：`PluginManager`/`SkillManager` 会往
/// `config.json` / `skills_state.json` 里写东西。
fn test_home_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("a_da_it_home_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn test_protocol_snapshot_serialization() {
    let store = store_for_test("E:/codes/rust_projects/a_da");
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
    let mut store = store_for_test("E:/test");
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

    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
        Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
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
    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
        Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home")))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
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
    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let session_mgr = Arc::new(SessionManager::new(Some(std::env::temp_dir().join("a_da_test_home"))));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(std::env::temp_dir().join("a_da_test_home"))));
    let subagent_mgr = Arc::new(SubagentManager::new());
    let approval_mgr = Arc::new(ApprovalManager::new());
    let plugin_mgr = Arc::new(PluginManager::with_home(test_home_dir()));
    let skill_mgr = Arc::new(SkillManager::with_home(test_home_dir()));
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
    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
    let subagent_mgr = Arc::new(SubagentManager::new());
    let approval_mgr = Arc::new(ApprovalManager::new());
    let plugin_mgr = Arc::new(PluginManager::with_home(test_home_dir()));
    let skill_mgr = Arc::new(SkillManager::with_home(test_home_dir()));
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
    assert_eq!(new_thread.agent_id, Some("ada-coding".to_string()));
    assert_eq!(store.read().await.config.mode, AgentMode::Create);

    // 5. 新建 PM 模式会话：**产品仍是 ada-coding**（PM 已降级为模式，不再是产品）
    let create_pm_res = dispatcher
        .dispatch("thread.create", serde_json::json!({ "mode": "pm", "title": "项目管理测试" }))
        .await
        .expect("新建 PM 会话失败");
    let pm_id = create_pm_res.get("threadId").and_then(|v| v.as_str()).unwrap();
    let pm_thread = store.read().await.threads.iter().find(|t| t.id == pm_id).cloned().unwrap();
    assert_eq!(pm_thread.mode, Some(AgentMode::Pm));
    assert_eq!(
        pm_thread.agent_id,
        Some("ada-coding".to_string()),
        "PM 是**模式**：产品身份必须仍是 ada-coding（早先这里写成 ada-pm，正是双身份的账）"
    );

    // 6. 切换既有会话模式至 PM：模式变、**产品身份不变**
    dispatcher
        .dispatch("thread.setMode", serde_json::json!({ "threadId": new_id, "mode": "pm" }))
        .await
        .expect("切换会话为 pm 失败");
    let switched_thread = store.read().await.threads.iter().find(|t| t.id == new_id).cloned().unwrap();
    assert_eq!(switched_thread.mode, Some(AgentMode::Pm));
    assert_eq!(
        switched_thread.agent_id,
        Some("ada-coding".to_string()),
        "切到 PM 模式不得改产品身份"
    );

    // 7. 切回 code 模式，联动恢复为 ada-coding
    dispatcher
        .dispatch("thread.setMode", serde_json::json!({ "threadId": new_id, "mode": "code" }))
        .await
        .expect("切换会话为 code 失败");
    let restored_thread = store.read().await.threads.iter().find(|t| t.id == new_id).cloned().unwrap();
    assert_eq!(restored_thread.mode, Some(AgentMode::Code));
    assert_eq!(restored_thread.agent_id, Some("ada-coding".to_string()));
}

#[tokio::test]
async fn test_plugin_and_skill_lifecycle() {
    let store = Arc::new(RwLock::new(store_for_test("E:/codes/rust_projects/a_da")));
    let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
    let subagent_mgr = Arc::new(SubagentManager::new());
    let approval_mgr = Arc::new(ApprovalManager::new());
    let plugin_mgr = Arc::new(PluginManager::with_home(test_home_dir()));
    let skill_mgr = Arc::new(SkillManager::with_home(test_home_dir()));
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

#[tokio::test]
async fn test_thread_create_binds_requested_workspace() {
    let store = Arc::new(RwLock::new(store_for_test("E:/codes/default_ws")));
    let test_dir = std::env::temp_dir().join(format!("a_da_ws_bind_{}", uuid::Uuid::new_v4()));
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
    let dispatcher = Dispatcher::new(
        store.clone(),
        session_mgr.clone(),
        checkpoint_mgr,
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
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
    let test_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);

    // 🔴 用注入式应用目录：provider.save/delete 会**落盘**，绝不能让它们写到
    // 用户真实的 `~/.a-da/config.json`（`set_var("A_DA_HOME")` 在 OnceLock 之后无效）
    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
    let subagent_mgr = Arc::new(SubagentManager::new());
    let approval_mgr = Arc::new(ApprovalManager::new());
    let plugin_mgr = Arc::new(PluginManager::with_home(test_home_dir()));
    let skill_mgr = Arc::new(SkillManager::with_home(test_home_dir()));
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
        assert_eq!(s.provider.protocol, agent_base::model::ModelProtocol::Anthropic);
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
    let store = Arc::new(RwLock::new(store_for_test("E:/codes/rust_projects/a_da")));
    let test_dir = std::env::temp_dir().join(format!("a_da_queue_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let checkpoint_mgr = Arc::new(CheckpointManager::new(Some(test_dir.clone())));
    let dispatcher = Dispatcher::new(
        store.clone(),
        session_mgr.clone(),
        checkpoint_mgr,
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
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

/// 设置项「请求重试次数」的端到端契约：**改得了、夹得住、真的落盘**。
///
/// 用注入式应用目录（`AgentStore::with_home`）而不是 `set_var("A_DA_HOME")`：
/// 端口是 `OnceLock`，初始化之后再改环境变量无效，而且会写进用户真实的 `~/.a-da`。
#[tokio::test]
async fn test_config_set_retry_count_is_clamped_and_persisted() {
    use agent_base::testing::TempAppHome;

    let test_dir = std::env::temp_dir().join(format!("a_da_retry_cfg_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let cfg_path = test_dir.join("config.json");
    let home = TempAppHome::at(&test_dir).with_config_file(&cfg_path);

    let store = Arc::new(RwLock::new(AgentStore::with_home("E:/test".to_string(), &home)));
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(test_dir.clone()))),
        Arc::new(CheckpointManager::new(Some(test_dir.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
        None,
    );

    // 1. 默认值：没配过就是 5（绝不是 0——0 意味着一次都不重试）
    assert_eq!(
        store.read().await.config.retry_count,
        agent_base::model::DEFAULT_MODEL_RETRY_COUNT as u32
    );

    // 2. 设置 7 次
    let res = dispatcher
        .dispatch("config.setRetryCount", serde_json::json!({ "retries": 7 }))
        .await
        .expect("config.setRetryCount 必须可分发");
    assert_eq!(res.get("retryCount").and_then(|v| v.as_u64()), Some(7));
    assert_eq!(store.read().await.config.retry_count, 7, "内存态必须立即生效");

    // 3. 真的落盘（配置文件里能读到 7）
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("配置必须已落盘")).unwrap();
    assert_eq!(
        written.get("retryCount").and_then(|v| v.as_u64()),
        Some(7),
        "重试次数必须落盘：重启后设置不能丢"
    );

    // 4. 越界值夹到上限；缺参数按非法参数处理（不能静默当成 0）
    let clamped = dispatcher
        .dispatch("config.setRetryCount", serde_json::json!({ "retries": 9999 }))
        .await
        .unwrap();
    assert_eq!(
        clamped.get("retryCount").and_then(|v| v.as_u64()),
        Some(agent_base::model::MAX_MODEL_RETRY_COUNT as u64)
    );
    assert!(
        dispatcher
            .dispatch("config.setRetryCount", serde_json::json!({}))
            .await
            .is_err(),
        "缺少 retries 参数必须报错，不能静默当成 0"
    );

    // 5. 0 是合法值（明确表示不重试），不能被当成"没设置"
    let zero = dispatcher
        .dispatch("config.setRetryCount", serde_json::json!({ "retries": 0 }))
        .await
        .unwrap();
    assert_eq!(zero.get("retryCount").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(store.read().await.config.retry_count, 0);

    let _ = std::fs::remove_dir_all(&test_dir);
}

/// 端到端守门：**审批请求必须真的能弹到界面上，并被界面放行**。
///
/// 引擎的事件顺序是 `ApprovalRequested` →（等用户）→ `ToolCallStarted`（建卡，且在审批
/// 通过**之后**）。早先 `store.set_tool_waiting_approval` 只改"已存在的卡片"，于是审批
/// 事件静默 no-op：界面既没有审批卡、`waiting_thread_ids` 也为空——用户无从批准，只能干等
/// 审批超时（默认 30 分钟）被自动拒绝。实测踩到：一条含 `Format-Table` 的**只读**列目录
/// 命令被危险命令子串匹配误判为高危，整轮就此卡死。
///
/// 这条测试跑的是**真链路**：脚本模型 → `AgentRuntime::run_turn` → 事件桥 → dispatch 事件
/// 循环 → `AgentStore`；然后走与前端完全相同的 `approval.decide` 把闸门放行，并断言本轮
/// 继续跑完。闸门接的是**本 dispatcher 的** `ApprovalManager`（生产同构）。
#[tokio::test]
async fn test_approval_request_reaches_store_and_decide_resumes_the_turn() {
    use agent_base::domain::{
        Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolDescriptor, ToolReceipt,
    };
    use agent_base::engine::{AgentRuntime, RunPolicy};
    use agent_base::model::{StreamDelta, ToolCallInfo};
    use agent_base::ports::ToolCatalog;
    use agent_base::testing::{
        FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, MockScope, MockTool,
        ScriptedModelClient,
    };
    use agent_node::approval::HostApprovalGate;
    use agent_node::node_config::FixedNodeConfig;
    use agent_proto::ApprovalMode;

    const CALL_ID: &str = "call_approval_e2e";

    let approval_mgr = Arc::new(ApprovalManager::new());
    // `Ask` 档位：一律要问，稳定触发审批（不依赖危险命令匹配）
    let gate = Arc::new(HostApprovalGate::new(
        Arc::new(FixedNodeConfig::new(
            agent_base::model::ProviderConfig::default(),
            ApprovalMode::Ask,
        )),
        approval_mgr.clone(),
    ));

    let tool = Arc::new(MockTool::new(
        ToolDescriptor {
            name: "read_file".to_string(),
            summary: "读取文件".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Named("approval-guard"),
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        },
        ToolReceipt::success("文件内容", 0, 1),
    ));
    let catalog: Arc<dyn ToolCatalog> = Arc::new(InMemoryToolCatalog::with_tools(vec![tool]));

    let model = Arc::new(ScriptedModelClient::new(vec![
        // 第 1 轮：模型要求调用工具（必须先过审批）
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: CALL_ID.to_string(),
                    name: "read_file".to_string(),
                    args: r#"{"path":"README.md"}"#.to_string(),
                },
            },
            StreamDelta::Done { stop_reason: "tool_calls".to_string() },
        ],
        // 第 2 轮：拿到工具结果后自然收尾
        vec![
            StreamDelta::Text { text: "读完了".to_string() },
            StreamDelta::Done { stop_reason: "stop".to_string() },
        ],
    ]));

    let rt = Arc::new(AgentRuntime::new(
        model,
        catalog,
        gate,
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("测试系统提示词")),
        Arc::new(MockScope::new("s")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy::default(),
    ));

    let store = Arc::new(RwLock::new(store_for_test("E:/codes/approval_e2e_ws")));
    let test_dir = std::env::temp_dir().join(format!("a_da_appr_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(test_dir.clone()))),
        Arc::new(CheckpointManager::new(Some(test_dir.clone()))),
        Arc::new(SubagentManager::new()),
        approval_mgr.clone(),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
        None,
    )
    .with_engine(rt);

    let created = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "审批端到端" }))
        .await
        .unwrap();
    let tid = created
        .get("threadId")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    dispatcher
        .dispatch(
            "thread.send",
            serde_json::json!({ "threadId": tid, "text": "读一下 README" }),
        )
        .await
        .unwrap();

    // 取该会话下 CALL_ID 对应卡片的状态（界面渲染的依据就是这个状态）
    let card_status = |store: &Arc<RwLock<AgentStore>>, tid: &str| {
        let store = store.clone();
        let tid = tid.to_string();
        async move {
            let s = store.read().await;
            s.threads
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| {
                    t.items.iter().find_map(|it| match it {
                        Item::Tool { call_id, status, .. } if call_id == CALL_ID => {
                            Some(status.clone())
                        }
                        _ => None,
                    })
                })
        }
    };

    // 1) 审批请求必须落到 store：卡片进入 `waiting_approval`，且"有会话在等你"可见。
    //    这两件事缺任何一件，界面就弹不出审批卡（正是本次修的病）。
    let mut saw_waiting = false;
    let mut saw_visible = false;
    for _ in 0..300 {
        saw_waiting = card_status(&store, &tid).await.as_deref() == Some("waiting_approval");
        saw_visible = store
            .read()
            .await
            .waiting_thread_ids
            .iter()
            .any(|id| id == &tid);
        if saw_waiting && saw_visible {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        saw_waiting,
        "审批到达时工具卡片必须被补出来并进入 waiting_approval（否则界面没有批准按钮）"
    );
    assert!(
        saw_visible,
        "审批等待必须让 waiting_thread_ids 可见（否则别的标签页上完全看不出来）"
    );

    // 2) 界面上点"批准" → 与前端同一条 RPC
    let decided = dispatcher
        .dispatch(
            "approval.decide",
            serde_json::json!({ "toolItemId": CALL_ID, "approved": true }),
        )
        .await
        .expect("approval.decide 必须可分发");
    assert_eq!(
        decided.get("resolved").and_then(|v| v.as_bool()),
        Some(true),
        "必须命中引擎注册的等待通道（命中不了就说明闸门与 manager 没接上）"
    );

    // 3) 放行后本轮必须继续跑完：卡片收尾、等待态摘除、会话不再运行
    let mut done = false;
    for _ in 0..300 {
        done = card_status(&store, &tid).await.as_deref() == Some("done");
        if done {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(done, "批准后工具必须真的执行并收尾（卡片应为 done）");
    let s = store.read().await;
    assert!(
        !s.waiting_thread_ids.iter().any(|id| id == &tid),
        "开始执行/收尾后不得仍挂着'等待审批'"
    );
    assert!(
        s.threads
            .iter()
            .find(|t| t.id == tid)
            .map(|t| {
                t.items.iter().any(|it| {
                    matches!(it, Item::Assistant { text, .. } if text.contains("读完了"))
                })
            })
            .unwrap_or(false),
        "闸门放行后本轮应继续到底（第 2 轮模型的收尾文本要落进会话）"
    );

    let _ = std::fs::remove_dir_all(&test_dir);
}
