use std::path::{Path, PathBuf};

/// 移除 Windows 平台下的 `\?\` UNC 前缀
pub fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

/// 解析路径的规范化真实落点（展开符号链接）
pub fn resolve_real_path(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => strip_unc_prefix(&canonical),
        Err(_) => path.to_path_buf(),
    }
}

/// 寻找最近一个真实存在的祖先并解析符号链接后拼回剩余路径
fn realpath_deepest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut missing_tail = Vec::new();

    for _ in 0..64 {
        if current.exists() {
            let real = resolve_real_path(&current);
            let mut result = real;
            for part in missing_tail.into_iter().rev() {
                result.push(part);
            }
            return result;
        }

        if let Some(name) = current.file_name() {
            missing_tail.push(name.to_os_string());
        }
        if let Some(parent) = current.parent() {
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    path.to_path_buf()
}

/// 把插件给出的路径解析成真实落点：相对路径拼到 `workspace`，绝对路径原样解析。
///
/// **不做边界判定**：核心不再负责允许/拒绝；需要策略时由插件事件决定。
pub fn resolve_workspace_path(workspace: &Path, target_path: &str) -> PathBuf {
    let raw_target = Path::new(target_path);
    let full = if raw_target.is_absolute() {
        raw_target.to_path_buf()
    } else {
        workspace.join(raw_target)
    };
    realpath_deepest_existing(&full)
}
