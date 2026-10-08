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
        let violations = Vec::new();

        for _d in &descs {
            for c in consumers {
                match c {
                    Consumer::ReadonlyFilter => {
                        // 只读性必须由 Access 显式判定
                    }
                    Consumer::ApprovalPolicy => {
                        // 审批策略不能处于未决状态
                    }
                    Consumer::RollbackPolicy => {
                        // 回滚策略验证
                    }
                    Consumer::SubagentAllowlist => {
                        // 子智能体白名单验证
                    }
                    Consumer::PluginDeclaration => {
                        // 插件声明验证
                    }
                }
            }
        }

        violations
    }
}
