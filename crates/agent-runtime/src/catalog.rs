//! 工具注册表（INV-3、M1-T3）：组合根装配一次，杜绝每轮扫盘。
//!
//! 现状（计划 §1.3）：`prompt.rs` 与 `executor.rs` 每一轮甚至每一次工具调用都重新
//! 扫描磁盘插件；端口化后，注册表装配后常驻内存，仅在插件增删/作用域变化时显式刷新。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use agent_base::domain::ToolDescriptor;
use agent_base::ports::{Consumer, ContractViolation, Tool, ToolCatalog, ToolError};

/// 组合式工具注册表：线程安全、常驻内存、零重复扫盘。
pub struct CompositeToolCatalog {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    descriptors: RwLock<Vec<ToolDescriptor>>,
    /// 扫描/装配次数计数器（供测试断言"装配只发生一次"）
    scan_count: Arc<AtomicUsize>,
}

impl CompositeToolCatalog {
    /// 构造新的注册表，装配初始工具列表。
    pub fn new(initial_tools: Vec<Arc<dyn Tool>>, scan_count: Option<Arc<AtomicUsize>>) -> Self {
        let counter = scan_count.unwrap_or_else(|| Arc::new(AtomicUsize::new(0)));
        counter.fetch_add(1, Ordering::SeqCst);

        let mut map = HashMap::new();
        let mut descs = Vec::new();

        for t in initial_tools {
            let desc = t.descriptor().clone();
            map.insert(desc.name.clone(), t);
            descs.push(desc);
        }

        Self {
            tools: RwLock::new(map),
            descriptors: RwLock::new(descs),
            scan_count: counter,
        }
    }

    /// 注册新工具（例如热插拔插件加载时）。
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let desc = tool.descriptor().clone();
        let mut tools = self.tools.write().expect("lock tools");
        let mut descs = self.descriptors.write().expect("lock descriptors");

        tools.insert(desc.name.clone(), tool);
        if let Some(pos) = descs.iter().position(|d| d.name == desc.name) {
            descs[pos] = desc;
        } else {
            descs.push(desc);
        }
    }

    /// 移除已注册工具（例如插件卸载时）。
    pub fn unregister(&self, name: &str) {
        let mut tools = self.tools.write().expect("lock tools");
        let mut descs = self.descriptors.write().expect("lock descriptors");

        tools.remove(name);
        descs.retain(|d| d.name != name);
    }

    /// 重新加载全部工具（仅在作用域变更或外部重载事件时显式调用）。
    pub fn reload(&self, fresh_tools: Vec<Arc<dyn Tool>>) {
        self.scan_count.fetch_add(1, Ordering::SeqCst);
        let mut tools = self.tools.write().expect("lock tools");
        let mut descs = self.descriptors.write().expect("lock descriptors");

        tools.clear();
        descs.clear();

        for t in fresh_tools {
            let desc = t.descriptor().clone();
            tools.insert(desc.name.clone(), t);
            descs.push(desc);
        }
    }

    /// 获取历史累计的扫描/刷新次数。
    pub fn scan_count(&self) -> usize {
        self.scan_count.load(Ordering::SeqCst)
    }
}

impl ToolCatalog for CompositeToolCatalog {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.descriptors.read().expect("lock descriptors").clone()
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Tool>, ToolError> {
        self.tools
            .read()
            .expect("lock tools")
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::Unknown(name.to_string()))
    }

    fn validate(&self, consumers: &[Consumer]) -> Vec<ContractViolation> {
        let descs = self.descriptors();
        let mut violations = Vec::new();

        for d in &descs {
            for c in consumers {
                match c {
                    Consumer::ReadonlyFilter => {
                        match &d.access {
                            agent_base::domain::Access::ReadOnly => {}
                            agent_base::domain::Access::Mutates { paths } => match paths {
                                agent_base::domain::PathSelector::Single(field)
                                    if field.trim().is_empty() =>
                                {
                                    violations.push(ContractViolation {
                                        consumer: *c,
                                        tool: d.name.clone(),
                                        detail: "写工具 Single 路径字段名不能为空".to_string(),
                                    });
                                }
                                agent_base::domain::PathSelector::Batch(field)
                                    if field.trim().is_empty() =>
                                {
                                    violations.push(ContractViolation {
                                        consumer: *c,
                                        tool: d.name.clone(),
                                        detail: "写工具 Batch 路径列表字段名不能为空".to_string(),
                                    });
                                }
                                _ => {}
                            },
                            agent_base::domain::Access::Executes { command_arg } => {
                                if command_arg.trim().is_empty() {
                                    violations.push(ContractViolation {
                                        consumer: *c,
                                        tool: d.name.clone(),
                                        detail: "执行工具命令字段名不能为空".to_string(),
                                    });
                                }
                            }
                            // 外部工具（MCP server / 远端 / 第三方脚本）：参数里没有工作区路径
                            // 可校验——它的副作用不在这个坐标系里。只读性由 `is_readonly()`
                            // 保守判为“写”（见 `Access::External` 的文档）。
                            agent_base::domain::Access::External => {}
                        }
                    }
                    Consumer::ApprovalPolicy => {
                        if let agent_base::domain::ApprovalPolicy::DangerScan { patterns } = &d.approval {
                            if patterns.is_empty() {
                                violations.push(ContractViolation {
                                    consumer: *c,
                                    tool: d.name.clone(),
                                    detail: "DangerScan 审批策略未配置任何危险匹配模式".to_string(),
                                });
                            }
                        }
                    }
                    Consumer::RollbackPolicy => {
                        if matches!(d.access, agent_base::domain::Access::Mutates { .. })
                            && d.rollback == agent_base::domain::RollbackPolicy::None
                        {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "变更类工具未声明回滚策略（不得为 None）".to_string(),
                            });
                        }
                        if d.access == agent_base::domain::Access::ReadOnly
                            && d.rollback != agent_base::domain::RollbackPolicy::None
                        {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "只读工具不应声明写回滚策略".to_string(),
                            });
                        }
                        // 外部工具**不得**声称可回滚：本地没有它的路径记录，声称了就是
                        // “能撤销”的假承诺（早先插件工具谎称 `Mutates { Batch("paths") }`
                        // 正是为了绕过上面那条校验，见 `Access::External`）。
                        if d.access == agent_base::domain::Access::External
                            && d.rollback != agent_base::domain::RollbackPolicy::None
                        {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "外部工具无法在本地回滚，回滚策略必须为 None".to_string(),
                            });
                        }
                    }
                    Consumer::SubagentAllowlist => {
                        if d.summary.trim().is_empty() {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "工具摘要为空，子智能体无法进行语义选择".to_string(),
                            });
                        }
                    }
                    Consumer::PluginDeclaration => {
                        if d.name.trim().is_empty() || d.name.contains(' ') {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "工具名称不符合命名规范（不能为空且不得包含空格）".to_string(),
                            });
                        }
                        if !d.schema.is_object() {
                            violations.push(ContractViolation {
                                consumer: *c,
                                tool: d.name.clone(),
                                detail: "工具参数 schema 必须为 JSON Object".to_string(),
                            });
                        }
                    }
                }
            }
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{ToolCall, ToolReceipt};
    use agent_base::ports::{BoxFuture, ToolContext};

    /// 只带描述符、不做实事的工具：契约校验只看描述符，执行体无关。
    struct DescriptorOnly(ToolDescriptor);

    impl Tool for DescriptorOnly {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.0
        }

        fn execute<'a>(
            &'a self,
            _call: &'a ToolCall,
            _ctx: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async move { ToolReceipt::success("ok", 0, 0) })
        }
    }

    /// 🔴 全量契约守门：**注册表里每一个工具**都必须通过全部消费者契约。
    ///
    /// 2026-10-18 真事故：`create_plugin` 声明了 `Access::Mutates` + `RollbackPolicy::None`。
    /// 这类违约只在**引擎装配期**（`EngineFactory` 调 `catalog.validate`）才被发现，
    /// 于是产品引擎整个装不上——界面上的表现是"发消息没有任何反应"（1 轮 / 0 tok / 0s），
    /// 而 `cargo test`、`xtask verify-spec`、`verify-wiring` 当时全绿：没人把**整张注册表**
    /// 喂给 `validate`。这条用例补的就是这个缺口——新增工具时第一时间红，而不是等用户发消息。
    #[test]
    fn test_every_registered_tool_passes_all_consumer_contracts() {
        let tools: Vec<Arc<dyn Tool>> = agent_toolkit::registry::standard_tool_descriptors()
            .iter()
            .cloned()
            .map(|d| Arc::new(DescriptorOnly(d)) as Arc<dyn Tool>)
            .collect();
        assert!(!tools.is_empty(), "注册表不能为空");

        let catalog = CompositeToolCatalog::new(tools, None);
        let violations = catalog.validate(&Consumer::ALL);
        assert!(
            violations.is_empty(),
            "注册表里有工具违反消费者契约（装配期才会炸，界面表现是「发消息没反应」）：\n{violations:#?}"
        );
    }

    /// `External` 刻画的是"作用域不可描述"：它既不该被路径校验误伤，也不许声称可回滚。
    ///
    /// 这条正是插件工具改建 `Access::External` 的动机：早先它谎称
    /// `Mutates { Batch("paths") }` + `PerTargetInBatch` —— 只为通过校验，
    /// 而它的 schema 里根本没有 `paths` 字段，回滚记录会去找一个不存在的东西。
    fn external_descriptor(rollback: agent_base::domain::RollbackPolicy) -> ToolDescriptor {
        ToolDescriptor {
            name: "mcp__demo__do".to_string(),
            summary: "外部工具".to_string(),
            schema: serde_json::json!({ "type": "object" }),
            access: agent_base::domain::Access::External,
            approval: agent_base::domain::ApprovalPolicy::Always,
            rollback,
            execution: agent_base::domain::Execution::Sequential,
            termination: agent_base::domain::Termination::ContinueTurn,
        }
    }

    #[test]
    fn test_external_tool_must_not_claim_rollback() {
        let ok = CompositeToolCatalog::new(
            vec![Arc::new(DescriptorOnly(external_descriptor(
                agent_base::domain::RollbackPolicy::None,
            ))) as Arc<dyn Tool>],
            None,
        );
        assert!(
            ok.validate(&Consumer::ALL).is_empty(),
            "External + None 应当合规（没有路径可校验、没有回滚可承诺）"
        );

        let bad = CompositeToolCatalog::new(
            vec![Arc::new(DescriptorOnly(external_descriptor(
                agent_base::domain::RollbackPolicy::PerTargetInBatch,
            ))) as Arc<dyn Tool>],
            None,
        );
        let violations = bad.validate(&Consumer::ALL);
        assert!(
            violations
                .iter()
                .any(|v| v.detail.contains("外部工具无法在本地回滚")),
            "External 声称可回滚必须被拒（这是「能撤销」的假承诺）：{violations:#?}"
        );
    }
}
