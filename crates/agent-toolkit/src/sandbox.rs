use std::path::{Component, Path, PathBuf};
use anyhow::{bail, Result};


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

/// 判断一个目标路径是否安全落在基准工作区内（不区分大小写）
fn is_inside(base: &Path, target: &Path) -> bool {
    let base_comps: Vec<_> = base.components().collect();
    let target_comps: Vec<_> = target.components().collect();

    if target_comps.len() < base_comps.len() {
        return false;
    }

    for (b, t) in base_comps.iter().zip(target_comps.iter()) {
        match (b, t) {
            (Component::Prefix(p1), Component::Prefix(p2)) => {
                if p1.as_os_str().to_string_lossy().to_lowercase()
                    != p2.as_os_str().to_string_lossy().to_lowercase()
                {
                    return false;
                }
            }
            (Component::RootDir, Component::RootDir) => {}
            (Component::Normal(n1), Component::Normal(n2)) => {
                if n1.to_string_lossy().to_lowercase() != n2.to_string_lossy().to_lowercase() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// 校验模型请求的路径是否在工作区沙箱内，越界即拒绝
pub fn check_workspace_sandbox(workspace: &Path, target_path: &str) -> Result<PathBuf> {
    let norm_ws = resolve_real_path(workspace);

    let raw_target = Path::new(target_path);
    let full = if raw_target.is_absolute() {
        raw_target.to_path_buf()
    } else {
        workspace.join(raw_target)
    };

    // 1. 语法级与祖先级检查
    let real_full = realpath_deepest_existing(&full);

    if !is_inside(&norm_ws, &real_full) {
        bail!("拒绝访问工作区外的路径: {}", target_path);
    }

    Ok(real_full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_safety() {
        let ws = std::env::current_dir().unwrap();
        let safe_file = check_workspace_sandbox(&ws, "Cargo.toml");
        assert!(safe_file.is_ok());

        let escape_file = check_workspace_sandbox(&ws, "../../../../../Windows/System32/calc.exe");
        assert!(escape_file.is_err());
    }

    #[test]
    fn test_skip_dirs() {
        assert!(should_skip_dir("node_modules"));
        assert!(should_skip_dir("target"));
        assert!(should_skip_dir(".git"));
        assert!(!should_skip_dir("src"));
    }
}
