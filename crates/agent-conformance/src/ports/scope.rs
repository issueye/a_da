//! ScopePolicy 端口契约合规断言（INV-2, §8.1）
//!
//! 断言点：
//! 1. 作用域必须严格限制在指定 scope 之下
//! 2. 任何跨目录逃逸（如 ../../windows）一律被拒绝

use agent_base::ports::Scope;

/// 验证 Scope 端口契约合规性
pub fn verify_scope_contract<S: Scope>(scope: &S) -> Result<(), String> {
    // 1. scope id 非空
    if scope.id().trim().is_empty() {
        return Err("scope id 不可为空".into());
    }

    // 2. 正常路径包含在内
    if scope.resolve_path("file.txt").is_err() {
        return Err("相对文件应该被允许".into());
    }

    // 3. 逃逸路径被阻止
    if scope.resolve_path("../../../etc/passwd").is_ok() {
        return Err("逃逸路径必须被阻断".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::MockScope;

    #[test]
    fn test_mock_scope_conformance() {
        let scope = MockScope::new("workspace_root");
        verify_scope_contract(&scope).expect("Scope 契约必须通过");
    }
}
