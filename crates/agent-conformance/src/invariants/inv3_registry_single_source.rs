//! INV-3: 注册表即真源不变量
//! 工具的只读性/审批/回滚/执行模式/终止语义全部来自 ToolDescriptor；
//! 任何按名字的旁路名单都是缺陷。

use agent_base::ports::{Consumer, ContractViolation, ToolCatalog};

/// 断言工具目录对所有消费者类别完全合规
pub fn assert_catalog_single_source_integrity<C: ToolCatalog>(catalog: &C) -> Result<(), Vec<ContractViolation>> {
    let violations = catalog.validate(&Consumer::ALL);
    if !violations.is_empty() {
        return Err(violations);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use agent_runtime::CompositeToolCatalog;
    use agent_toolkit::core::FinishTool;

    #[test]
    fn test_inv3_registry_single_source() {
        let catalog = CompositeToolCatalog::new(vec![Arc::new(FinishTool::default())], None);
        assert_catalog_single_source_integrity(&catalog).expect("内置工具目录必须单一真源合规");
    }
}
