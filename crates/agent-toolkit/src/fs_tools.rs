use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::diff::unified_diff;
use super::sandbox::{check_workspace_sandbox, resolve_real_path, should_skip_dir};


const MAX_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_LIMIT: usize = 400;
const MAX_LIST_ENTRIES: usize = 200;
const MAX_SEARCH_MATCHES: usize = 200;

/// 工具失败的**结构化**原因（W4-T2）。
///
/// 为什么需要它：`run_command` 原先让上层靠 `output.contains("取消")` / `contains("超时")`
/// 猜状态。输出是**给人看的**（措辞随时会改、还要本地化），拿它当机器判据必然漂移——
/// 换个措辞，`Aborted` 就静默变成 `Error`，界面徽章与调用方判断一起错。
/// 失败原因必须由**产生它的那层**明确给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolFailure {
    /// 用户/轮次取消
    Aborted,
    /// 超时
    Timeout,
    /// 进程非零退出
    NonZeroExit,
    /// 其它失败（参数错误、IO 失败等）
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
    /// 结构化失败原因（成功时为 `None`）。见 [`ToolFailure`]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ToolFailure>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            ok: true,
            output: output.into(),
            patch: None,
            details: None,
            terminate: None,
            failure: None,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
            patch: None,
            details: None,
            terminate: None,
            failure: Some(ToolFailure::Other),
        }
    }

    /// 带**结构化原因**的失败。
    pub fn failed(kind: ToolFailure, output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
            patch: None,
            details: None,
            terminate: None,
            failure: Some(kind),
        }
    }

    /// 失败原因（成功则 `None`）。上层据此判定状态，**不要解析 `output`**。
    pub fn failure(&self) -> Option<ToolFailure> {
        self.failure
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPair {
    pub old_string: String,
    pub new_string: String,
}

/// 检查并读取文本文件（包含目录、体积、二进制过滤）
fn read_text_file(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        anyhow::bail!("这是目录，不是文件: {}", path.display());
    }
    if metadata.len() > MAX_FILE_BYTES {
        anyhow::bail!(
            "文件过大（{} 字节，上限 {} 字节），请用 offset/limit 分段读取",
            metadata.len(),
            MAX_FILE_BYTES
        );
    }

    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    // 检查前 4096 字节是否包含 0 字节（二进制判定）
    let check_len = buffer.len().min(4096);
    if buffer[..check_len].contains(&0) {
        anyhow::bail!("这是二进制文件: {}", path.display());
    }

    let content = String::from_utf8(buffer)
        .map_err(|_| anyhow::anyhow!("文件不是合法的 UTF-8 编码文本"))?;
    Ok(content)
}

/// 1. read_file
pub fn read_file(
    workspace: &Path,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> ToolResult {
    let full = match check_workspace_sandbox(workspace, path) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e.to_string()),
    };

    let content = match read_text_file(&full) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(e.to_string()),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let off = offset.unwrap_or(1).max(1);
    let lim = limit.unwrap_or(DEFAULT_LIMIT).max(1);

    let start_idx = off - 1;
    if start_idx >= total_lines {
        return ToolResult::success(format!("(指定行号超出文件末尾，总共 {} 行)", total_lines));
    }

    let end_idx = (start_idx + lim).min(total_lines);
    let slice = &lines[start_idx..end_idx];

    let mut output = String::new();
    for (i, line) in slice.iter().enumerate() {
        let line_num = off + i;
        output.push_str(&format!("{} | {}\n", line_num, line));
    }

    ToolResult {
        ok: true,
        output,
        patch: None,
        details: Some(serde_json::json!({
            "lines": slice.len(),
            "offset": off,
            "total": total_lines
        })),
        terminate: None,
        failure: None,
    }
}

/// 2. write_file
pub fn write_file(workspace: &Path, path: &str, content: &str) -> ToolResult {
    let full = match check_workspace_sandbox(workspace, path) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e.to_string()),
    };

    let before = fs::read_to_string(&full).unwrap_or_default();

    if let Some(parent) = full.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                return ToolResult::error(format!("创建目录失败: {}", e));
            }
        }
    }

    if let Err(e) = fs::write(&full, content) {
        return ToolResult::error(format!("写入文件失败: {}", e));
    }

    let patch = unified_diff(path, &before, content);
    let line_count = content.lines().count();

    ToolResult {
        ok: true,
        output: format!("已写入 {}（{} 行）", path, line_count),
        patch: if patch.is_empty() { None } else { Some(patch) },
        details: None,
        terminate: None,
        failure: None,
    }
}

/// 3. edit_file
pub fn edit_file(
    workspace: &Path,
    path: &str,
    old_string: Option<&str>,
    new_string: Option<&str>,
    edits: Option<Vec<EditPair>>,
) -> ToolResult {
    let full = match check_workspace_sandbox(workspace, path) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e.to_string()),
    };

    if !full.exists() {
        return ToolResult::error(format!("文件不存在: {}", path));
    }

    let before = match fs::read_to_string(&full) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("读取文件失败: {}", e)),
    };

    let mut edit_list = Vec::new();
    if let Some(list) = edits {
        for item in list {
            if !item.old_string.is_empty() {
                edit_list.push((item.old_string, item.new_string));
            }
        }
    } else if let (Some(old_s), Some(new_s)) = (old_string, new_string) {
        edit_list.push((old_s.to_string(), new_s.to_string()));
    } else {
        return ToolResult::error("必须提供 old_string 与 new_string，或提供 edits 替换列表。");
    }

    let mut after = before.clone();
    for (old_s, new_s) in edit_list {
        let first_idx = match after.find(&old_s) {
            Some(idx) => idx,
            None => {
                return ToolResult::error(format!(
                    "未能替换：原文本在 {} 中不存在。请确认代码上下文。",
                    path
                ));
            }
        };

        let second_search_start = first_idx + old_s.len();
        if after[second_search_start..].contains(&old_s) {
            return ToolResult::error(format!(
                "未能替换：原文本在 {} 中出现了多次，请扩大上下文使其唯一。",
                path
            ));
        }

        let mut replaced = String::with_capacity(after.len() + new_s.len());
        replaced.push_str(&after[..first_idx]);
        replaced.push_str(&new_s);
        replaced.push_str(&after[first_idx + old_s.len()..]);
        after = replaced;
    }

    if let Err(e) = fs::write(&full, &after) {
        return ToolResult::error(format!("保存修改失败: {}", e));
    }

    let patch = unified_diff(path, &before, &after);
    ToolResult {
        ok: true,
        output: format!("已成功修改 {}", path),
        patch: if patch.is_empty() { None } else { Some(patch) },
        details: None,
        terminate: None,
        failure: None,
    }
}

/// 4. list_files
pub fn list_files(workspace: &Path, subpath: Option<&str>, depth: Option<usize>) -> ToolResult {
    let root = resolve_real_path(workspace);
    let target = match subpath {
        Some(p) if !p.trim().is_empty() => match check_workspace_sandbox(workspace, p.trim()) {
            Ok(checked) => checked,
            Err(e) => return ToolResult::error(e.to_string()),
        },
        _ => root.clone(),
    };

    let max_depth = depth.unwrap_or(3).clamp(1, 6);
    let mut entries = Vec::new();
    let mut budget = MAX_LIST_ENTRIES;

    walk_dir(&root, &target, 1, max_depth, &mut budget, &mut entries);

    if entries.is_empty() {
        return ToolResult::success("(空目录)");
    }

    let output = entries.join("\n");
    ToolResult::success(output)
}

fn walk_dir(
    root: &Path,
    current: &Path,
    current_depth: usize,
    max_depth: usize,
    budget: &mut usize,
    output: &mut Vec<String>,
) {
    if *budget == 0 {
        return;
    }

    let dir_entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut items = Vec::new();
    for entry in dir_entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name != ".github" {
            continue;
        }
        items.push(entry);
    }

    // 目录在前，同类按名称字典序
    items.sort_by(|a, b| {
        let is_dir_a = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_dir_b = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        match (is_dir_a, is_dir_b) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    for item in items {
        if *budget == 0 {
            break;
        }

        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);

        let rel_path = match path.strip_prefix(root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        if is_dir {
            if should_skip_dir(&name) {
                continue;
            }
            *budget -= 1;
            output.push(format!("{}/", rel_path));

            if current_depth < max_depth {
                walk_dir(root, &path, current_depth + 1, max_depth, budget, output);
            }
        } else {
            *budget -= 1;
            let size = item.metadata().map(|m| m.len()).unwrap_or(0);
            let size_kb = (size as f64 / 1024.0).ceil() as u64;
            output.push(format!("{}  {}k", rel_path, size_kb.max(1)));
        }
    }
}

/// 5. search_files
pub fn search_files(
    workspace: &Path,
    pattern: &str,
    glob: Option<&str>,
    path: Option<&str>,
    literal: bool,
    case_sensitive: bool,
    context: usize,
) -> ToolResult {
    let source = pattern.trim();
    if source.is_empty() {
        return ToolResult::error("缺少 pattern 搜索内容参数。");
    }

    let root = resolve_real_path(workspace);
    let target = match path {
        Some(p) if !p.trim().is_empty() => match check_workspace_sandbox(workspace, p.trim()) {
            Ok(checked) => checked,
            Err(e) => return ToolResult::error(e.to_string()),
        },
        _ => root.clone(),
    };

    let mut matches = Vec::new();
    let mut match_count = 0;

    let ext_filter = glob.map(|g| g.trim_start_matches('.').to_lowercase());
    let max_context = context.min(3);

    search_walk(
        &root,
        &target,
        source,
        ext_filter.as_deref(),
        literal,
        case_sensitive,
        max_context,
        &mut match_count,
        &mut matches,
    );

    if matches.is_empty() {
        return ToolResult::success("未找到匹配内容。");
    }

    let output = matches.join("\n");
    ToolResult::success(output)
}

fn search_walk(
    root: &Path,
    current: &Path,
    pattern: &str,
    ext_filter: Option<&str>,
    literal: bool,
    case_sensitive: bool,
    context_lines: usize,
    match_count: &mut usize,
    output: &mut Vec<String>,
) {
    if *match_count >= MAX_SEARCH_MATCHES {
        return;
    }

    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if *match_count >= MAX_SEARCH_MATCHES {
            break;
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name != ".github" {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if !should_skip_dir(&name) {
                search_walk(
                    root,
                    &path,
                    pattern,
                    ext_filter,
                    literal,
                    case_sensitive,
                    context_lines,
                    match_count,
                    output,
                );
            }
        } else {
            if let Some(ext) = ext_filter {
                let file_ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if file_ext != ext {
                    continue;
                }
            }

            if let Ok(content) = read_text_file(&path) {
                let lines: Vec<&str> = content.lines().collect();
                let rel_path = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());

                for (idx, line) in lines.iter().enumerate() {
                    if *match_count >= MAX_SEARCH_MATCHES {
                        break;
                    }

                    let is_match = if literal {
                        if case_sensitive {
                            line.contains(pattern)
                        } else {
                            line.to_lowercase().contains(&pattern.to_lowercase())
                        }
                    } else {
                        // 简单正则或忽略大小写搜索
                        if case_sensitive {
                            line.contains(pattern)
                        } else {
                            line.to_lowercase().contains(&pattern.to_lowercase())
                        }
                    };

                    if is_match {
                        *match_count += 1;
                        let line_no = idx + 1;

                        if context_lines > 0 {
                            let start = idx.saturating_sub(context_lines);
                            let end = (idx + context_lines + 1).min(lines.len());
                            for ctx_idx in start..end {
                                let prefix = if ctx_idx == idx { ">" } else { " " };
                                output.push(format!(
                                    "{}:{}:{}: {}{}",
                                    rel_path,
                                    ctx_idx + 1,
                                    prefix,
                                    prefix,
                                    lines[ctx_idx]
                                ));
                            }
                        } else {
                            output.push(format!("{}:{}: {}", rel_path, line_no, line));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_operations_lifecycle() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_fs_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)?;

        // 1. 写文件
        let res_write = write_file(
            &temp_dir,
            "test_dir/hello.txt",
            "Hello World\nLine 2\nLine 3\n",
        );
        assert!(res_write.ok);
        assert!(res_write.patch.is_some());

        // 2. 读文件
        let res_read = read_file(&temp_dir, "test_dir/hello.txt", Some(1), Some(2));
        assert!(res_read.ok);
        assert!(res_read.output.contains("1 | Hello World"));
        assert!(res_read.output.contains("2 | Line 2"));
        assert!(!res_read.output.contains("3 | Line 3"));

        // 3. 编辑文件
        let res_edit = edit_file(
            &temp_dir,
            "test_dir/hello.txt",
            Some("Line 2"),
            Some("Line 2 Modified"),
            None,
        );
        assert!(res_edit.ok);
        assert!(res_edit.patch.is_some());

        // 4. 列出文件
        let res_list = list_files(&temp_dir, None, Some(3));
        assert!(res_list.ok);
        assert!(res_list.output.contains("test_dir/"));
        assert!(res_list.output.contains("test_dir/hello.txt"));

        // 5. 搜索文件
        let res_search = search_files(
            &temp_dir,
            "Modified",
            None,
            None,
            true,
            false,
            0,
        );
        assert!(res_search.ok);
        assert!(res_search.output.contains("Line 2 Modified"));

        // 6. 清理
        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
