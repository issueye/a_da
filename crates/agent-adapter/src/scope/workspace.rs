//! `WorkspaceScope`：编码助手形态的作用域（工作区目录）。
//!
//! 契约（`agent_base::ports::Scope`）：
//! - `id()` 给一个**稳定标识**，用于落盘分组/日志/事件归属 → 这里取规范化后的真实落点，
//!   这样同一个工作区经不同写法（`./ws`、`ws\`、大小写差异）进来时 id 一致。
//! - `resolve_path()` 做**一次路径判定**，越界一律 [`DenialKind::Sandbox`]（失败方向固定为拒绝）。
//!
//! **判定逻辑的来源**：`agent_toolkit::check_workspace_sandbox`。适配器只做
//! "错误类型翻译"（`anyhow::Error` → `DenialKind`），不复制一份路径把关代码——
//! 一旦复制，`..`/symlink/junction 的修法就会在两处分叉（这正是本计划要消灭的漂移）。

use std::path::{Path, PathBuf};

use agent_base::domain::DenialKind;
use agent_base::ports::Scope;

pub struct WorkspaceScope {
    /// 原始工作区路径（未规范化）：用于拼相对路径，保持与既有工具链一致的语义
    root: PathBuf,
    /// 规范化后的稳定标识（展开符号链接、去掉 Windows `\\?\` 前缀）
    id: String,
}

impl WorkspaceScope {
    /// 以某个目录为作用域根。
    ///
    /// 不要求目录此刻存在——`resolve_path` 对"尚不存在的目标"会走
    /// `realpath_deepest_existing` 的祖先回退逻辑（新建文件是常见场景）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let id = agent_toolkit::resolve_real_path(&root)
            .to_string_lossy()
            .to_string();
        Self { root, id }
    }

    /// 作用域根（原始路径，未规范化）。
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Scope for WorkspaceScope {
    fn id(&self) -> &str {
        &self.id
    }

    fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
        agent_toolkit::check_workspace_sandbox(&self.root, raw)
            .map_err(|_| DenialKind::Sandbox { path: raw.to_string() })
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
        std::env::temp_dir().join(format!("a_da_ws_scope_{tag}_{}_{}", std::process::id(), nanos))
    }

    #[test]
    fn test_id_is_stable_and_non_empty() {
        let dir = unique_dir("id");
        std::fs::create_dir_all(&dir).expect("建临时工作区失败");
        let scope = WorkspaceScope::new(&dir);
        assert!(!scope.id().trim().is_empty(), "作用域 id 不可为空");
        // 同一目录的两种写法（带尾分隔符）必须得到同一个 id
        let scope2 = WorkspaceScope::new(dir.join("."));
        assert_eq!(scope.id(), scope2.id(), "同一工作区的不同写法必须归一到同一 id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_inside_path_resolves_and_escape_is_denied() {
        let dir = unique_dir("inside");
        std::fs::create_dir_all(dir.join("sub")).expect("建临时工作区失败");
        let scope = WorkspaceScope::new(&dir);

        // 作用域内（含尚不存在的目标）→ 放行
        let ok = scope.resolve_path("sub/new_file.rs").expect("作用域内路径必须放行");
        assert!(ok.starts_with(agent_toolkit::resolve_real_path(&dir)));

        // `..` 穿越 → 拒绝，且拒绝原因是 Sandbox（不是别的）
        let denied = scope.resolve_path("../../../../etc/passwd").expect_err("越界必须被拒");
        assert!(
            matches!(denied, DenialKind::Sandbox { .. }),
            "越界必须报 DenialKind::Sandbox，实际 {denied:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
