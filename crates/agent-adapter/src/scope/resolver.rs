//! `PathResolver`：只解析路径，不裁决访问边界。
//!
//! 契约（`agent_base::ports::Scope`）：
//! - `id()` 给一个**稳定标识**（规范化后的真实落点），用于落盘分组/日志/事件归属；
//! - `resolve_path()` 把相对路径拼到根上，再解析符号链接与不存在的尾段。
//!
//! 解析逻辑复用 `agent-toolkit::resolve_path`（一次只动一个真源）。它**不会**因为
//! "越出工作区"而失败——访问策略已交给插件事件；核心只负责给出真实路径。

use std::path::{Path, PathBuf};

use agent_base::ports::Scope;

pub struct PathResolver {
    /// 原始根路径（未规范化）：用于拼相对路径，保持与既有工具链一致的语义
    root: PathBuf,
    /// 规范化后的稳定标识（展开符号链接、去掉 Windows `\\?\` 前缀）
    id: String,
}

impl PathResolver {
    /// 以某个目录为解析根。不要求目录此刻存在。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let id = agent_toolkit::resolve_real_path(&root)
            .to_string_lossy()
            .to_string();
        Self { root, id }
    }

    /// 解析根（原始路径，未规范化）。
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Scope for PathResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn resolve_path(&self, raw: &str) -> PathBuf {
        agent_toolkit::resolve_path(&self.root, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "a_da_path_resolver_{tag}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn id_is_stable_and_non_empty() {
        let dir = unique_dir("id");
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let resolver = PathResolver::new(&dir);
        assert!(!resolver.id().trim().is_empty());
        let same = PathResolver::new(dir.join("."));
        assert_eq!(resolver.id(), same.id(), "同一目录的不同写法必须归一到同一 id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_relative_and_absolute_paths_without_a_boundary() {
        let dir = unique_dir("resolve");
        let outside = unique_dir("outside");
        std::fs::create_dir_all(dir.join("sub")).expect("建子目录失败");
        std::fs::create_dir_all(&outside).expect("建外部目录失败");

        let resolver = PathResolver::new(&dir);
        let inside = resolver.resolve_path("sub/new_file.rs");
        assert!(inside.starts_with(agent_toolkit::resolve_real_path(&dir)));

        let absolute = resolver.resolve_path(&outside.to_string_lossy());
        assert_eq!(absolute, agent_toolkit::resolve_real_path(&outside));

        let parent = resolver.resolve_path("../");
        assert_eq!(
            parent,
            agent_toolkit::resolve_real_path(dir.parent().expect("临时目录应有父目录")),
            "相对路径跨出根目录也必须照常解析"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
