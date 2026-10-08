//! Tool 与 ToolCatalog 端口契约合规断言（INV-2, INV-3, INV-5, §8.1）
//!
//! 断言点：
//! 1. 未知工具查找返回 Err，杜绝静默兜底
//! 2. 工具 schema 必须为合法的 JSON Object，工具描述符字段非空
//! 3. CompositeToolCatalog 对五类消费者的 validate 校验无 ContractViolation
//! 4. 工具执行返回的 ToolReceipt 结构化字段齐全，duration_ms 计算合法

use agent_base::domain::ToolReceipt;
use agent_base::ports::{Consumer, ContractViolation, Tool, ToolCatalog};

/// 验证单个 Tool 契约合规性
pub fn verify_tool_contract(tool: &dyn Tool) -> Vec<ContractViolation> {
    let desc = tool.descriptor();
    let mut violations = Vec::new();

    if desc.name.trim().is_empty() {
        violations.push(ContractViolation {
            consumer: Consumer::PluginDeclaration,
            tool: desc.name.clone(),
            detail: "工具名称不能为空".into(),
        });
    }

    if !desc.schema.is_object() {
        violations.push(ContractViolation {
            consumer: Consumer::PluginDeclaration,
            tool: desc.name.clone(),
            detail: "工具 schema 必须是合法 JSON Object".into(),
        });
    }

    violations
}

/// 验证 ToolCatalog 契约合规性
pub fn verify_tool_catalog_contract<C: ToolCatalog>(catalog: &C) -> Result<(), Vec<ContractViolation>> {
    // 1. 未知名查找必须为 Err
    if catalog.resolve("__non_existent_tool_xyz__").is_ok() {
        return Err(vec![ContractViolation {
            consumer: Consumer::ReadonlyFilter,
            tool: "__non_existent_tool_xyz__".into(),
            detail: "查询未注册工具必须返回 Err，不可静默成功".into(),
        }]);
    }

    // 2. 对所有五类消费者的全面校验
    let violations = catalog.validate(&Consumer::ALL);
    if !violations.is_empty() {
        return Err(violations);
    }

    Ok(())
}

/// 验证 ToolReceipt 结构体回执字段非空与耗时正确（INV-5）
pub fn verify_tool_receipt_contract(receipt: &ToolReceipt) -> Result<(), String> {
    if receipt.finished_at < receipt.started_at {
        return Err("finished_at 不能早于 started_at".into());
    }
    let dur = receipt.duration_ms();
    let expected_ms = (receipt.finished_at - receipt.started_at).max(0) as u64;
    if dur != expected_ms {
        return Err(format!("回执 duration_ms 派生计算错误: expected {}ms, got {}ms", expected_ms, dur));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use agent_base::domain::ToolStatus;
    use agent_runtime::CompositeToolCatalog;
    use agent_toolkit::core::FinishTool;

    #[test]
    fn test_finish_tool_conformance() {
        let tool = FinishTool::default();
        let violations = verify_tool_contract(&tool);
        assert!(violations.is_empty(), "FinishTool 必须符合规范");

        let catalog = CompositeToolCatalog::new(vec![Arc::new(tool)], None);
        assert!(verify_tool_catalog_contract(&catalog).is_ok());
    }

    #[test]
    fn test_receipt_structure_conformance() {
        let receipt = ToolReceipt {
            status: ToolStatus::Success,
            output: "完成".into(),
            data: None,
            details: None,
            started_at: 1000,
            finished_at: 1500,
        };
        assert!(verify_tool_receipt_contract(&receipt).is_ok());
        assert_eq!(receipt.duration_ms(), 500);
    }
}
