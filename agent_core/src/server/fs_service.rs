use crate::protocol::{AppErrorCode, FsEntry, FsListing, FsRoot, ProtocolError, RpcErrorCode};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAX_ENTRIES: usize = 2000;

pub fn list_roots(extra_roots: &[String]) -> Vec<FsRoot> {
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut push_root = |path_buf: PathBuf, label: String, kind: &str| {
        let key = path_buf.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            roots.push(FsRoot {
                path: path_buf.to_string_lossy().to_string(),
                label,
                kind: kind.to_string(),
            });
        }
    };

    for ws in extra_roots {
        if !ws.trim().is_empty() {
            let p = PathBuf::from(ws);
            let name = p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| ws.clone());
            push_root(p, name, "workspace");
        }
    }

    // 主目录 (Home)
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        push_root(PathBuf::from(home), "主目录".to_string(), "home");
    }

    #[cfg(windows)]
    {
        for byte in b'C'..=b'Z' {
            let drive = format!("{}:\\", byte as char);
            let p = PathBuf::from(&drive);
            if p.exists() {
                push_root(p, drive, "drive");
            }
        }
    }

    #[cfg(not(windows))]
    {
        push_root(PathBuf::from("/"), "/".to_string(), "drive");
    }

    roots
}

pub fn list_directory(raw_path: &str, show_hidden: bool, limit: Option<usize>) -> Result<FsListing, ProtocolError> {
    #[cfg(windows)]
    let normalized = {
        let trimmed = raw_path.trim().replace('/', "\\");
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            format!("{}\\", trimmed)
        } else if trimmed.len() == 3 && trimmed.ends_with('\\') && trimmed.chars().nth(1) == Some(':') {
            trimmed
        } else {
            trimmed
        }
    };
    #[cfg(not(windows))]
    let normalized = raw_path.trim().to_string();

    let target = PathBuf::from(&normalized);
    if !target.exists() {
        return Err(ProtocolError::new(
            AppErrorCode::NotFound.code(),
            format!("找不到路径：{}", raw_path),
            Some(serde_json::json!({ "path": raw_path })),
        ));
    }
    let metadata = fs::metadata(&target).map_err(|e| {
        ProtocolError::new(
            AppErrorCode::Denied.code(),
            format!("无法读取路径信息: {}", e),
            None,
        )
    })?;

    if !metadata.is_dir() {
        return Err(ProtocolError::new(
            RpcErrorCode::InvalidParams.code(),
            format!("不是目录：{}", raw_path),
            Some(serde_json::json!({ "path": raw_path })),
        ));
    }

    let read_dir = fs::read_dir(&target).map_err(|e| {
        ProtocolError::new(
            AppErrorCode::Denied.code(),
            format!("读不了这个路径: {}", e),
            None,
        )
    })?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry_res in read_dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && file_name.starts_with('.') {
            continue;
        }

        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let fs_entry = FsEntry {
            name: file_name,
            path: entry.path().to_string_lossy().replace('\\', "/"),
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

    // 目录在前，文件在后，各自不区分大小写排序
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let dirs_clone = dirs.clone();
    let files_clone = files.clone();

    let max_limit = limit.unwrap_or(MAX_ENTRIES).min(MAX_ENTRIES);

    let mut entries = Vec::with_capacity(max_limit);
    entries.extend(dirs);
    entries.extend(files);

    let truncated = entries.len() > max_limit;
    let omitted = if truncated { entries.len() - max_limit } else { 0 };
    if truncated {
        entries.truncate(max_limit);
    }

    let parent = target
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"));

    Ok(FsListing {
        path: target.to_string_lossy().replace('\\', "/"),
        parent,
        entries,
        dirs: dirs_clone,
        files: files_clone,
        truncated,
        omitted,
    })
}

pub fn make_directory(raw_path: &str) -> Result<String, ProtocolError> {
    let target = PathBuf::from(raw_path);
    if target.exists() {
        return Err(ProtocolError::new(
            RpcErrorCode::InvalidParams.code(),
            format!("目录已存在：{}", raw_path),
            None,
        ));
    }
    let parent = target.parent().unwrap_or(Path::new(""));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        return Err(ProtocolError::new(
            AppErrorCode::NotFound.code(),
            format!("上一级目录不存在：{}", parent.to_string_lossy()),
            None,
        ));
    }

    fs::create_dir(&target).map_err(|e| {
        ProtocolError::new(
            AppErrorCode::Denied.code(),
            format!("创建目录失败: {}", e),
            None,
        )
    })?;

    Ok(target.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_directory_returns_dirs_and_files() {
        let current_dir = std::env::current_dir().unwrap();
        let listing = list_directory(&current_dir.to_string_lossy(), false, None).unwrap();
        assert!(!listing.entries.is_empty(), "entries 不应为空");
        assert_eq!(listing.entries.len(), listing.dirs.len() + listing.files.len());
    }

    #[test]
    #[cfg(windows)]
    fn test_list_directory_windows_root() {
        if Path::new("E:\\").exists() {
            let listing = list_directory("E:/", false, None).unwrap();
            assert!(!listing.dirs.is_empty() || !listing.files.is_empty(), "E:/ 应当能列出内容");
        }
    }
}
