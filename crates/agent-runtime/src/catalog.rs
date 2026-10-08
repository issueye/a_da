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
