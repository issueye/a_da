use std::path::{Path, PathBuf};

pub static SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    "target",
    ".next",
    ".cache",
    ".venv",
    "__pycache__",
    ".turbo",
    "coverage",
];

pub fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|&d| d.eq_ignore_ascii_case(name))
}

/// 移除 Windows 平台下的 `\\?\` UNC 前缀
pub fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

/// 解析路径的规范化真实落点（展开符号链接）
pub fn resolve_real_path(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => strip_unc_prefix(&canonical),
        Err(_) => {
            let mut current = path.to_path_buf();
            for _ in 0..16 {
                if let Ok(target) = std::fs::read_link(&current) {
                    let resolved_target = if target.is_absolute() {
                        target
                    } else if let Some(parent) = current.parent() {
                        parent.join(target)
                    } else {
                        target
                    };
                    if let Ok(can) = std::fs::canonicalize(&resolved_target) {
                        return strip_unc_prefix(&can);
                    }
                    current = resolved_target;
                } else {
                    break;
                }
            }
            strip_unc_prefix(&current)
        }
    }
}

/// 寻找最近一个真实存在的祖先并解析符号链接后拼回剩余路径
fn realpath_deepest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut missing_tail = Vec::new();

    for _ in 0..64 {
        if current.symlink_metadata().is_ok() {
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

/// 把模型给出的路径解析成真实落点：相对路径拼到 `root`，绝对路径原样解析。
///
/// **不做边界判定**：核心不再负责允许/拒绝；需要策略时由插件事件决定。
pub fn resolve_path(root: &Path, target: &str) -> PathBuf {
    let p = Path::new(target);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    realpath_deepest_existing(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_resolved_under_the_root() {
        let root = std::env::current_dir().unwrap();
        let resolved = resolve_path(&root, "Cargo.toml");
        assert_eq!(resolved, resolve_real_path(&root).join("Cargo.toml"));
    }

    #[test]
    fn absolute_paths_and_parent_segments_are_resolved_not_rejected() {
        let root = std::env::current_dir().unwrap();
        let outside = resolve_path(&root, "../../../../../Windows/System32/calc.exe");
        assert!(outside.is_absolute());
        assert!(
            !outside.starts_with(resolve_real_path(&root)),
            "解析结果如实反映目标位置，不再因为越界而拒绝：{}",
            outside.display()
        );
    }

    #[test]
    fn test_skip_dirs() {
        assert!(should_skip_dir("node_modules"));
        assert!(should_skip_dir("target"));
        assert!(should_skip_dir(".git"));
        assert!(!should_skip_dir("src"));
    }
}
