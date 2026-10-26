use super::*;

/// 造一个最小的 `PluginItem`（静态扫描的形状：贡献字段全空）。
fn plugin_item(id: &str, scope: &str) -> agent_node::plugins::PluginItem {
    agent_node::plugins::PluginItem {
        plugin: agent_node::plugins::LoadedPlugin {
            manifest: agent_node::plugins::PluginManifest {
                id: id.to_string(),
                name: id.to_string(),
                description: String::new(),
                version: None,
                author: None,
                scope: None,
            },
            contributions: Default::default(),
            declarative: false,
            status: "ready".to_string(),
            diagnostics: Vec::new(),
        },
        id: id.to_string(),
        name: id.to_string(),
        file_name: id.to_string(),
        file_path: format!("/tmp/{id}/index.ts"),
        scope: scope.to_string(),
        enabled: true,
        status: "ready".to_string(),
        version: None,
        diagnostics: Vec::new(),
        tools: Vec::new(),
        skills: Vec::new(),
        prompts: Vec::new(),
        hooks: Vec::new(),
        is_package: Some(true),
        error: None,
        size_bytes: 0,
        updated_at: 0,
    }
}

/// P4：静态扫描给的贡献恒为空，**装配期的活体视图**才是"实际装上了什么"。
///
/// 没有这一步时，插件卡的贡献计数对每个磁盘插件都显示 0（而引擎里工具明明装上了）。
#[test]
fn test_merge_contribution_views_fills_disk_plugin_contributions() {
    let store = agent_node::plugins::new_contribution_store(vec![
        agent_node::plugins::PluginContributionView {
            plugin_id: "workspace:demo".to_string(),
            tools: vec![agent_node::plugins::PluginToolInfo {
                name: "custom_tool".to_string(),
                description: "自定义工具".to_string(),
                parameters: None,
                is_write: true,
            }],
            prompts: vec![agent_node::plugins::PluginPromptDeclaration {
                id: "style".to_string(),
                content: "一律用 tabs".to_string(),
            }],
            skills: Vec::new(),
            hooks: vec!["beforeToolCall".to_string()],
        },
    ]);
    let mut items = vec![plugin_item("workspace:demo", "workspace")];

    merge_contribution_views(Some(&store), &mut items);
    assert_eq!(
        items[0].tools.len(),
        1,
        "工具贡献要填上：{:?}",
        items[0].tools
    );
    assert!(
        items[0].tools[0].is_write,
        "读写性如实照抄（界面据此显示写徽章）"
    );
    assert_eq!(items[0].prompts.len(), 1);
    assert_eq!(items[0].hooks, vec!["beforeToolCall".to_string()]);
    assert_eq!(
        items[0].plugin.contributions.tools.len(),
        1,
        "声明形状也要一起填（核心工具页的'被插件重实现'读它）"
    );
    assert!(
        !items[0].plugin.contributions.tools[0].parameters.is_null(),
        "缺 parameters 时兜成空对象 schema，而不是 null"
    );

    // 没有视图的插件一个字段都不动（如实为空）
    let mut untouched = vec![plugin_item("global:other", "global")];
    merge_contribution_views(Some(&store), &mut untouched);
    assert!(untouched[0].tools.is_empty() && untouched[0].hooks.is_empty());

    // 没注入视图 → 同样不动（"这台宿主没执行过插件"的真相）
    let mut no_store = vec![plugin_item("workspace:demo", "workspace")];
    merge_contribution_views(None, &mut no_store);
    assert!(no_store[0].tools.is_empty());
}

/// 内置插件的贡献由 `builtins.rs` 给全，**不许被视图覆盖**（视图只讲磁盘插件）。
#[test]
fn test_merge_contribution_views_never_touches_builtin_items() {
    let store = agent_node::plugins::new_contribution_store(vec![
        agent_node::plugins::PluginContributionView {
            plugin_id: "builtin:approval-guard".to_string(),
            tools: vec![agent_node::plugins::PluginToolInfo {
                name: "should_not_appear".to_string(),
                description: String::new(),
                parameters: None,
                is_write: false,
            }],
            prompts: Vec::new(),
            skills: Vec::new(),
            hooks: Vec::new(),
        },
    ]);
    let mut items = vec![plugin_item("builtin:approval-guard", "builtin")];
    merge_contribution_views(Some(&store), &mut items);
    assert!(
        items[0].tools.is_empty(),
        "内置插件的贡献字段不许被视图改写：{:?}",
        items[0].tools
    );
}

/// 图片归一化：data URI 原样通过。
#[test]
fn test_normalize_images_accepts_data_uri_as_is() {
    let out = normalize_images(Some(vec!["data:image/png;base64,AAA".to_string()]));
    assert_eq!(out, Some(vec!["data:image/png;base64,AAA".to_string()]));
}

/// 空的、读不到的——一律归成"没有图片"，**绝不把裸路径原样送进模型请求**。
///
/// 这条归一化就是**零 IO 边界**本身：`agent-base` 读不了盘，路径形态的图片必须在
/// RPC 入口被消灭掉，否则模型只会收到一个它取不到的路径（错误还很难看懂）。
#[test]
fn test_normalize_images_drops_empty_and_unreadable() {
    assert_eq!(normalize_images(None), None);
    assert_eq!(normalize_images(Some(vec![])), None);
    assert_eq!(normalize_images(Some(vec!["   ".to_string()])), None);
    assert_eq!(
        normalize_images(Some(vec!["E:/definitely/not/here_9f3a.png".to_string()])),
        None
    );
}

/// 裸路径（老盘上恢复出来的会话）读盘转成 data URI，mime 按扩展名判。
#[test]
fn test_normalize_images_reads_a_real_file_as_data_uri() {
    let dir = std::env::temp_dir().join(format!("a_da_img_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("shot.png");
    std::fs::write(&path, [0x89u8, 0x50, 0x4E, 0x47]).expect("写临时图片");

    let out = normalize_images(Some(vec![path.to_string_lossy().to_string()]))
        .expect("路径应当被读成 data URI");
    assert!(
        out[0].starts_with("data:image/png;base64,"),
        "应按扩展名判出 image/png：{}",
        out[0]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 守门：压缩必须**真的落盘 compact 条目**、留下最近几项、并用**真实**的上下文规模记账。
///
/// 早先 `thread.compact` 只写一句"已对前序 N 轮…进行了结构化压缩与归纳"——假装总结过，
/// 而且 pre/post tokens 是按"每项 450 tokens"编的。现在摘要由模型生成（测试注入），
/// 规模取"最近一次模型调用如实报回的 prompt_tokens"。
#[tokio::test]
async fn test_compact_writes_entry_and_keeps_recent_items() {
    let dir = std::env::temp_dir().join(format!("a_da_compact_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    let session_mgr = SessionManager::new(Some(dir.clone()));
    let ws = "E:/codes/compact_ws";

    let store = Arc::new(RwLock::new(AgentStore::new(ws.to_string())));
    let tid = {
        let mut s = store.write().await;
        let id = s.create_thread(Some("压缩测试".to_string()), None);
        // 造 6 项历史，最后一项带**真实用量**（445k 是实测那次撞顶的规模）
        s.add_user_message(&id, "任务：把项目跑起来");
        for i in 0..4 {
            s.start_tool_call(&id, &format!("call_{i}"), "read_file", "{}");
            s.finish_tool_call(
                &id,
                &format!("call_{i}"),
                true,
                Some(format!("输出 {i}")),
                Some(1),
                Some(0),
                Some(1),
                None,
            );
        }
        s.append_assistant_delta(&id, "最后一段回复");
        s.set_assistant_stats(
            &id,
            Some(agent_base::model::TokenUsage {
                prompt_tokens: 445_080,
                completion_tokens: 557,
                total_tokens: 445_637,
                ..Default::default()
            }),
            10,
            20,
        );
        let _ = session_mgr.create_session(&id, ws, Some("压缩测试"), None, None);
        id
    };

    let summary = compact_thread_with_summary(
        &store,
        &session_mgr,
        &tid,
        Some("## 摘要\n- 目标：跑起来".to_string()),
    )
    .await
    .expect("历史足够，必须压缩成功");
    assert!(summary.contains("摘要"));

    {
        let s = store.read().await;
        let t = s.threads.iter().find(|t| t.id == tid).expect("会话还在");
        assert!(
            matches!(t.items.first(), Some(Item::Compact { .. })),
            "首项必须是压缩卡片"
        );
        let compact = match t.items.first() {
            Some(Item::Compact {
                pre_tokens,
                turns_summarized,
                ..
            }) => (*pre_tokens, *turns_summarized),
            _ => unreachable!(),
        };
        assert_eq!(
            compact.0, 445_080,
            "pre_tokens 必须取真实用量，不是编的 450/项"
        );
        assert!(compact.1 > 0, "必须如实记录压缩了几项");
        assert!(t.items.len() < 7, "压缩后项数必须变少：{}", t.items.len());
    }

    // 落盘：最后一行必须是 compact 条目（引擎恢复上下文时靠它折叠）
    let path = session_mgr.get_session_path(ws, &tid);
    let content = std::fs::read_to_string(&path).expect("会话文件应存在");
    let last = content.lines().last().unwrap_or("");
    assert!(
        last.contains("\"type\":\"compact\""),
        "必须落盘 compact 条目：{last}"
    );
    assert!(last.contains("摘要"), "落盘的条目里要带真实摘要：{last}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 历史太短 → **如实返回 None**（调用方据此回"无需压缩"），不假装压缩成功。
#[tokio::test]
async fn test_compact_returns_none_for_short_history() {
    let dir = std::env::temp_dir().join(format!("a_da_compact_short_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    let session_mgr = SessionManager::new(Some(dir.clone()));
    let store = Arc::new(RwLock::new(AgentStore::new(
        "E:/codes/short_ws".to_string(),
    )));
    let tid = {
        let mut s = store.write().await;
        let id = s.create_thread(Some("短会话".to_string()), None);
        s.add_user_message(&id, "你好");
        id
    };
    assert!(
        compact_thread_with_summary(&store, &session_mgr, &tid, Some("x".into()))
            .await
            .is_none(),
        "历史太短必须如实返回 None"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── W6-T4：回滚回执必须带真实文件清单 ────────────────────────────────────

#[test]
fn test_revert_response_carries_the_restored_file_list() {
    let outcome = agent_node::checkpoint::RevertOutcome {
        restored: vec!["a.rs".to_string(), "b.rs".to_string()],
        deleted: vec!["c.tmp".to_string()],
        skipped: vec!["d.bin".to_string()],
        invalidated: vec!["ck_1".to_string()],
    };
    let v = revert_response(Some(outcome));

    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    let restored: Vec<&str> = v
        .get("restored")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(restored, vec!["a.rs", "b.rs"], "必须回传恢复的文件清单");
    assert_eq!(v.get("restoredCount").and_then(|c| c.as_u64()), Some(2));
    assert_eq!(v.get("deletedCount").and_then(|c| c.as_u64()), Some(1));
}

/// 没有检查点 → **如实说没回滚**，不许回 `{ok:true}` 骗界面。
#[test]
fn test_revert_response_reports_nothing_reverted() {
    let v = revert_response(None);
    assert_eq!(
        v.get("ok").and_then(|b| b.as_bool()),
        Some(false),
        "什么都没回滚时不得报 ok"
    );
    assert!(
        v.get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .contains("什么都没回滚"),
        "{v}"
    );
    assert_eq!(v.get("restoredCount").and_then(|c| c.as_u64()), Some(0));
}

// ── W6-T4：抗体——"把事件抽干丢掉"的循环不许再出现 ──────────────────────

/// 编辑重发原先自己 spawn 一个空循环把 `event_rx` 抽干丢弃，
/// 于是界面看不到任何流式输出。这条断言钉住那个形态不再出现。
///
/// 为什么用源码断言：它是**结构性缺陷**（"事件被丢掉"），
/// 而行为断言只能在有引擎+模型时才能观察到；源码断言在毫秒内守住这条线。
///
/// 只扫**生产段**：`include_str!` 会把本测试自身的字面量也算进去，
/// 不切掉测试段的话断言会自己把自己判红（踩过一次）。
#[test]
fn test_no_event_discarding_drain_loop() {
    // 生产代码已按职责拆到 `mod.rs` / `thread_loop.rs`；两处都要扫。
    // 测试体本身在 `tests.rs`，不在 `include_str!` 的范围里，所以不需要再切测试段。
    let prod = concat!(include_str!("mod.rs"), "\n", include_str!("thread_loop.rs"));

    let discarded_events = prod.contains("while let Some(_event) = event_rx");
    let discarded_events_short = prod.contains("while let Some(_e) = event_rx");
    assert!(
        !discarded_events && !discarded_events_short,
        "不得再有把事件抽干丢弃的循环（W6-T4 修掉的缺陷）"
    );

    // 反向：编辑重发必须走共用执行泵
    assert!(
        prod.contains("self.spawn_thread_loop("),
        "编辑重发必须与 thread.start 共用同一条执行泵"
    );
}

/// **W3-T6 出口判据**：`session.initialize` 的能力位与产品身份必须
/// **从产品声明派生**，而不是硬编码。
///
/// 用 `ada-skeleton` 的声明做样本：它声明 `rollback: false` 且**没有** `plugins`，
/// 而 `ServerCapabilities::default()` 是 `rollback: true, plugins: true`——
/// 所以"派生"与"硬编码"在这份声明上**结果不同**，断言才有意义。
#[tokio::test]
async fn test_handshake_capabilities_come_from_the_product_declaration() {
    let spec = agent_runtime::AgentSpec::from_json_str(
            r#"{
            "id": "ada-skeleton",
            "archetype": "assistant",
            "identity": { "name": "骨架助手", "persona": "最小骨架测试助手。", "locale": "zh-CN" },
            "toolkits": ["core"],
            "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
            "policies": { "maxSteps": 10, "parallelTools": 1, "toolTimeoutSec": 30 }
        }"#,
        )
        .expect("骨架声明应可解析");

    let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(SessionManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(CheckpointManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::new()),
        Arc::new(SkillManager::new()),
        None,
    )
    .with_product_spec(Arc::new(spec));

    let res = dispatcher
        .dispatch(
            "session.initialize",
            serde_json::json!({
                "protocolVersion": "1.0",
                "client": { "name": "test", "version": "0.0.1", "platform": "test" }
            }),
        )
        .await
        .expect("握手应成功");

    let caps = res.get("capabilities").expect("应有 capabilities");
    assert_eq!(
        caps.get("rollback").and_then(|v| v.as_bool()),
        Some(false),
        "声明 rollback=false → 握手必须回 false（硬编码的 default 是 true）"
    );
    assert_eq!(
        caps.get("plugins").and_then(|v| v.as_bool()),
        Some(false),
        "声明里没有 plugins → 必须回 false（硬编码的 default 是 true）"
    );
    assert_eq!(caps.get("images").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        caps.get("hooks").and_then(|v| v.as_bool()),
        Some(false),
        "骨架产品**没有**声明 hooks → 必须如实回 false（能力位从声明派生，不是恒真）"
    );

    // 产品身份同样来自声明
    let product = res.get("product").expect("应有 product");
    assert_eq!(
        product.get("id").and_then(|v| v.as_str()),
        Some("ada-skeleton")
    );
    assert_eq!(
        product.get("name").and_then(|v| v.as_str()),
        Some("骨架助手")
    );
    assert_eq!(
        product.get("archetype").and_then(|v| v.as_str()),
        Some("assistant")
    );
}

/// 没注入声明时退回默认值（老调用点/测试的兼容路径）。
#[tokio::test]
async fn test_handshake_without_spec_falls_back_to_defaults() {
    let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(SessionManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(CheckpointManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::new()),
        Arc::new(SkillManager::new()),
        None,
    );

    let res = dispatcher
        .dispatch(
            "session.initialize",
            serde_json::json!({
                "protocolVersion": "1.0",
                "client": { "name": "test", "version": "0.0.1", "platform": "test" }
            }),
        )
        .await
        .expect("握手应成功");

    let caps = res.get("capabilities").expect("应有 capabilities");
    assert_eq!(
        caps.get("rollback").and_then(|v| v.as_bool()),
        Some(true),
        "无声明时退回 default（rollback: true）"
    );
    assert!(res.get("product").is_none(), "无声明时不捏造产品身份");
}

#[test]
fn test_thread_workspace_follows_thread_binding() {
    let mut store = AgentStore::new("E:/codes/default_ws".to_string());

    // 会话自带工作区时，全局当前工作区再怎么变都不影响它
    let bound = store.create_thread(
        Some("绑定会话".to_string()),
        Some("E:/codes/fpc_projects".to_string()),
    );
    store.workspace.project = "E:/codes/rust_projects/a_da/target/release".to_string();
    assert_eq!(thread_workspace(&store, &bound), "E:/codes/fpc_projects");

    // 只有历史遗留的空工作区会话，才退回当前工作区
    let legacy = store.create_thread(Some("遗留会话".to_string()), None);
    if let Some(t) = store.threads.iter_mut().find(|t| t.id == legacy) {
        t.workspace.clear();
    }
    assert_eq!(
        thread_workspace(&store, &legacy),
        "E:/codes/rust_projects/a_da/target/release"
    );

    // 会话不存在时退回当前工作区，当前工作区也空则退回公共区
    assert_eq!(
        thread_workspace(&store, "thread_不存在"),
        "E:/codes/rust_projects/a_da/target/release"
    );
    store.workspace.project.clear();
    let public = store.public_workspace.clone();
    assert_eq!(thread_workspace(&store, "thread_不存在"), public);
}

#[test]
fn test_assistant_stats_lands_on_current_card() {
    use agent_base::model::TokenUsage;

    let mut store = AgentStore::new("E:/codes/default_ws".to_string());
    let tid = store.create_thread(Some("遥测".to_string()), None);

    let usage = TokenUsage {
        prompt_tokens: 1234,
        completion_tokens: 56,
        total_tokens: 1290,
        thinking_tokens: None,
        cached_tokens: Some(1000),
    };

    // 1. 本步产出了文本：用量与耗时挂在当前流式卡片上
    store.append_assistant_delta(&tid, "答案");
    store.set_assistant_stats(&tid, Some(usage.clone()), 2500, 4100);
    match store
        .threads
        .iter()
        .find(|t| t.id == tid)
        .unwrap()
        .items
        .last()
        .unwrap()
    {
        Item::Assistant {
            text,
            usage: u,
            duration_ms,
            turn_duration_ms,
            ..
        } => {
            assert_eq!(text, "答案");
            assert_eq!(u.as_ref().map(|u| u.prompt_tokens), Some(1234));
            assert_eq!(u.as_ref().and_then(|u| u.cached_tokens), Some(1000));
            assert_eq!(*duration_ms, Some(2500));
            assert_eq!(*turn_duration_ms, Some(4100));
        }
        other => panic!("应为助手卡片，实际: {other:?}"),
    }

    // 2. 本步只调了工具（没有文本卡片）：补一张隐藏卡片承载用量，
    //    否则遥测条会停在上一轮的旧数字上
    store.start_tool_call(&tid, "call_1", "read_file", "{}");
    store.set_assistant_stats(&tid, Some(usage), 900, 5000);
    match store
        .threads
        .iter()
        .find(|t| t.id == tid)
        .unwrap()
        .items
        .last()
        .unwrap()
    {
        Item::Assistant {
            text,
            usage: u,
            duration_ms,
            ..
        } => {
            assert!(text.is_empty(), "承载用量的补位卡片不带正文");
            assert_eq!(u.as_ref().map(|u| u.completion_tokens), Some(56));
            assert_eq!(*duration_ms, Some(900));
        }
        other => panic!("应为承载用量的助手卡片，实际: {other:?}"),
    }
}

/// W2-T5 守门：内置工具目录必须与 `ToolDescriptor` 注册表**逐名相等**。
///
/// 这是第 6 张名单的**行为断言**：目录里多一个不存在的工具（界面广告假工具）
/// 或少一个真实工具（用户看不到）都必须在这里红。结构侧的对应检查在
/// `cargo xtask verify-wiring` 的审计 B（确保这一段始终是**派生**的）。
#[tokio::test]
async fn test_builtin_catalog_matches_registry_exactly() {
    use std::collections::BTreeSet;

    let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new(
        "E:/codes/default_ws".to_string(),
    )));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(agent_node::session::SessionManager::new(Some(
            std::path::PathBuf::from("E:/codes/default_ws"),
        ))),
        Arc::new(agent_node::checkpoint::CheckpointManager::new(None)),
        Arc::new(agent_node::subagents::SubagentManager::new()),
        Arc::new(agent_node::approval::ApprovalManager::new()),
        Arc::new(agent_node::plugins::PluginManager::new()),
        Arc::new(agent_node::skills::SkillManager::new()),
        None,
    );

    let catalog = dispatcher
        .dispatch(
            agent_proto::methods::PLUGIN_BUILTIN_CATALOG,
            serde_json::json!({}),
        )
        .await
        .expect("plugin.builtinCatalog 分发失败");

    let items = catalog.as_array().expect("目录必须是数组");
    let got: BTreeSet<String> = items
        .iter()
        .map(|v| v["name"].as_str().expect("每条必须有 name").to_string())
        .collect();
    let expected: BTreeSet<String> = agent_toolkit::registry::standard_tool_descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect();

    assert_eq!(
        got, expected,
        "内置工具目录必须与注册表逐名相等（不许是第二张名单）"
    );

    // 界面需要的四个字段都要在，且 isReadOnly 必须与描述符一致
    for item in items {
        for field in ["name", "label", "description", "isReadOnly"] {
            assert!(!item[field].is_null(), "条目缺少字段 {field}: {item}");
        }
        let name = item["name"].as_str().unwrap();
        let desc = agent_toolkit::registry::find_tool_descriptor(name)
            .unwrap_or_else(|| panic!("目录里的 `{name}` 必须在注册表里"));
        assert_eq!(
            item["isReadOnly"].as_bool().unwrap(),
            desc.is_readonly(),
            "`{name}` 的只读性与描述符不一致"
        );
    }
}

#[tokio::test]
async fn test_approval_decide_resolves_pending_question() {
    let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new(
        "E:/codes/default_ws".to_string(),
    )));
    let session_mgr = Arc::new(agent_node::session::SessionManager::new(Some(
        std::path::PathBuf::from("E:/codes/default_ws"),
    )));
    let checkpoint_mgr = Arc::new(agent_node::checkpoint::CheckpointManager::new(None));
    let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
    let approval_mgr = Arc::new(agent_node::approval::ApprovalManager::new());
    let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::new());
    let skill_mgr = Arc::new(agent_node::skills::SkillManager::new());

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

    let call_id = "test_ask_approval_fallback";
    let rx = agent_node::approval::global_question_manager().register_waiter(call_id);
    assert!(agent_node::approval::global_question_manager().has_pending(call_id));

    // 模拟前端调用 approval.decide
    let req = serde_json::json!({
        "toolItemId": call_id,
        "approved": true
    });

    let res = dispatcher
        .dispatch(agent_proto::methods::APPROVAL_DECIDE, req)
        .await
        .expect("调用应该成功");
    assert_eq!(res.get("resolved").and_then(|v| v.as_bool()), Some(true));

    let ans = rx.await.expect("应该唤醒等待者");
    assert_eq!(ans.answered_by, "user");
    assert!(!agent_node::approval::global_question_manager().has_pending(call_id));
}

#[tokio::test]
async fn test_session_initialize_returns_capabilities_and_snapshot_has_seq() {
    let store = Arc::new(tokio::sync::RwLock::new(AgentStore::new(
        "E:/codes/default_ws".to_string(),
    )));
    let session_mgr = Arc::new(agent_node::session::SessionManager::new(None));
    let checkpoint_mgr = Arc::new(agent_node::checkpoint::CheckpointManager::new(None));
    let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
    let approval_mgr = Arc::new(agent_node::approval::ApprovalManager::new());
    let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::new());
    let skill_mgr = Arc::new(agent_node::skills::SkillManager::new());

    let dispatcher = Dispatcher::new(
        store,
        session_mgr,
        checkpoint_mgr,
        subagent_mgr,
        approval_mgr,
        plugin_mgr,
        skill_mgr,
        None,
    )
    // W3-T6：产品身份与能力位**从声明派生**。这条测试原先断言的是
    // 硬编码常量（`ada-coding` / `a_da 编程助手`），所以它无法发现
    // "声明改了、握手没跟上"。现在注入真实声明再断言。
    .with_product_spec(std::sync::Arc::new(
        agent_runtime::AgentSpec::from_json_str(include_str!(
            "../../../../../products/ada-coding/agent.spec.json"
        ))
        .expect("ada-coding 声明应可解析"),
    ));

    // 1. 测试 session.initialize 能力位握手返回（M3-T4）
    let init_params = serde_json::json!({
        "protocolVersion": "1.0"
    });
    let init_val = dispatcher
        .dispatch(agent_proto::methods::SESSION_INITIALIZE, init_params)
        .await
        .expect("握手成功");

    let caps = init_val
        .get("capabilities")
        .expect("必须包含 capabilities 节点");
    // ada-coding 声明 images=true（多模态已接通：dispatch → engine_bridge → TurnRequest
    // → 适配器请求体），带头的是产品声明本身——声明改了握手没跟上，这里就会红。
    assert_eq!(
        caps.get("images").and_then(|v| v.as_bool()),
        Some(true),
        "ada-coding 声明 images=true"
    );
    assert_eq!(caps.get("rollback").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(caps.get("plugins").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        caps.get("hooks").and_then(|v| v.as_bool()),
        Some(true),
        "P3a 起 ada-coding 声明 hooks=true（引擎有生产调用点 + 成对埋点 + 缺席可见用例）"
    );
    let evts = caps.get("events").expect("必须包含 events 能力");
    assert_eq!(
        evts.get("snapshotSeq").and_then(|v| v.as_bool()),
        Some(true)
    );

    let prod = init_val.get("product").expect("必须包含 product 节点");
    assert_eq!(prod.get("id").and_then(|v| v.as_str()), Some("ada-coding"));
    assert_eq!(
        prod.get("archetype").and_then(|v| v.as_str()),
        Some("coding")
    );
    assert_eq!(
        prod.get("name").and_then(|v| v.as_str()),
        Some("a_da 编程助手")
    );

    // 2. 测试 session.snapshot 包含 seq 序号（M3-T2）
    let snap_val = dispatcher
        .dispatch(
            agent_proto::methods::SESSION_SNAPSHOT,
            serde_json::json!({}),
        )
        .await
        .expect("拉取快照成功");

    assert!(snap_val.get("seq").is_some(), "快照必须携带 seq 字段");
    assert_eq!(snap_val.get("seq").and_then(|v| v.as_u64()), Some(0));
}

/// 回归：插件重载必须让**按工作区缓存的动态引擎**失效。
///
/// 用户症状：新加插件后点「重新加载」，插件列表里有、模型却调不到；重启程序才恢复。
/// 根因是 `plugin.reload` 只重装配启动工作区那份主引擎，而 `workspace_engines`
/// 里的动态引擎仍是旧工具集合。清缓存后下一轮会按最新磁盘重建。
#[tokio::test]
async fn test_plugin_reload_invalidates_workspace_engine_caches() {
    use agent_base::engine::{AgentRuntime, RunPolicy};
    use agent_base::ports::ToolCatalog;
    use agent_base::testing::{
        FixedClock, FixedPrompt, InMemorySessionStore, InMemoryToolCatalog, MockScope,
        RecordingApprovalGate, ScriptedModelClient,
    };
    use std::path::PathBuf;

    let catalog: Arc<dyn ToolCatalog> = Arc::new(InMemoryToolCatalog::with_tools(vec![]));
    let runtime = Arc::new(AgentRuntime::new(
        Arc::new(ScriptedModelClient::new(vec![])),
        catalog,
        Arc::new(RecordingApprovalGate::new(true)),
        Arc::new(InMemorySessionStore::new()),
        Arc::new(FixedPrompt::new("test")),
        Arc::new(MockScope::new("test")),
        Arc::new(FixedClock::new(1000)),
        RunPolicy::default(),
    ));

    let store = Arc::new(RwLock::new(AgentStore::new("E:/test".to_string())));
    let dispatcher = Dispatcher::new(
        store,
        Arc::new(SessionManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(CheckpointManager::new(Some(
            std::env::temp_dir().join("a_da_test_home"),
        ))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::new()),
        Arc::new(SkillManager::new()),
        None,
    );

    let ws = PathBuf::from("E:/active");
    dispatcher
        .workspace_engines
        .lock()
        .await
        .insert(ws.clone(), runtime.clone());
    dispatcher
        .pm_workspace_engines
        .lock()
        .await
        .insert(ws, runtime);

    dispatcher
        .dispatch(
            agent_proto::methods::PLUGIN_RELOAD,
            serde_json::json!({ "workspace": "E:/active" }),
        )
        .await
        .expect("plugin.reload 应可分发");

    assert!(
        dispatcher.workspace_engines.lock().await.is_empty(),
        "重载后动态编码引擎缓存必须清空"
    );
    assert!(
        dispatcher.pm_workspace_engines.lock().await.is_empty(),
        "重载后动态 PM 引擎缓存必须清空"
    );
}

#[tokio::test]
async fn test_task_list_and_kill_use_the_injected_background_store() {
    let home = std::env::temp_dir().join(format!(
        "a_da_task_dispatch_{}",
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::create_dir_all(&home);
    let workspace = home.join("ws");
    let _ = std::fs::create_dir_all(&workspace);

    let tasks = Arc::new(agent_toolkit::background::BackgroundTasks::new(
        workspace.clone(),
    ));
    let task_id = tasks
        .start_in_thread("echo task-dispatch", None, "thread-1")
        .expect("后台任务应能启动");
    let dispatcher = Dispatcher::new(
        Arc::new(RwLock::new(AgentStore::new(
            workspace.to_string_lossy().to_string(),
        ))),
        Arc::new(SessionManager::new(Some(home.clone()))),
        Arc::new(CheckpointManager::new(Some(home.clone()))),
        Arc::new(SubagentManager::new()),
        Arc::new(ApprovalManager::new()),
        Arc::new(PluginManager::with_home(home.clone())),
        Arc::new(SkillManager::with_home(home.clone())),
        None,
    )
    .with_background_tasks(tasks.clone());

    let listed = dispatcher
        .dispatch(TASK_LIST, serde_json::json!({ "threadId": "thread-1" }))
        .await
        .expect("task.list 应可分发");
    assert_eq!(listed["count"], 1, "{listed}");
    assert_eq!(listed["tasks"][0]["id"], task_id);
    assert_eq!(listed["tasks"][0]["threadId"], "thread-1");

    let killed = dispatcher
        .dispatch(TASK_KILL, serde_json::json!({ "taskId": task_id }))
        .await
        .expect("task.kill 应可分发");
    assert_eq!(killed["ok"], true, "{killed}");

    let _ = std::fs::remove_dir_all(&home);
}
