//! **网关文件系统服务**：提供磁盘驱动器根节点发现、目录树遍历与新建目录能力。
//!
//! 供客户端通过 `gateway.fs.*` RPC 方法调用，完全不依赖特定的 Agent 实例。
//! 遵循最小权限与路径防穿越原则。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use agent_proto::{FsEntry, FsListing, FsRoot};

const MAX_ENTRIES_DEFAULT: usize = 1000;
const MAX_ENTRIES_HARD_LIMIT: usize = 5000;

/// 规范化路径字符串（统一斜杠为 '/'，并处理 Windows 盘符）
pub fn normalize_path_str(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        // 去除 Windows 扩展长度前缀 "\\?\"
        if let Some(stripped) = s.strip_prefix("//?/") {
            return stripped.to_string();
        }
        if let Some(stripped) = s.strip_prefix("\\\\?\\") {
            return stripped.to_string();
        }
    }
    s
}

/// 规范化输入原始路径为平台特定的绝对 PathBuf
pub fn resolve_path(raw_path: &str) -> PathBuf {
    let trimmed = raw_path.trim();
    #[cfg(windows)]
    {
        let replaced = trimmed.replace('/', "\\");
        if replaced.len() == 2 && replaced.ends_with(':') {
            PathBuf::from(format!("{}\\", replaced))
        } else if replaced.len() > 3 && replaced.ends_with('\\') {
            PathBuf::from(replaced.trim_end_matches('\\'))
        } else {
            PathBuf::from(replaced)
        }
    }
    #[cfg(not(windows))]
    {
        if trimmed.len() > 1 && trimmed.ends_with('/') {
            PathBuf::from(trimmed.trim_end_matches('/'))
        } else {
            PathBuf::from(trimmed)
        }
    }
}

/// 检查目标路径是否属于允许的根目录范围（若未指定白名单则放行）
pub fn check_allowed_root(target: &Path, allowed_roots: Option<&[String]>) -> Result<(), String> {
    let Some(roots) = allowed_roots else {
        return Ok(());
    };
    if roots.is_empty() {
        return Ok(());
    }

    let target_canon = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let target_str = target_canon.to_string_lossy().to_lowercase();

    for r in roots {
        let r_path = PathBuf::from(r);
        let r_canon = fs::canonicalize(&r_path).unwrap_or(r_path);
        let r_str = r_canon.to_string_lossy().to_lowercase();
        if target_str.starts_with(&r_str) {
            return Ok(());
        }
    }

    Err(format!(
        "访问被拒绝：路径 `{}` 超出允许的根目录白名单",
        target.display()
    ))
}

/// 列出所有可用的系统根目录、主目录与驱动器
pub fn list_roots(
    extra_workspaces: &[String],
    allowed_roots: Option<&[String]>,
) -> Vec<FsRoot> {
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut push_root = |path_buf: PathBuf, label: String, kind: &str| {
        let norm = normalize_path_str(&path_buf);
        let key = norm.to_lowercase();
        if seen.insert(key) {
            // 如果存在 allowed_roots 限制，过滤掉不在白名单内的根
            if check_allowed_root(&path_buf, allowed_roots).is_ok() {
                roots.push(FsRoot {
                    label,
                    path: norm,
                    kind: kind.to_string(),
                });
            }
        }
    };

    // 1. 用户主目录 (Home)
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home_path = PathBuf::from(home);
        push_root(home_path.clone(), "用户主目录".to_string(), "home");

        // 桌面快捷入口
        let desktop = home_path.join("Desktop");
        if desktop.is_dir() {
            push_root(desktop, "桌面".to_string(), "folder");
        }
    }

    // 2. 物理磁盘驱动器
    #[cfg(windows)]
    {
        for byte in b'C'..=b'Z' {
            let drive_str = format!("{}:\\", byte as char);
            let p = PathBuf::from(&drive_str);
            if p.exists() {
                push_root(p, format!("本地磁盘 ({}:)", byte as char), "drive");
            }
        }
    }

    #[cfg(not(windows))]
    {
        push_root(PathBuf::from("/"), "根目录 (/)".to_string(), "drive");
    }

    // 3. 额外追加的历史/配置工作区
    for ws in extra_workspaces {
        if !ws.trim().is_empty() {
            let p = PathBuf::from(ws);
            if p.is_dir() {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| ws.clone());
                push_root(p, name, "workspace");
            }
        }
    }

    roots
}

/// 浏览指定目录下的子项（支持仅目录、支持隐藏文件过滤、支持条数截断）
pub fn list_directory(
    raw_path: &str,
    directories_only: bool,
    show_hidden: bool,
    limit: Option<usize>,
    allowed_roots: Option<&[String]>,
) -> Result<FsListing, String> {
    if raw_path.trim().is_empty() {
        return Err("路径不能为空".to_string());
    }

    let target = resolve_path(raw_path);
    if !target.exists() {
        return Err(format!("找不到路径：{}", raw_path));
    }

    // 安全检查：防止相对路径越界
    check_allowed_root(&target, allowed_roots)?;

    let meta = fs::metadata(&target).map_err(|e| format!("无法读取路径元数据: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("目标不是一个目录：{}", raw_path));
    }

    let read_dir = fs::read_dir(&target).map_err(|e| format!("读取目录内容失败: {e}"))?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry_res in read_dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_name = entry.file_name().to_string_lossy().to_string();

        // 隐藏文件过滤（以 '.' 开头）
        if !show_hidden && file_name.starts_with('.') {
            continue;
        }

        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

        // 如果要求仅目录，则忽略非目录文件
        if directories_only && !is_dir {
            continue;
        }

        let entry_meta = entry.metadata().ok();
        let size = entry_meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = entry_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let fs_entry = FsEntry {
            name: file_name,
            path: normalize_path_str(&entry.path()),
            is_dir,
            size,
            mtime,
        };

        if is_dir {
            dirs.push(fs_entry);
        } else {
            files.push(fs_entry);
        }
    }

    // 排序：不区分大小写
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let dirs_clone = dirs.clone();
    let files_clone = files.clone();

    let max_limit = limit
        .unwrap_or(MAX_ENTRIES_DEFAULT)
        .min(MAX_ENTRIES_HARD_LIMIT);

    let mut entries = Vec::with_capacity(max_limit);
    entries.extend(dirs);
    entries.extend(files);

    let truncated = entries.len() > max_limit;
    let omitted = if truncated {
        entries.len() - max_limit
    } else {
        0
    };
    if truncated {
        entries.truncate(max_limit);
    }

    let parent = target.parent().map(|p| normalize_path_str(p));

    Ok(FsListing {
        path: normalize_path_str(&target),
        parent,
        entries,
        dirs: dirs_clone,
        files: files_clone,
        truncated,
        omitted,
    })
}

/// 在指定父级目录下新建子文件夹
pub fn make_directory(
    parent_path: &str,
    folder_name: &str,
    allowed_roots: Option<&[String]>,
) -> Result<String, String> {
    let name = folder_name.trim();
    if name.is_empty() {
        return Err("文件夹名称不能为空".to_string());
    }

    // 非法文件名字符校验
    if name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
        || name.contains('*')
        || name.contains('?')
        || name.contains('"')
        || name.contains('<')
        || name.contains('>')
        || name.contains('|')
    {
        return Err(format!("文件夹名称包含非法字符：`{}`", name));
    }

    let parent = resolve_path(parent_path);
    if !parent.exists() {
        return Err(format!("上一级目录不存在：{}", parent_path));
    }
    if !parent.is_dir() {
        return Err(format!("上一级路径不是目录：{}", parent_path));
    }

    check_allowed_root(&parent, allowed_roots)?;

    let target = parent.join(name);
    if target.exists() {
        return Err(format!("该目录已存在：{}", normalize_path_str(&target)));
    }

    fs::create_dir(&target).map_err(|e| format!("创建文件夹失败: {e}"))?;

    Ok(normalize_path_str(&target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_roots_contains_home() {
        let roots = list_roots(&[], None);
        assert!(!roots.is_empty());
        let has_home = roots.iter().any(|r| r.kind == "home");
        assert!(has_home, "应能探测到用户主目录");
    }

    #[test]
    fn test_list_directory_temp() {
        let temp = std::env::temp_dir();
        let listing = list_directory(&temp.to_string_lossy(), false, false, Some(100), None)
            .expect("读取临时目录应成功");
        assert!(!listing.path.is_empty());
    }

    #[test]
    fn test_make_directory_and_illegal_chars() {
        let test_base = std::env::current_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("temp")
            .join(format!("ada_test_gw_mkdir_{}", uuid::Uuid::new_v4().simple()));
        let _ = fs::create_dir_all(&test_base);
        let illegal_res = make_directory(&test_base.to_string_lossy(), "test/illegal", None);
        assert!(illegal_res.is_err(), "非法字符应被拒绝");

        let unique_name = format!("sub_{}", uuid::Uuid::new_v4().simple());
        let ok_res = make_directory(&test_base.to_string_lossy(), &unique_name, None);
        assert!(ok_res.is_ok(), "合法目录创建应成功: {:?}", ok_res.err());

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_allowed_roots_security() {
        let temp = std::env::temp_dir();
        let allowed = vec![normalize_path_str(&temp)];

        // 临时目录内应允许
        assert!(check_allowed_root(&temp, Some(&allowed)).is_ok());

        // 伪造系统根目录应被拒绝
        #[cfg(windows)]
        let outside = PathBuf::from("C:\\Windows");
        #[cfg(not(windows))]
        let outside = PathBuf::from("/etc");

        assert!(check_allowed_root(&outside, Some(&allowed)).is_err());
    }
}
