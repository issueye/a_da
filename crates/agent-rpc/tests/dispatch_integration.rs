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

fn write_extension_package(
    workspace: &std::path::Path,
    package: &str,
    tool_names: &[&str],
    prompt_ids: &[&str],
    skill_names: &[&str],
    hook_points: &[&str],
    factory_body: &str,
) -> std::path::PathBuf {
    let package_root = PluginManager::agent_extensions_dir(&workspace.to_string_lossy()).join(package);
    std::fs::create_dir_all(&package_root).expect("创建插件包目录");
    let metadata = serde_json::json!({
        "name": package, "description": format!("{package} test extension"),
        "tools": {"count": tool_names.len(), "names": tool_names},
        "systemPrompt": {"count": prompt_ids.len(), "names": prompt_ids},
        "skills": {"count": skill_names.len(), "names": skill_names},
        "hooks": {"count": hook_points.len(), "names": hook_points},
    });
    let entry = package_root.join("index.ts");
    std::fs::write(
        &entry,
        format!("const metadata = {}; export default function(ada: any) {{ ada.metadata = metadata; {} }}", metadata, factory_body),
    ).expect("写插件包入口");
    entry
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
    // 8 个内置插件（`decision` 已于 2026-10-18 随 `decide`/`check_gate` 一并下线）
    assert!(plugins_arr.len() >= 8);
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
/// 模式切换的**唯一窗口是"会话还没开始"**（本轮口径：滑块只在新建会话里出现）。
///
/// 这条用例守两件事：
/// 1. 空会话（新建、还没发过消息）里切模式**照旧有效**——否则"新会话选模式"就成了空话；
/// 2. 已经有内容的会话里切模式**必须被拒**，且理由说清"新建一条会话即可换模式"。
///
/// 为什么服务端也要拦（界面上的滑块已经不可点）：别处还有调用点（老客户端、脚本、
/// 将来的移动端）。改掉一个已开始会话的模式，等于把同一段对话的一半交给另一套系统
/// 提示词与工具集去解释，事后没人说得清哪句话是谁说的。
#[tokio::test]
async fn test_mode_can_only_be_chosen_before_the_conversation_starts() {
    let store = Arc::new(RwLock::new(store_for_test("E:/test")));
    let test_dir = std::env::temp_dir().join(format!("a_da_mode_{}", uuid::Uuid::new_v4()));
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

    // 造一条**干净**的会话：`store_for_test` 的 store 在集成测试里可能已经同步过
    // 别的会话（依赖按非 test 编译，应用目录端口仍指真实 `~/.a-da`），
    // 所以这里不假设活动会话是空的，而是自己建一条。
    let created = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "模式用例" }))
        .await
        .expect("新建会话");
    let tid = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();
    {
        let mut s = store.write().await;
        if let Some(t) = s.threads.iter_mut().find(|t| t.id == tid) {
            t.items.clear();
            t.messages.clear();
        }
    }

    // ① 空会话：切模式有效，并且同时更新"下一个新会话的默认模式"
    dispatcher
        .dispatch("thread.setMode", serde_json::json!({ "mode": "plan", "threadId": tid }))
        .await
        .expect("空会话应当允许选模式");
    {
        let s = store.read().await;
        assert_eq!(s.config.mode, AgentMode::Plan, "config.mode 是下一个新会话的默认值");
        assert_eq!(
            s.threads.iter().find(|t| t.id == tid).and_then(|t| t.mode.clone()),
            Some(AgentMode::Plan)
        );
    }

    // ② 会话一旦有内容：拒，且理由能指导用户怎么办
    {
        let mut s = store.write().await;
        if let Some(t) = s.threads.iter_mut().find(|t| t.id == tid) {
            t.messages.push(serde_json::json!({ "role": "user", "content": "开始干活" }));
        }
    }
    let err = dispatcher
        .dispatch("thread.setMode", serde_json::json!({ "mode": "create", "threadId": tid }))
        .await
        .expect_err("已开始的会话不能再切模式");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("会话已开始") && msg.contains("新建一条会话"),
        "拒绝理由要说清为什么、以及该怎么办：{msg}"
    );
    assert_eq!(
        store.read().await.config.mode,
        AgentMode::Plan,
        "被拒之后连默认模式也不该被改（否则下一条新会话会莫名变成另一个模式）"
    );

    // ③ 新建会话时带模式仍然有效（滑块那一路的正规做法）
    let created2 = dispatcher
        .dispatch("thread.create", serde_json::json!({ "mode": "create", "title": "带模式" }))
        .await
        .expect("带模式新建会话");
    let tid2 = created2.get("threadId").and_then(|v| v.as_str()).unwrap();
    let t2 = store.read().await.threads.iter().find(|t| t.id == tid2).cloned().unwrap();
    assert_eq!(t2.mode, Some(AgentMode::Create));
    // PM 是模式不是产品：产品身份恒为 ada-coding
    assert_eq!(t2.agent_id, Some("ada-coding".to_string()));

    let _ = std::fs::remove_dir_all(&test_dir);
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

    // 3. 测试 8 大内置插件（`decision` 已于 2026-10-18 下线）
    let plugins = plugin_mgr.scan_plugins(None);
    assert!(plugins.len() >= 8);
    let plugin_ids: Vec<String> = plugins.iter().map(|p| p.id.clone()).collect();
    assert!(plugin_ids.contains(&"builtin:git-tools".to_string()));
    assert!(plugin_ids.contains(&"builtin:code-outline".to_string()));
    assert!(plugin_ids.contains(&"builtin:project-inspector".to_string()));
    assert!(plugin_ids.contains(&"builtin:test-runner".to_string()));
    assert!(plugin_ids.contains(&"builtin:batch-ops".to_string()));
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

/// 设置项「深度档位 / 审批档位」的端到端契约：**改完重启还在**。
///
/// 用户报过的真实症状：把"推理力度"设成「中（Medium）」，重启后又变回默认。
///
/// 根因不可能在 UI（它每次保存都会发 `config.setEffort`），而在"内存改了、文件没写"：
/// `save_app_config` 是**手工列字段**的，字段表里少了哪一项，重启就精确地丢哪一项。
/// 所以这条测试同时钉两件事：
/// 1. 配置文件里真的出现了这几个键（漏写字段 = 这里红）；
/// 2. 用同一份 home 重建 store（等价于"重启进程"）能把值读回来（漏读 = 这里红）。
#[tokio::test]
async fn test_effort_and_approval_survive_restart() {
    use agent_base::testing::TempAppHome;
    use agent_proto::{ApprovalMode, Effort};

    let test_dir = std::env::temp_dir().join(format!("a_da_effort_cfg_{}", uuid::Uuid::new_v4()));
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

    // 1. 默认档位：深度 = Max（"最大"），审批 = Auto
    assert_eq!(store.read().await.config.effort, Effort::Max);

    // 2. 改成「中」+「询问确认」
    dispatcher
        .dispatch("config.setEffort", serde_json::json!({ "effort": "medium" }))
        .await
        .expect("config.setEffort 必须可分发");
    dispatcher
        .dispatch("config.setApproval", serde_json::json!({ "mode": "ask" }))
        .await
        .expect("config.setApproval 必须可分发");
    assert_eq!(store.read().await.config.effort, Effort::Medium, "内存态必须立即生效");

    // 3. 真的落盘——这正是不变量所在：手工列字段的写法最容易漏项
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("配置必须已落盘")).unwrap();
    assert_eq!(
        written.get("effort").and_then(|v| v.as_str()),
        Some("medium"),
        "深度档位必须写进 config.json：{written}"
    );
    assert_eq!(
        written.get("approval").and_then(|v| v.as_str()),
        Some("ask"),
        "审批档位必须写进 config.json：{written}"
    );

    // 4. 重启：同一份 home 重新构造 store，值必须原样回来
    let reloaded = AgentStore::with_home("E:/test".to_string(), &home);
    assert_eq!(reloaded.config.effort, Effort::Medium, "重启后深度档位不能回默认");
    assert_eq!(reloaded.config.approval, ApprovalMode::Ask, "重启后审批档位不能回默认");
    assert_eq!(
        reloaded.config.retry_count,
        agent_base::model::DEFAULT_MODEL_RETRY_COUNT as u32,
        "别的设置项不该被顺手改掉"
    );

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

/// 「模型请求失败」的报错必须落在 **Notice 卡片**上，而不是助手正文里。
///
/// 为什么值得一条集成测试（而不是只测 jsonl 的 `fold_messages`）：
/// 失败链路的三个环节分布在三个 crate——宿主拼文案并落盘（`agent-rpc`）、
/// 会话文件读写（`agent-adapter`）、还原成 UI 卡片（`agent-node`）。
/// 任何一环把 Notice 当成普通消息，"报错混进模型上下文"就会悄悄回来。
///
/// 这条测试同时钉住两件事：
/// 1. 还原出的 items 里有 `Item::Notice`（界面有东西可渲染）；
/// 2. 模型上下文（`load_session` 的 messages）里**没有**这条报错——
///    否则模型会以为"我上一轮说过这句"，而一段连贯回答也会被从中间劈开。
#[tokio::test]
async fn test_failure_notice_stays_out_of_assistant_text_and_model_context() {
    let test_dir = std::env::temp_dir().join(format!("a_da_test_notice_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let session_mgr = SessionManager::new(Some(test_dir.clone()));
    let ws = "E:/test-notice";
    let tid = "thread_notice_1";
    session_mgr
        .create_session(tid, ws, Some("提示测试"), None, None)
        .expect("建会话失败");

    // 先有一条正常对话，模拟"回答说到一半失败了"（正是截图里的形态）
    session_mgr
        .append_message(
            tid,
            agent_base::domain::AgentMessage::User {
                content: "帮我看下这个项目".to_string(),
                images: None,
                timestamp: Some(1),
            },
            Some(ws),
        )
        .expect("写用户消息失败");
    session_mgr
        .append_message(
            tid,
            agent_base::domain::AgentMessage::Assistant {
                content: "我先看看目录结构。".to_string(),
                thinking: None,
                tool_calls: None,
                stop_reason: Some("stop".to_string()),
                error_message: None,
                timestamp: Some(2),
                usage: None,
                duration_ms: None,
                turn_duration_ms: None,
            },
            Some(ws),
        )
        .expect("写助手消息失败");

    let err_text = "请求异常：OpenAI API 错误 (HTTP 400)（已重试 0 次仍失败）\n\
                    重试已耗尽，本次对话已断开；排队中的指令仍保留。";
    session_mgr
        .append_notice_entry(tid, err_text, "error", Some(ws))
        .expect("写 notice 失败");

    // 1. 还原出的卡片流：助手正文干净 + 有一条 error 级 Notice
    let threads = session_mgr.restore_all_threads();
    let t = threads.iter().find(|t| t.id == tid).expect("应还原出会话");
    assert!(
        t.items.iter().any(|it| matches!(it, Item::Notice { level, text, .. }
            if level == "error" && text.contains("已重试 0 次仍失败"))),
        "报错应还原成一张 Notice 卡片；实际 items：{:?}",
        t.items
    );
    assert!(
        !t.items
            .iter()
            .any(|it| matches!(it, Item::Assistant { text, .. } if text.contains("请求异常"))),
        "报错**不得**混进助手正文"
    );

    // 2. 模型上下文里没有它（这才是"只给人看"的硬证据）
    let (_, messages) = session_mgr
        .load_session(tid, Some(ws))
        .expect("加载会话失败")
        .expect("会话应存在");
    let flat = format!("{messages:?}");
    assert!(
        !flat.contains("请求异常") && !flat.contains("重试已耗尽"),
        "Notice 绝不该进入模型上下文；实际：{flat}"
    );
    assert_eq!(messages.len(), 2, "上下文只应有两轮正常对话");

    // 3. 重试清理：内存卡片没了，磁盘记录也得没（否则同步会把卡片拽回来）
    let mut store_items = t.items.clone();
    store_items.retain(|it| !matches!(it, Item::Notice { .. }));
    assert!(
        !store_items.iter().any(|it| matches!(it, Item::Notice { .. })),
        "清内存后不该还有 Notice"
    );
    let removed = session_mgr.clear_notice_entries(tid).expect("清理磁盘 notice 失败");
    assert_eq!(removed, 1, "磁盘上应正好删掉 1 条 notice");
    assert!(
        session_mgr
            .clear_notice_entries(tid)
            .expect("再清一次不该报错")
            == 0,
        "没有 notice 时应如实返回 0"
    );
    // 清完之后：正常对话仍在，报错彻底消失
    let threads_after = session_mgr.restore_all_threads();
    let t_after = threads_after.iter().find(|t| t.id == tid).expect("会话应还在");
    assert!(
        !t_after.items.iter().any(|it| matches!(it, Item::Notice { .. })),
        "磁盘清干净后不该再还原出 Notice"
    );
    assert!(
        t_after
            .items
            .iter()
            .any(|it| matches!(it, Item::Assistant { text, .. } if text.contains("我先看看"))),
        "清理只该删 Notice，正常对话必须留着"
    );

    let _ = std::fs::remove_dir_all(&test_dir);
}

/// SP-06 端到端（P0 埋点地基）：**诊断遥测必须走完整条链**。
///
/// 引擎发 `Diagnostic` 事件 → dispatch 投影进 `AgentStore.log` → 快照的 `log` 字段下发 →
/// 界面按 `payload.origin` 过滤。
///
/// 为什么这条用例必要：面板此前只显示客户端自己记的请求/响应，Rust 侧的诊断写进
/// `store.log` 之后**没有任何读出路径**——埋点等于白埋。这条用例把"链子真的接上了"钉住，
/// 而且断言的是**界面过滤所依赖的那些字段**（`family` / `point` / `origin` / `outcome` /
/// `turnId`），不是"日志里多了一行字"。
#[tokio::test]
async fn test_diagnostics_reach_store_log_and_snapshot_with_origin() {
    use agent_base::engine::{AgentRuntime, RunPolicy};
    use agent_base::model::StreamDelta;
    use agent_base::testing::{
        FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, RecordingApprovalGate,
        MockScope, ScriptedModelClient,
    };

    // 最小回合：无工具、纯文本收尾
    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "你好".to_string() },
        StreamDelta::Done { stop_reason: "stop".to_string() },
    ]]));
    let rt = Arc::new(AgentRuntime::new(
        model,
        Arc::new(InMemoryToolCatalog::new()),
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("测试系统提示词")),
        Arc::new(MockScope::new("s")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy::default(),
    ));

    let store = Arc::new(RwLock::new(store_for_test("E:/codes/diag_e2e")));
    let test_dir = std::env::temp_dir().join(format!("a_da_diag_test_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(test_dir.clone()))),
        Arc::new(CheckpointManager::new(Some(test_dir.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
        None,
    )
    .with_engine(rt);

    let created = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "诊断链" }))
        .await
        .unwrap();
    let tid = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();

    dispatcher
        .dispatch("thread.send", serde_json::json!({ "threadId": tid, "text": "打个招呼" }))
        .await
        .unwrap();

    // `thread.send` 是**后台起轮次**（生产同构）：必须等它真的跑起来，
    // 否则断言会撞上"事件还没到"的竞态。
    let find_composed = |s: &AgentStore| {
        s.log
            .iter()
            .find(|e| {
                e.kind == "diag"
                    && e.payload
                        .as_ref()
                        .and_then(|p| p.get("point"))
                        .and_then(|v| v.as_str())
                        == Some("prompt.composed")
            })
            .cloned()
    };
    let mut diag = None;
    for _ in 0..300 {
        diag = {
            let s = store.read().await;
            find_composed(&s)
        };
        if diag.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let s = store.read().await;

    // 1) 诊断落进了 store.log，且是结构化的（界面靠这些字段过滤/展示）
    let diag = diag.or_else(|| find_composed(&s)).expect(
        "每轮必须有一条 `prompt.composed` 诊断落进 store.log",
    );
    let payload = diag.payload.as_ref().expect("诊断必须带结构化 payload");
    assert_eq!(payload["origin"], "engine", "来源标注是界面过滤的依据：{payload:?}");
    assert_eq!(payload["family"], "prompt");
    assert_eq!(payload["outcome"], "applied");
    assert!(
        payload["turnId"].is_string(),
        "关联键必须下发（界面按 turn_id 归组）：{payload:?}"
    );
    assert!(
        payload["bytes"].as_u64().unwrap_or(0) > 0,
        "提示词字节数必须如实上报：{payload:?}"
    );

    // 2) 快照的 `log` 字段带着它（界面就是从这里读的）
    let snapshot = generate_snapshot(&s);
    let json = serde_json::to_value(&snapshot).expect("快照必须可序列化");
    let snap_log = json
        .get("log")
        .and_then(|v| v.as_array())
        .expect("快照必须带 log 数组");
    assert!(
        snap_log
            .iter()
            .any(|e| e["kind"] == "diag" && e["payload"]["origin"] == "engine"),
        "诊断必须随快照下发，否则界面无路可读：{snap_log:?}"
    );

    drop(s);
    let _ = std::fs::remove_dir_all(&test_dir);
}

/// SP-08 契约（P0 埋点地基，插件/技能侧）：扫描类诊断必须**如实落到诊断缓冲**，
/// 且**缺席可见**——"一个都没有"要报出来，而不是什么都不发。
///
/// 这条守的是 AGENTS §15 的那个坑：声明了却没人发 = 静默失效。缺席不报的话，
/// 面板上"没扫到"与"根本没扫过"长得一模一样。
#[tokio::test]
async fn test_plugin_and_skill_scan_diagnostics_with_absence_signal() {
    let store = Arc::new(RwLock::new(store_for_test("E:/codes/diag_scan")));
    // 空的临时应用目录：既保证"没有磁盘插件/自定义技能"，也不碰用户真实的 ~/.a-da
    let home = test_home_dir();
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(home.clone()))),
        Arc::new(CheckpointManager::new(Some(home.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(home.clone())),
        Arc::new(SkillManager::with_home(home.clone())),
        None,
    );

    dispatcher
        .dispatch("plugin.list", serde_json::json!({ "workspace": "E:/codes/diag_scan" }))
        .await
        .expect("plugin.list 必须可分发");
    dispatcher
        .dispatch("skill.list", serde_json::json!({ "workspace": "E:/codes/diag_scan" }))
        .await
        .expect("skill.list 必须可分发");

    let s = store.read().await;
    let find = |point: &str| {
        s.log
            .iter()
            .find(|e| {
                e.kind == "diag"
                    && e.payload
                        .as_ref()
                        .and_then(|p| p.get("point"))
                        .and_then(|v| v.as_str())
                        == Some(point)
            })
            .and_then(|e| e.payload.clone())
    };

    // 1) 没有磁盘插件 → 必须留下"缺席"诊断（而不是一条都没有）
    let discovered = find("plugin.discovered")
        .expect("插件扫描必须留下诊断：一个都没有时也要报缺席");
    assert_eq!(
        discovered["outcome"], "skipped",
        "没有插件时 outcome 应为 skipped：{discovered:?}"
    );
    assert_eq!(
        discovered["reason"], "no_plugin",
        "缺席理由必须如实写出来：{discovered:?}"
    );

    // 2) 技能同理（内置技能总是存在，所以缺席信号针对**自定义**技能）
    let skill = find("skill.skipped").expect("技能扫描必须留下缺席诊断");
    assert_eq!(skill["reason"], "no_custom_skill", "{skill:?}");
    assert!(
        skill["data"]["builtin"].as_u64().unwrap_or(0) > 0,
        "内置技能数量要如实带上（否则看不出'只是没有自定义的'）：{skill:?}"
    );

    drop(s);
}

/// P2·SP-45：`skill.list` 必须**同时**回答两件界面答不出来的事——
///
/// 1. 插件带来的技能在列表里（`pluginId` 非空、`scope = "plugin"`）：装配期收好的那份
///    随 `EngineInjection` 递进来，所以"界面列出的"与"真的进提示词的"是同一份；
/// 2. frontmatter 里认不出的键要报出来（`skill.invalid`）——以前是静默忽略。
#[tokio::test]
async fn test_skill_list_reports_plugin_skills_and_invalid_frontmatter() {
    // 工作区里放一个**写错了键名**的技能：它必须进列表（照旧可用）**且**留下诊断
    let ws_dir = std::env::temp_dir().join(format!("a_da_ws_skill_{}", uuid::Uuid::new_v4()));
    let skill_dir = ws_dir.join(".ada").join("skills").join("typo");
    std::fs::create_dir_all(&skill_dir).expect("建技能目录失败");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: 拼错的技能\ndescription: 说明\nwhen_to_use_typo: 写错了\n---\n正文\n",
    )
    .expect("写技能失败");
    let ws = ws_dir.to_string_lossy().to_string();

    let store = Arc::new(RwLock::new(store_for_test(&ws)));
    let home = test_home_dir();
    let plugin_skill = agent_node::plugins::PluginSkillDeclaration {
        name: "deploy-runbook".to_string(),
        description: "部署手册".to_string(),
        path: None,
        body: "步骤一……".to_string(),
        when_to_use: None,
        user_invocable: None,
    }
    .to_summary("demo-plugin", "Demo 插件");

    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(home.clone()))),
        Arc::new(CheckpointManager::new(Some(home.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(home.clone())),
        Arc::new(SkillManager::with_home(home.clone())),
        None,
    )
    // P2·SP-46 起技能是**共享活体**（不是快照）：启停重装配原地换它
    .with_plugin_skills(agent_node::skills::new_skill_store(vec![plugin_skill]));

    let listed = dispatcher
        .dispatch("skill.list", serde_json::json!({ "workspace": ws }))
        .await
        .expect("skill.list 必须可分发");
    let arr = listed.as_array().expect("技能列表必须是数组");

    let plugin_skill = arr
        .iter()
        .find(|s| s["name"] == "deploy-runbook")
        .expect("插件技能必须出现在列表里（否则界面少一截、模型却看得到）");
    assert_eq!(plugin_skill["scope"], "plugin");
    assert_eq!(
        plugin_skill["pluginId"], "demo-plugin",
        "归因字段必须被填上（此前恒为空）：{plugin_skill:?}"
    );
    assert_eq!(plugin_skill["pluginName"], "Demo 插件");

    let typo = arr
        .iter()
        .find(|s| s["name"] == "拼错的技能")
        .expect("写错的技能照样进列表（不该因为一个键就整条消失）");
    assert_eq!(
        typo["unknownFrontmatterKeys"],
        serde_json::json!(["when_to_use_typo"]),
        "认不出的键要如实带出去：{typo:?}"
    );

    // 诊断侧：`skill.invalid` 必须落在缓冲里，且说得清是哪个技能、哪个键
    let s = store.read().await;
    let invalid = s
        .log
        .iter()
        .find(|e| {
            e.kind == "diag"
                && e.payload
                    .as_ref()
                    .and_then(|p| p.get("point"))
                    .and_then(|v| v.as_str())
                    == Some("skill.invalid")
        })
        .and_then(|e| e.payload.clone())
        .expect("frontmatter 有认不出的键就必须留下 skill.invalid 诊断");
    assert_eq!(invalid["reason"], "unknown_frontmatter_key", "{invalid:?}");
    assert_eq!(
        invalid["data"]["keys"],
        serde_json::json!(["when_to_use_typo"]),
        "要指出是哪个键（否则作者只能靠猜）：{invalid:?}"
    );
    drop(s);

    let _ = std::fs::remove_dir_all(&ws_dir);
}

/// P3a·SP-55：管理面点位（插件启停 / 会话创建）在 **RPC 这一层**跑，而且：
///
/// - 拦下即定稿：**不写盘、不重装配**，接口如实回 `denied`；
/// - 会话创建那条必须**原子**：拒绝之后不得留下"内存里建了、磁盘没建"的半成品；
/// - 事后点位只追加旁注（控制面第三方只能 observe / note）。
#[tokio::test]
async fn test_management_plane_hooks_gate_toggle_and_thread_create() {
    use agent_base::testing::ScriptedHookRunner;

    let store = Arc::new(RwLock::new(store_for_test("E:/codes/manage_hooks")));
    let home = test_home_dir();
    let build = || {
        Dispatcher::new(
            store.clone(),
            Arc::new(SessionManager::new(Some(home.clone()))),
            Arc::new(CheckpointManager::new(Some(home.clone()))),
            Arc::new(SubagentManager::new()),
            Arc::new(ApprovalManager::new()),
            Arc::new(PluginManager::with_home(home.clone())),
            Arc::new(SkillManager::with_home(home.clone())),
            None,
        )
    };

    // ① 插件启停被拦下 → 不写盘、不重装配
    let guard = Arc::new(
        ScriptedHookRunner::new()
            .deny_at("beforePluginToggle", "这个插件正在被审计，暂不允许停用"),
    );
    let dispatcher = build()
        .with_hook_runner(guard.clone())
        .with_plugin_reassembler(Arc::new(NoopReassembler));
    let res = dispatcher
        .dispatch(
            "plugin.setEnabled",
            serde_json::json!({ "pluginId": "demo-plugin", "enabled": false }),
        )
        .await
        .expect("plugin.setEnabled 必须可分发");
    assert_eq!(res["denied"], true, "{res:?}");
    assert!(
        res["reason"]
            .as_str()
            .unwrap_or("")
            .contains("正在被审计"),
        "拦下的理由要如实回给调用方：{res:?}"
    );
    assert!(res.get("report").is_none(), "被拦下就不该有重装配收据：{res:?}");
    assert_eq!(
        guard.calls(),
        vec!["beforePluginToggle".to_string()],
        "被拦下之后不该再问事后点位：{:?}",
        guard.calls()
    );

    // ② 会话创建被拦下 → 原子：store 里不得出现新会话
    let before_count = store.read().await.threads.len();
    let veto = Arc::new(ScriptedHookRunner::new().deny_at(
        "beforeThreadCreate",
        "当前有正在跑的审计，先不要新建会话",
    ));
    let dispatcher = build().with_hook_runner(veto);
    let res = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "被拦下的会话" }))
        .await
        .expect("thread.create 必须可分发");
    assert_eq!(res["denied"], true, "{res:?}");
    assert_eq!(
        store.read().await.threads.len(),
        before_count,
        "被拦下必须在**任何状态变更之前**（否则留下'内存里建了、磁盘没建'的半成品）"
    );

    // ③ 放行 + 事后旁注：`afterThreadCreate` 的 note 随回执带出
    let note_taker = Arc::new(ScriptedHookRunner::new().note_at(
        "afterThreadCreate",
        "已记录：新会话已落盘",
    ));
    let dispatcher = build().with_hook_runner(note_taker);
    let res = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "正常新建的会话" }))
        .await
        .expect("thread.create 必须可分发");
    assert!(res["threadId"].is_string(), "正常路径要回 threadId：{res:?}");
    assert_eq!(
        res["notes"],
        serde_json::json!(["已记录：新会话已落盘"]),
        "事后旁注要随回执带出（模型/审计插件能看到）：{res:?}"
    );
}

/// 最小重装配替身（只用来证明"被拦下时它压根不会被调"）。
struct NoopReassembler;

impl agent_rpc::server::dispatch::PluginReassembler for NoopReassembler {
    fn reassemble<'a>(
        &'a self,
        plugin_id: &'a str,
        enabled: bool,
    ) -> agent_base::ports::BoxFuture<'a, Option<agent_runtime::ReassemblyReport>> {
        Box::pin(async move {
            let empty = agent_runtime::ContributionDigest::default();
            Some(agent_runtime::ReassemblyReport {
                request: agent_runtime::PluginToggleRequest {
                    plugin_id: plugin_id.to_string(),
                    scope: "workspace".to_string(),
                    from_enabled: !enabled,
                    to_enabled: enabled,
                },
                before: empty.clone(),
                after: empty.clone(),
                delta: agent_runtime::ContributionDelta::default(),
                skipped: Vec::new(),
                applied: true,
            })
        })
    }
}

/// P2·SP-46：`plugin.setEnabled` 必须**真的驱动重装配**，并把"实际生效的差异"回给界面。
///
/// 两个方向都守：
/// - 接了重装配入口 → 回执里带上差异（界面据此知道"关掉之后少了什么"）；
/// - 没接 → `applied: false`（**如实**：别让界面把"只写了个布尔值"当成"已经生效"）。
#[tokio::test]
async fn test_plugin_set_enabled_drives_reassembly_and_reports_the_delta() {
    /// 假重装配器：报"启用时新增一个工具、停用时移除它"。
    struct FakeReassembler;

    impl agent_rpc::server::dispatch::PluginReassembler for FakeReassembler {
        fn reassemble<'a>(
            &'a self,
            plugin_id: &'a str,
            enabled: bool,
        ) -> agent_base::ports::BoxFuture<'a, Option<agent_runtime::ReassemblyReport>> {
            Box::pin(async move {
                let empty = agent_runtime::ContributionDigest::default();
                let mut after = agent_runtime::ContributionDigest::default();
                if enabled {
                    after.tools = vec![format!("tool:plugin:{plugin_id}:web_search")];
                }
                Some(agent_runtime::ReassemblyReport {
                    request: agent_runtime::PluginToggleRequest {
                        plugin_id: plugin_id.to_string(),
                        scope: "workspace".to_string(),
                        from_enabled: !enabled,
                        to_enabled: enabled,
                    },
                    before: empty.clone(),
                    after: after.clone(),
                    delta: agent_runtime::ContributionDelta::between(&empty, &after),
                    skipped: Vec::new(),
                    applied: true,
                })
            })
        }
    }

    let store = Arc::new(RwLock::new(store_for_test("E:/codes/plugin_toggle")));
    let home = test_home_dir();
    let build = || {
        Dispatcher::new(
            store.clone(),
            Arc::new(SessionManager::new(Some(home.clone()))),
            Arc::new(CheckpointManager::new(Some(home.clone()))),
            Arc::new(SubagentManager::new()),
            Arc::new(ApprovalManager::new()),
            Arc::new(PluginManager::with_home(home.clone())),
            Arc::new(SkillManager::with_home(home.clone())),
            None,
        )
    };

    // ① 接了重装配入口 → 回执带差异
    let with = build().with_plugin_reassembler(Arc::new(FakeReassembler));
    let res = with
        .dispatch(
            "plugin.setEnabled",
            serde_json::json!({ "pluginId": "demo-plugin", "enabled": true }),
        )
        .await
        .expect("plugin.setEnabled 必须可分发");
    assert_eq!(res["applied"], true, "{res:?}");
    assert_eq!(res["report"]["request"]["pluginId"], "demo-plugin", "{res:?}");
    assert!(
        res["report"]["delta"]["added"][0]
            .as_str()
            .unwrap_or("")
            .contains("web_search"),
        "回执必须带上实际生效的差异：{res:?}"
    );

    // ② 没接 → 如实说"没有生效"（不撒谎）
    let without = build();
    let res = without
        .dispatch(
            "plugin.setEnabled",
            serde_json::json!({ "pluginId": "demo-plugin", "enabled": false }),
        )
        .await
        .expect("plugin.setEnabled 必须可分发");
    assert_eq!(
        res["applied"], false,
        "没有重装配入口时必须如实回报（否则界面会以为关掉就生效了）：{res:?}"
    );
    assert!(res["report"].is_null(), "{res:?}");
}

/// SP-08 契约（正向）：磁盘上真有一个插件时，诊断必须**如实**说明两件事——
/// 它被扫到了，以及**本页（列表）并没有执行它**（工具清单是文本推断的，
/// 实际生效的贡献以装配期的脚本执行结果为准）。
///
/// 为什么这条也必要：界面把"列出来了"当成"在生效"是这次埋点要根治的误读之一。
///
/// 🔴 这条用例改过一次，改的原因值得留着：它原先断言的是"执行面尚未接线"。
/// P1 把执行面接上了，那句断言就从"如实"变成了"钉住一句假话"——留着它，
/// 面板会永远对一个正常服务的插件喊"它不会被执行"。现在钉的是**真话**：
/// 列表只发现 Factory 入口、不执行插件代码；贡献由 Extension Factory 实际注册。
#[tokio::test]
async fn test_disk_plugin_is_reported_as_discovered_with_listing_caveat() {
    let ws_dir = std::env::temp_dir().join(format!("a_da_ws_plugin_{}", uuid::Uuid::new_v4()));
    write_extension_package(&ws_dir, "hello", &["hello_world"], &[], &[], &[],
        "ada.registerTool({ name: 'hello_world', description: '打招呼', parameters: { type: 'object', properties: {} }, execute: async () => 'hello' });");

    let ws = ws_dir.to_string_lossy().to_string();
    let store = Arc::new(RwLock::new(store_for_test(&ws)));
    let home = test_home_dir();
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(home.clone()))),
        Arc::new(CheckpointManager::new(Some(home.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(home.clone())),
        Arc::new(SkillManager::with_home(home.clone())),
        None,
    );

    let listed = dispatcher
        .dispatch("plugin.list", serde_json::json!({ "workspace": ws }))
        .await
        .expect("plugin.list 必须可分发");

    // 2) 响应里的插件项必须自述口径——这条字段以前恒为空，
    //    于是插件卡永远显示不出问题（"有壳无用"）。
    let items = listed["plugins"].as_array().expect("plugin.list 必须回插件数组");
    let disk_item = items
        .iter()
        .find(|p| p["scope"] != "builtin")
        .expect("磁盘插件必须出现在列表里");
    let diags = disk_item["diagnostics"]
        .as_array()
        .expect("插件项必须带 diagnostics");
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("Extension Factory")),
        "静态插件目录应说明真实贡献由 Extension Factory 注册：{diags:?}"
    );
    assert!(
        !diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("尚未接线")),
        "执行面在 P1 已接线，不许再对正常插件喊'未接线'：{diags:?}"
    );

    let s = store.read().await;
    let by_point = |point: &str| -> Vec<serde_json::Value> {
        s.log
            .iter()
            .filter(|e| {
                e.kind == "diag"
                    && e.payload
                        .as_ref()
                        .and_then(|p| p.get("point"))
                        .and_then(|v| v.as_str())
                        == Some(point)
            })
            .filter_map(|e| e.payload.clone())
            .collect()
    };

    let discovered = by_point("plugin.discovered");
    assert!(
        discovered
            .iter()
            .any(|p| p["outcome"] == "applied" && p["origin"].as_str().unwrap_or("").starts_with("plugin:")),
        "磁盘插件必须被如实报为 discovered 并按插件 id 归因：{discovered:?}"
    );

    let skipped = by_point("plugin.skipped");
    assert!(
        skipped.is_empty(),
        "这个插件是启用状态：本页没有任何东西被跳过（P1 已接线，启用中的插件工具会被模型调用，\
         不许再报 execution_not_wired）：{skipped:?}"
    );


    drop(s);
    let _ = std::fs::remove_dir_all(&ws_dir);
}

/// SP-20 端到端冒烟（P1 主线的闭环）：**插件工具被模型调用并拿到成功回执**。
///
/// 这条把所有前序任务串起来跑一遍真实链路：
/// 磁盘插件 → 装配期收集（SP-16）→ 进 catalog → 模型调用 → 脚本沙箱执行
/// （SP-10 端口 / SP-12 限额 / SP-13 注入清单）→ 回执映射（SP-15 唯一转换点）
/// → 界面卡片 `done`。
///
/// 判据刻意**不看日志、只看 store 里的卡片**：卡片是界面渲染的依据，
/// 也是"模型真的拿到了什么"的投影。
#[tokio::test]
async fn test_plugin_tool_is_called_by_the_model_and_returns_success() {
    use agent_base::engine::{AgentRuntime, RunPolicy};
    use agent_base::model::{StreamDelta, ToolCallInfo};
    use agent_base::ports::{ScriptBudget, ScriptHost, ToolCatalog};
    use agent_base::testing::{
        FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, MockScope,
        RecordingApprovalGate, ScriptedModelClient,
    };
    use std::sync::Arc;

    const CALL_ID: &str = "call_plugin_e2e";

    // 1) 临时工作区里的 pi-style extension package，自定义工具由 Factory 注册。
    let ws = std::env::temp_dir().join(format!("a_da_sp20_{}", uuid::Uuid::new_v4()));
    write_extension_package(&ws, "p", &["read_file"], &[], &[], &[],
        "ada.registerTool({ name: 'read_file', description: '插件实现', parameters: { type: 'object', properties: { path: { type: 'string' } } }, execute: async (args: any) => ({ ok: true, output: 'plugin-output:' + args.path }) });");

    // 2) 真装配：发现 package → 执行 Extension Factory → 注册声明贡献
    let mgr = PluginManager::with_home(test_home_dir());
    let script_host: Arc<dyn ScriptHost> = Arc::new(agent_node::scripts::TsScriptHost::new());
    let asm = agent_node::scripts::collect_plugin_tools(
        &mgr,
        &script_host,
        &ws,
        ScriptBudget::default(),
    )
    .await;
    assert_eq!(
        asm.tools.len(),
        1,
        "插件应当提供 1 个可装工具，实际 {:?}",
        asm.skipped
    );

    // 3) catalog **只有插件这一个工具**：于是"模型调到的"必然是插件实现
    let catalog: Arc<dyn ToolCatalog> = Arc::new(InMemoryToolCatalog::with_tools(asm.tools.into_iter().map(|(_, t)| t).collect::<Vec<_>>()));

    // 4) 脚本模型：先要工具、拿到结果后收尾
    let model = Arc::new(ScriptedModelClient::new(vec![
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
        vec![
            StreamDelta::Text { text: "读到了".to_string() },
            StreamDelta::Done { stop_reason: "stop".to_string() },
        ],
    ]));

    let rt = Arc::new(AgentRuntime::new(
        model,
        catalog,
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("测试系统提示词")),
        Arc::new(MockScope::new("s")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy::default(),
    ));

    let store = Arc::new(RwLock::new(store_for_test(&ws.to_string_lossy())));
    let test_dir = std::env::temp_dir().join(format!("a_da_sp20_home_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(test_dir.clone()))),
        Arc::new(CheckpointManager::new(Some(test_dir.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
        None,
    )
    .with_engine(rt);

    let created = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "插件工具冒烟" }))
        .await
        .unwrap();
    let tid = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();

    dispatcher
        .dispatch("thread.send", serde_json::json!({ "threadId": tid, "text": "读一下 README" }))
        .await
        .unwrap();

    // 5) 等卡片收尾，然后断言"插件真的被执行了"
    let mut card: Option<(String, Option<String>)> = None;
    for _ in 0..300 {
        card = {
            let s = store.read().await;
            s.threads.iter().find(|t| t.id == tid).and_then(|t| {
                t.items.iter().find_map(|it| match it {
                    Item::Tool { call_id, status, output, .. } if call_id == CALL_ID => {
                        Some((status.clone(), output.clone()))
                    }
                    _ => None,
                })
            })
        };
        if card.as_ref().map(|(st, _)| st == "done").unwrap_or(false) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let (status, output) = card.expect("必须留下这次工具调用的卡片");
    let output = output.unwrap_or_default();
    assert_eq!(status, "done", "回执必须是成功（SP-15 的唯一转换点），实际 output={output}");
    assert!(
        output.contains("plugin-output:README.md"),
        "卡片输出必须来自**插件脚本**（而不是任何内置实现）：{output}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&test_dir);
}

/// SP-18：`plugin.validate` 必须**说人话**——插件作者据此知道"为什么它没生效"。
#[tokio::test]
async fn test_plugin_validate_reports_actionable_problems() {
    let ws = std::env::temp_dir().join(format!("a_da_sp18_{}", uuid::Uuid::new_v4()));
    let good = write_extension_package(
        &ws, "good", &["read_file"], &[], &[], &[],
        "ada.registerTool({ name: 'read_file', description: 'x', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
    );
    let bad_root = PluginManager::agent_extensions_dir(&ws.to_string_lossy()).join("bad");
    std::fs::create_dir_all(&bad_root).expect("建坏插件目录失败");
    let bad = bad_root.join("index.ts");
    std::fs::write(&bad, "export default function (ada: any) { ").expect("写坏插件失败");

    let store = Arc::new(RwLock::new(store_for_test(&ws.to_string_lossy())));
    let test_dir = std::env::temp_dir().join(format!("a_da_sp18_home_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
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

    // 1) 好插件：问题清单为空
    let ok = dispatcher
        .dispatch(
            "plugin.validate",
            serde_json::json!({ "pluginId": "workspace:good", "filePath": good.to_string_lossy(), "workspace": ws.to_string_lossy() }),
        )
        .await
        .expect("plugin.validate 必须可分发");
    assert_eq!(ok["transpiled"], serde_json::json!(true), "{ok}");
    assert_eq!(ok["loaded"], serde_json::json!(true), "{ok}");
    assert_eq!(ok["problems"].as_array().map(|a| a.len()), Some(0), "{ok}");
    assert_eq!(ok["exports"][0]["registered"], serde_json::json!(true), "{ok}");
    // 权限与预算要如实说出来（插件作者据此判断能不能联网）
    assert!(
        ok["needs"].as_str().unwrap_or("").contains("net: 关"),
        "{ok}"
    );
    assert!(ok["limits"].as_str().unwrap_or("").contains("循环"), "{ok}");

    // 2) 坏插件：问题清单说清楚"转译失败"
    let broken = dispatcher
        .dispatch(
            "plugin.validate",
            serde_json::json!({ "pluginId": "workspace:bad", "filePath": bad.to_string_lossy(), "workspace": ws.to_string_lossy() }),
        )
        .await
        .expect("plugin.validate 必须可分发");
    assert_eq!(broken["transpiled"], serde_json::json!(false), "{broken}");
    let problems = broken["problems"].as_array().cloned().unwrap_or_default();
    assert!(
        problems.iter().any(|p| p.as_str().unwrap_or("").contains("转译失败")),
        "{broken}"
    );

    // 3) 按 pluginId 也能找到路径（不给 filePath 时走扫描结果）
    let by_id = dispatcher
        .dispatch(
            "plugin.validate",
            serde_json::json!({ "pluginId": "workspace:good", "workspace": ws.to_string_lossy() }),
        )
        .await
        .expect("按 pluginId 也应当可分发");
    assert_eq!(by_id["loaded"], serde_json::json!(true), "{by_id}");

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&test_dir);
}

/// P4：`plugin.list` 的贡献字段来自**装配期活体视图**，不再是空数组。
///
/// 修之前：`plugin.list` 读 `PluginManager::scan_plugins`（只扫目录、**从不执行插件**），
/// 于是每个磁盘插件的 `tools` / `skills` / `prompts` 恒为空——插件卡的贡献计数永远 0，
/// 而引擎里那些工具明明装上了。这条用例钉住"视图并进来"这条线。
#[tokio::test]
async fn test_plugin_list_merges_live_contribution_views() {
    let ws_dir = std::env::temp_dir().join(format!("a_da_ws_plugin_view_{}", uuid::Uuid::new_v4()));
    write_extension_package(
        &ws_dir,
        "hello_view",
        &["hello_view_tool"],
        &[],
        &[],
        &["afterTurn"],
        "ada.registerTool({ name: 'hello_view_tool', description: '打招呼', parameters: { type: 'object', properties: {} }, execute: async () => 'hello' });",
    );
    let ws = ws_dir.to_string_lossy().to_string();
    let store = Arc::new(RwLock::new(store_for_test(&ws)));
    let home = test_home_dir();

    let views = agent_node::plugins::new_contribution_store(vec![
        agent_node::plugins::PluginContributionView {
            plugin_id: "workspace:hello_view".to_string(),
            tools: vec![agent_node::plugins::PluginToolInfo {
                name: "hello_view_tool".to_string(),
                description: "打招呼".to_string(),
                parameters: Some(serde_json::json!({ "type": "object" })),
                is_write: false,
            }],
            prompts: Vec::new(),
            skills: Vec::new(),
            hooks: vec!["afterTurn".to_string()],
        },
    ]);

    let dispatcher = Dispatcher::new(
        store.clone(),
        Arc::new(SessionManager::new(Some(home.clone()))),
        Arc::new(CheckpointManager::new(Some(home.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(home.clone())),
        Arc::new(SkillManager::with_home(home.clone())),
        None,
    )
    .with_plugin_contribution_views(views);

    let listed = dispatcher
        .dispatch("plugin.list", serde_json::json!({ "workspace": ws }))
        .await
        .expect("plugin.list 必须可分发");
    let items = listed["plugins"].as_array().expect("plugin.list 必须回插件数组");
    let item = items
        .iter()
        .find(|p| p["id"] == "workspace:hello_view")
        .expect("磁盘插件必须出现在列表里");
    assert_eq!(
        item["tools"].as_array().map(|a| a.len()),
        Some(1),
        "工具贡献要由活体视图填上（静态扫描恒为空）：{item}"
    );
    assert_eq!(
        item["hooks"].as_array().map(|a| a.len()),
        Some(1),
        "钩子点位清单要一起给（'这个插件会动手做哪些事'）：{item}"
    );
    assert_eq!(
        item["plugin"]["contributions"]["tools"]
            .as_array()
            .map(|a| a.len()),
        Some(1),
        "声明形状也要一起填（核心工具页的'被插件重实现'读它）：{item}"
    );
    // 内置插件不受视图影响（它的贡献由 builtins.rs 给全）
    let builtin = items
        .iter()
        .find(|p| p["scope"] == "builtin")
        .expect("内置插件也在列表里");
    assert!(
        builtin["hooks"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "内置插件的钩子点位不该被视图改写：{builtin}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&home);
}

/// 守门：**引擎没注入**时，用户必须看到一张提示卡。
///
/// 2026-10-18 真事故：`create_plugin` 声明了 `RollbackPolicy::None`，装配期契约违约 →
/// 产品引擎整个装不上 → 每轮在 `run_agent_turn` 里静默返回 Err（只写日志）。
/// 界面上的表现是「1 轮 / 0 tok / 0s」加一句自己的提问，用户完全不知道发生了什么。
/// 这条用例钉住：这种"一个 AgentEvent 都不会产生"的失败，宿主必须自己补卡片。
#[tokio::test]
async fn test_turn_without_engine_leaves_a_visible_notice() {
    let store = Arc::new(RwLock::new(store_for_test("E:/codes/rust_projects/a_da")));
    let test_dir = std::env::temp_dir().join(format!("a_da_noengine_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&test_dir);
    let session_mgr = Arc::new(SessionManager::new(Some(test_dir.clone())));
    let dispatcher = Dispatcher::new(
        store.clone(),
        session_mgr.clone(),
        Arc::new(CheckpointManager::new(Some(test_dir.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(test_home_dir())),
        Arc::new(SkillManager::with_home(test_home_dir())),
        None, // 故意不注入引擎
    );

    let created = dispatcher
        .dispatch("thread.create", serde_json::json!({ "title": "无引擎" }))
        .await
        .unwrap();
    let tid = created.get("threadId").and_then(|v| v.as_str()).unwrap().to_string();

    dispatcher
        .dispatch("thread.send", serde_json::json!({ "threadId": tid, "text": "你好" }))
        .await
        .unwrap();

    // 回合在后台任务里跑，卡片是异步落进来的
    let mut notice: Option<String> = None;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let s = store.read().await;
        if let Some(t) = s.threads.iter().find(|t| t.id == tid) {
            if let Some(Item::Notice { text, .. }) =
                t.items.iter().find(|i| matches!(i, Item::Notice { .. }))
            {
                notice = Some(text.clone());
                break;
            }
        }
    }

    let notice = notice.expect("引擎没注入时必须落一张提示卡，否则界面就是「发了消息没反应」");
    assert!(
        notice.contains("本轮未能开始"),
        "提示卡要说清这轮为什么没开始：{notice}"
    );
}
