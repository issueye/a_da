//! AGENT RUNTIME 组合根（`docs/agent-base-design.md` v0.2 §7）。
//!
//! 职责：
//! 1. [`spec`] —— 产品规格（`agent.spec.json`）数据结构
//! 2. [`catalog`] —— 组合式工具注册表（装配一次，杜绝每轮扫盘，INV-3/M1-T3）
//! 3. [`builder`] —— [`ProductBuilder`]：唯一允许读取产品声明并装配各端口的地方（INV-8）

pub mod builder;
pub mod catalog;
pub mod spec;

pub use builder::{ProductBuilder, SpecError};
pub use catalog::CompositeToolCatalog;
pub use spec::{AgentSpec, CapabilitySpec, DelegationMode, GatewaySpec, IdentitySpec, PolicySpec};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    use agent_base::domain::{
        Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor, ToolReceipt,
    };
    use agent_base::ports::{BoxFuture, Tool, ToolCatalog, ToolContext};
    use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, MockScope, NeverCancel, RecordingApprovalGate, RecordingSink, ScriptedModelClient};
    use agent_base::engine::TurnRequest;
    use agent_base::model::{ProviderConfig, StreamDelta};

    struct DummyTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for DummyTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        fn execute<'a>(
            &'a self,
            _call: &'a ToolCall,
            _ctx: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async move { ToolReceipt::success("ok", 0, 0) })
        }
    }

    fn make_dummy_tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(DummyTool {
            descriptor: ToolDescriptor {
                name: name.to_string(),
                summary: "dummy".to_string(),
                schema: serde_json::json!({}),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        })
    }

    #[test]
    fn test_catalog_assembled_once_without_repeated_scans() {
        let scan_counter = Arc::new(AtomicUsize::new(0));

        // 初始装配
        let catalog = CompositeToolCatalog::new(
            vec![make_dummy_tool("tool_a"), make_dummy_tool("tool_b")],
            Some(scan_counter.clone()),
        );

        // 装配后计数为 1
        assert_eq!(catalog.scan_count(), 1);

        // 多次查询 descriptors 和 resolve 都不递增扫描计数（常驻内存，杜绝扫盘）
        for _ in 0..10 {
            assert_eq!(catalog.descriptors().len(), 2);
            assert!(catalog.resolve("tool_a").is_ok());
            assert!(catalog.resolve("tool_b").is_ok());
            assert!(catalog.resolve("non_existent").is_err());
        }

        // 扫描计数仍然恒为 1！
        assert_eq!(catalog.scan_count(), 1);

        // 只有显式 reload 时才递增
        catalog.reload(vec![make_dummy_tool("tool_c")]);
        assert_eq!(catalog.scan_count(), 2);
        assert_eq!(catalog.descriptors().len(), 1);
        assert!(catalog.resolve("tool_c").is_ok());
    }

    #[tokio::test]
    async fn test_product_builder_end_to_end() {
        let spec_json = r#"{
            "id": "ada-test",
            "archetype": "coding",
            "identity": {
                "name": "测试助手",
                "persona": "system.md",
                "locale": "zh-CN"
            },
            "toolkits": ["core"],
            "capabilities": {
                "images": false,
                "streaming": true,
                "rollback": false,
                "subagents": false
            },
            "policies": {
                "maxSteps": 5,
                "parallelTools": 1,
                "toolTimeoutSec": 60
            }
        }"#;

        let spec = AgentSpec::from_json_str(spec_json).expect("解析 spec");

        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "你好！".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));

        let builder = ProductBuilder::new(spec)
            .with_tool(make_dummy_tool("echo"))
            .with_model(model)
            .with_approval(Arc::new(RecordingApprovalGate::new(true)))
            .with_store(Arc::new(InMemorySessionStore::new()))
            .with_prompt(Arc::new(FixedPrompt::new("测试人格")))
            .with_scope(Arc::new(MockScope::new("test_scope")))
            .with_clock(Arc::new(FixedClock::new(1000)));

        let runtime = builder.build().expect("装配必须成功");

        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new(
            "thread_01",
            ProviderConfig {
                id: "test".into(),
                name: "test".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
        );

        let outcome = runtime.run_turn(req, &sink, &cancel).await.expect("执行成功");
        assert_eq!(outcome.steps_taken, 1);
    }

    #[test]
    fn test_catalog_validate_across_all_five_consumers() {
        use agent_base::domain::PathSelector;
        use agent_base::ports::Consumer;

        // 1. 合规工具列表通过所有 5 类消费者校验
        let valid_tool = Arc::new(DummyTool {
            descriptor: ToolDescriptor {
                name: "clean_tool".to_string(),
                summary: "一个合格的工具".to_string(),
                schema: serde_json::json!({ "type": "object" }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        });
        let catalog = CompositeToolCatalog::new(vec![valid_tool], None);
        let violations = catalog.validate(&Consumer::ALL);
        assert!(violations.is_empty(), "合规工具不应产生任何违规: {:?}", violations);

        // 2. 变更类工具未声明回滚策略（None），必须被 RollbackPolicy 消费者检出违约
        let invalid_rollback_tool = Arc::new(DummyTool {
            descriptor: ToolDescriptor {
                name: "unsafe_write".to_string(),
                summary: "不安全写入".to_string(),
                schema: serde_json::json!({ "type": "object" }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None, // 违约！
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        });
        let catalog_bad = CompositeToolCatalog::new(vec![invalid_rollback_tool], None);
        let violations = catalog_bad.validate(&[Consumer::RollbackPolicy]);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].consumer, Consumer::RollbackPolicy);
        assert_eq!(violations[0].tool, "unsafe_write");

        // 3. 插件声明不规范（包含空格）与 schema 非 object 违约
        let invalid_decl_tool = Arc::new(DummyTool {
            descriptor: ToolDescriptor {
                name: "bad name with space".to_string(),
                summary: "".to_string(), // 空摘要触发 SubagentAllowlist
                schema: serde_json::json!("not an object"), // 非 object
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        });
        let catalog_decl = CompositeToolCatalog::new(vec![invalid_decl_tool], None);
        let violations = catalog_decl.validate(&[Consumer::PluginDeclaration, Consumer::SubagentAllowlist]);
        assert_eq!(violations.len(), 3); // name + schema + summary
    }
}
