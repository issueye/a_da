use sha2::{Digest, Sha256};

/// 计算工作区的短散列目录名（与 TypeScript 的 workspaceSlug 完全一致）
///
/// 算法：
/// 1. 将一个或多个连续的 `/` 或 `\` 统一替换为单个 `\`
/// 2. 转为小写
/// 3. 进行 SHA-256 计算并取 hex 前 20 个字符
pub fn workspace_slug(workspace: &str) -> String {
    let mut canonical = String::with_capacity(workspace.len());
    let mut in_slash_run = false;

    for ch in workspace.chars() {
        if ch == '/' || ch == '\\' {
            if !in_slash_run {
                canonical.push('\\');
                in_slash_run = true;
            }
        } else {
            in_slash_run = false;
            for lower_ch in ch.to_lowercase() {
                canonical.push(lower_ch);
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let result = hasher.finalize();
    let hex_str = format!("{:x}", result);
    hex_str[..20].to_string()
}

/// 将会话 ID 中的非安全字符（除了字母、数字、下划线、减号）转换为 `_`
pub fn safe_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_slug_matches_typescript() {
        assert_eq!(
            workspace_slug("E:\\codes\\rust_projects\\a_da"),
            "f6ab61e19e121c35d37e"
        );
        assert_eq!(
            workspace_slug("E:/codes/rust_projects/a_da"),
            "f6ab61e19e121c35d37e"
        );
        assert_eq!(
            workspace_slug("E://codes///rust_projects\\\\a_da"),
            "f6ab61e19e121c35d37e"
        );
        assert_eq!(
            workspace_slug("/Users/test/project"),
            "ea89828cdc4d0a994d26"
        );
    }


    #[test]
    fn test_safe_id() {
        assert_eq!(safe_id("thread_123-abc"), "thread_123-abc");
        assert_eq!(safe_id("thread:foo/bar?baz"), "thread_foo_bar_baz");
    }
}
