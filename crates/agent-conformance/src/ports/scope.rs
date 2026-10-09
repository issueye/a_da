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
        std::env::temp_dir().join(format!("a_da_conf_{tag}_{}_{}", std::process::id(), nanos))
    }

    #[cfg(windows)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
    #[cfg(not(windows))]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// W1-T1 守门：**真实实现**（`WorkspaceScope`，不是 testing double）也要过同一份契约。
    ///
    /// 契约本身只覆盖 `id()` 非空与 `..` 逃逸；这里额外补两条真实实现才有的语义：
    /// 拒绝原因必须是 `Sandbox`（不是"随便什么错"），以及工作区内指向外部的
    /// 符号链接按**真实落点**拒绝。
    #[test]
    fn test_real_workspace_scope_conformance() {
        use agent_adapter::scope::WorkspaceScope;
        use agent_base::domain::DenialKind;

        let ws = unique_dir("ws");
        let outside = unique_dir("outside");
        std::fs::create_dir_all(&ws).expect("建临时工作区失败");
        std::fs::create_dir_all(&outside).expect("建作用域外目录失败");

        let scope = WorkspaceScope::new(&ws);
        verify_scope_contract(&scope).expect("真实 WorkspaceScope 必须通过 Scope 契约");

        // 拒绝原因必须是 Sandbox：界面要靠它区分"越界"与"审批被拒"
        let err = scope
            .resolve_path("../../../../etc/passwd")
            .expect_err("越界必须被拒");
        assert!(
            matches!(err, DenialKind::Sandbox { .. }),
            "越界拒绝原因必须是 DenialKind::Sandbox，实际 {err:?}"
        );

        // 作用域内（含尚不存在的目标）必须放行——新建文件是常见场景
        assert!(
            scope.resolve_path("src/new_file.rs").is_ok(),
            "作用域内路径必须放行"
        );

        // 工作区内指向外部的符号链接/junction：按真实落点拒绝
        let link = ws.join("escape_link");
        if create_dir_link(&outside, &link) {
            let err = scope
                .resolve_path("escape_link")
                .expect_err("指向作用域外的链接必须被拒");
            assert!(
                matches!(err, DenialKind::Sandbox { .. }),
                "链接越界也必须报 Sandbox，实际 {err:?}"
            );
        } else {
            eprintln!(
                "[skip] 当前环境无创建符号链接权限（Windows 需管理员或开发者模式），\
                 跳过 symlink 越界断言；realpath 把关本身在 agent-toolkit 的沙箱实现里"
            );
        }

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
