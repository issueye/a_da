//! PluginHost 端口契约合规断言（INV-2, INV-3, INV-4, §8.1）
//!
//! 断言点：
//! 1. 禁用状态插件（enabled = false）的工具调用必须被强行阻断
//! 2. 插件执行失败或超时时，按照 FailDirection::Closed 处理
//! 3. 插件自述工具必须符合注册表 ToolDescriptor 规范

use agent_base::domain::ToolDescriptor;
use agent_base::ports::{Consumer, ContractViolation};

/// 验证插件导出的描述符是否符合基座规范
pub fn verify_plugin_tool_descriptor(desc: &ToolDescriptor) -> Vec<ContractViolation> {
    let mut violations = Vec::new();

    if desc.name.contains(' ') {
        violations.push(ContractViolation {
            consumer: Consumer::PluginDeclaration,
            tool: desc.name.clone(),
            detail: "插件工具名称不可包含空格".into(),
        });
    }

    if !desc.schema.is_object() {
        violations.push(ContractViolation {
            consumer: Consumer::PluginDeclaration,
            tool: desc.name.clone(),
            detail: "插件工具 schema 必须是合法的 JSON Object".into(),
        });
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{Access, ApprovalPolicy, Execution, RollbackPolicy, Termination};

    #[test]
    fn test_plugin_descriptor_conformance() {
        let valid_desc = ToolDescriptor {
            name: "plugin_calc".into(),
            summary: "计算器".into(),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };
        assert!(verify_plugin_tool_descriptor(&valid_desc).is_empty());

        let invalid_desc = ToolDescriptor {
            name: "invalid calc with space".into(),
            summary: "不合格".into(),
            schema: serde_json::json!("not object"),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };
        let violations = verify_plugin_tool_descriptor(&invalid_desc);
        assert_eq!(violations.len(), 2);
    }
}
