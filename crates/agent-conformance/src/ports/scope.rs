//! Scope 端口契约合规断言（INV-2, §8.1）
//!
//! 断言点：
//! 1. 作用域必须有稳定、非空的 id；
//! 2. 相对路径以作用域根为基准解析；
//! 3. 绝对路径与 `..` 照常解析——核心**不做**允许/拒绝裁决。

use agent_base::ports::Scope;

/// 验证 Scope 端口契约合规性
pub fn verify_scope_contract<S: Scope>(scope: &S) -> Result<(), String> {
    if scope.id().trim().is_empty() {
        return Err("scope id 不可为空".into());
    }

    if scope.resolve_path("file.txt").as_os_str().is_empty() {
        return Err("相对路径必须解析出非空结果".into());
    }

    // 跨目录路径也必须给出解析结果（是否允许由插件策略决定，不由 Scope 决定）
    if scope.resolve_path("../../../etc/passwd").as_os_str().is_empty() {
        return Err("跨目录路径也必须如实解析".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::MockScope;
    use std::path::PathBuf;

    #[test]
    fn test_mock_scope_conformance() {
        let scope = MockScope::new("workspace_root");
        verify_scope_contract(&scope).expect("Scope 契约必须通过");
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "a_da_conf_{tag}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn test_real_path_resolver_conformance() {
        use agent_adapter::scope::PathResolver;

        let ws = unique_dir("ws");
        let outside = unique_dir("outside");
        std::fs::create_dir_all(&ws).expect("建临时工作区失败");
        std::fs::create_dir_all(&outside).expect("建作用域外目录失败");

        let scope = PathResolver::new(&ws);
        verify_scope_contract(&scope).expect("真实 PathResolver 必须通过 Scope 契约");

        let inside = scope.resolve_path("src/new_file.rs");
        assert!(
            inside.starts_with(agent_toolkit::resolve_real_path(&ws)),
            "相对路径必须落在作用域根下：{}",
            inside.display()
        );

        let absolute = scope.resolve_path(&outside.to_string_lossy());
        assert_eq!(
            absolute,
            agent_toolkit::resolve_real_path(&outside),
            "绝对路径必须解析到真实落点，而不是被拒绝"
        );

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
