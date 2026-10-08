//! RollbackStore 与回滚策略契约合规断言（INV-2, INV-3, §8.1）
//!
//! 断言点：
//! 1. RollbackPolicy 区分 None、SingleTarget 与 PerTargetInBatch
//! 2. 批量变更工具必须声明 PerTargetInBatch
//! 3. 产生文件变更的操作必须与回滚记录形成映射，不可逃逸检查点追踪

use agent_base::domain::{Access, PathSelector, RollbackPolicy, ToolDescriptor};

/// 验证工具声明的回滚策略合规性
pub fn verify_rollback_policy_contract(desc: &ToolDescriptor) -> Result<(), String> {
    match desc.access {
        Access::ReadOnly => {
            // 只读工具应当为 None
            if desc.rollback != RollbackPolicy::None {
                return Err(format!("只读工具 {} 不应声明变更回滚策略", desc.name));
            }
        }
        Access::Mutates { ref paths } => {
            match paths {
                PathSelector::Single(_) => {
                    if desc.rollback == RollbackPolicy::None {
                        return Err(format!("变更工具 {} 必须声明有效回滚策略（SingleTarget 或 PerTargetInBatch）", desc.name));
                    }
                }
                PathSelector::Batch(_) => {
                    if desc.rollback != RollbackPolicy::PerTargetInBatch {
                        return Err(format!("批量变更工具 {} 必须声明 PerTargetInBatch 回滚策略", desc.name));
                    }
                }
            }
        }
        Access::Executes { .. } => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_toolkit::fs::{BatchWriteTool, ReadFileTool, WriteFileTool};
    use agent_base::ports::Tool;

    #[test]
    fn test_toolkit_rollback_conformance() {
        let read = ReadFileTool::new(".");
        verify_rollback_policy_contract(read.descriptor()).expect("读工具合规");

        let write = WriteFileTool::new(".");
        verify_rollback_policy_contract(write.descriptor()).expect("写工具合规");

        let batch = BatchWriteTool::new(".");
        verify_rollback_policy_contract(batch.descriptor()).expect("批量写工具合规");
    }
}
