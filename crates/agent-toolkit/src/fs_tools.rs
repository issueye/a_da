use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::diff::unified_diff;
use super::sandbox::{resolve_path, resolve_real_path, should_skip_dir};use crate::newline::{
    apply_eol, dominant_eol, expand_newline_escapes, follow_eol, looks_newline_escaped, to_lf,
    LfView, CRLF,
};

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

/// 工具路径上下文：只解析路径，不裁决访问边界。
///
/// 相对路径以 `root` 为基准；绝对路径与跨出根目录的 `..` 照常解析。
#[derive(Clone, Copy)]
pub struct ToolPaths<'a> {
    root: &'a Path,
}

impl<'a> ToolPaths<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }

    /// 相对路径的基准目录（也是列目录/搜索时"相对显示"的根）。
    pub fn workspace(&self) -> &'a Path {
        self.root
    }

    /// 解析一个模型给出的路径。
    pub fn resolve(&self, raw: &str) -> PathBuf {
        resolve_path(self.root, raw)
    }
}


/// 1. read_file
pub fn read_file(
    paths: ToolPaths<'_>,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> ToolResult {
    let full = paths.resolve(path);

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
///
/// 覆写一个**已存在**的文件时，内容换行会跟随该文件原有的约定（见 [`follow_eol`]）：
/// 否则模型给一串 LF 就能把一个 CRLF 文件整体改成 LF，diff 变成"每行都动了"。
pub fn write_file(paths: ToolPaths<'_>, path: &str, content: &str) -> ToolResult {
    let full = paths.resolve(path);

    let before = fs::read_to_string(&full).unwrap_or_default();
    let content = follow_eol(content, &before);
    let content = content.as_ref();

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
///
/// 匹配策略见 [`crate::newline`]：**按 LF 归一化后定位，写回按文件原有 EOL 还原**。
///
/// 直白的 `after.find(&old_s)` 是行不通的：`read_file` 用 `str::lines()` 输出，CRLF 文件的
/// `\r` 不会出现在工具输出里，模型抄回来的 `old_string` 只带 LF，跨行匹配就必然失败。
pub fn edit_file(
    paths: ToolPaths<'_>,
    path: &str,
    old_string: Option<&str>,
    new_string: Option<&str>,
    edits: Option<Vec<EditPair>>,
) -> ToolResult {
    let full = paths.resolve(path);

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

    let eol = dominant_eol(&before);
    let mut after = before.clone();
    // 这次编辑有没有动用"换行容忍"，用于回执里如实告知（不静默改口径）
    let mut tolerated = false;

    for (old_raw, new_raw) in edit_list {
        // 空 old_string 早先会命中下标 0（"插到文件最前面"），这既不是谁的意图也不好排查
        if old_raw.is_empty() {
            return ToolResult::error(
                "old_string 不能为空：要新增内容请把它并入 new_string，并带上足够定位的上下文。",
            );
        }

        let view = LfView::new(&after);
        let mut needle = to_lf(&old_raw).into_owned();
        let mut replacement_src = new_raw;
        let mut hits = view.occurrences(&needle);
        let mut escaped_fix = false;

        // 二级兜底：old_string 把换行写成了字面 `\r\n`（模型自己转义过头）。
        // 只在"精确匹配落空 + 整段没有真实换行"时才这样解读，见 `looks_newline_escaped`。
        if hits.is_empty() && looks_newline_escaped(&old_raw) {
            let alt = to_lf(&expand_newline_escapes(&old_raw)).into_owned();
            let alt_hits = view.occurrences(&alt);
            if !alt_hits.is_empty() {
                needle = alt;
                hits = alt_hits;
                replacement_src = expand_newline_escapes(&replacement_src);
                escaped_fix = true;
            }
        }

        let (start, end) = match hits.len() {
            0 => return ToolResult::error(not_found_message(path, &old_raw, &after, eol)),
            1 => hits[0],
            n => {
                return ToolResult::error(format!(
                    "未能替换：old_string 在 {} 中出现了 {} 次（已按换行归一化统计），请扩大上下文使其唯一。",
                    path, n
                ))
            }
        };

        // 命中区间来自归一化视图，长度可能和 needle 不同（原文是 CRLF）
        let raw_matched = &after[start..end];
        let matched_lf = to_lf(raw_matched).into_owned();
        debug_assert_eq!(matched_lf, needle, "归一化视图的命中必须与 needle 等值");

        // 替换文本按文件原有换行还原，避免"改一行、整个文件换行全变"的巨型 diff
        let replacement = apply_eol(&replacement_src, eol);
        if escaped_fix || raw_matched != old_raw.as_str() || replacement != replacement_src {
            tolerated = true;
        }
        let mut replaced = String::with_capacity(after.len() + replacement.len());
        replaced.push_str(&after[..start]);
        replaced.push_str(&replacement);
        replaced.push_str(&after[end..]);
        after = replaced;
    }

    if let Err(e) = fs::write(&full, &after) {
        return ToolResult::error(format!("保存修改失败: {}", e));
    }

    let patch = unified_diff(path, &before, &after);
    let suffix = if tolerated {
        format!("（换行已按文件实际的 {} 归一化）", eol_name(eol))
    } else {
        String::new()
    };

    ToolResult {
        ok: true,
        output: format!("已成功修改 {}{}", path, suffix),
        patch: if patch.is_empty() { None } else { Some(patch) },
        details: None,
        terminate: None,
        failure: None,
    }
}

/// 换行的可读名，用于回执与报错文案
fn eol_name(eol: &str) -> &'static str {
    if eol == CRLF {
        "CRLF"
    } else {
        "LF"
    }
}

/// 匹配落空时的报错：说清**试过什么**、文件实际是什么，并给一条可执行的下一步。
///
/// 早先只说"原文本不存在"，模型只能盲试（实测两次尝试才命中一次）。
fn not_found_message(path: &str, old_raw: &str, text: &str, eol: &str) -> String {
    let normalized = to_lf(text);
    let mut msg = format!(
        "未能替换：old_string 在 {} 中不存在（已按 LF 归一化匹配，文件实际换行为 {}）。",
        path,
        eol_name(eol)
    );

    if looks_newline_escaped(old_raw) {
        msg.push_str("注意：old_string 里的换行写成了字面转义序列 `\\r\\n` / `\\n`，请改用真实换行。");
    }

    // 首个像样的行是否出现过：能区分"上下文完全不对"和"只是首尾边界没对齐"
    if let Some(first_line) = old_raw
        .lines()
        .map(str::trim)
        .find(|line| line.chars().count() >= 6)
    {
        if let Some(pos) = normalized.find(first_line) {
            let line_no = normalized[..pos].matches('\n').count() + 1;
            msg.push_str(&format!(
                "提示：old_string 的首个非空行出现在第 {} 行，请据此核对上下文边界（可先用 read_file 取回原文片段）。",
                line_no
            ));
        }
    }

    msg
}

/// 4. list_files
pub fn list_files(paths: ToolPaths<'_>, subpath: Option<&str>, depth: Option<usize>) -> ToolResult {
    let workspace_root = resolve_real_path(paths.workspace());
    let target = match subpath {
        Some(p) if !p.trim().is_empty() => paths.resolve(p.trim()),
        _ => workspace_root.clone(),
    };

    // 相对显示的根：完全访问下列到工作区外时，按目标自身为根显示，
    // 否则 `strip_prefix(workspace)` 全部失配、列表会是空的。
    let root = if target.starts_with(&workspace_root) {
        workspace_root
    } else {
        target.clone()
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
    paths: ToolPaths<'_>,
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

    let workspace_root = resolve_real_path(paths.workspace());
    let target = match path {
        Some(p) if !p.trim().is_empty() => paths.resolve(p.trim()),
        _ => workspace_root.clone(),
    };

    // 与 list_files 同理：工作区外按目标自身为根显示
    let root = if target.starts_with(&workspace_root) {
        workspace_root
    } else {
        target.clone()
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
        let res_write = write_file(ToolPaths::new(&temp_dir),
            "test_dir/hello.txt",
            "Hello World\nLine 2\nLine 3\n",
        );
        assert!(res_write.ok);
        assert!(res_write.patch.is_some());

        // 2. 读文件
        let res_read = read_file(ToolPaths::new(&temp_dir), "test_dir/hello.txt", Some(1), Some(2));
        assert!(res_read.ok);
        assert!(res_read.output.contains("1 | Hello World"));
        assert!(res_read.output.contains("2 | Line 2"));
        assert!(!res_read.output.contains("3 | Line 3"));

        // 3. 编辑文件
        let res_edit = edit_file(ToolPaths::new(&temp_dir),
            "test_dir/hello.txt",
            Some("Line 2"),
            Some("Line 2 Modified"),
            None,
        );
        assert!(res_edit.ok);
        assert!(res_edit.patch.is_some());

        // 4. 列出文件
        let res_list = list_files(ToolPaths::new(&temp_dir), None, Some(3));
        assert!(res_list.ok);
        assert!(res_list.output.contains("test_dir/"));
        assert!(res_list.output.contains("test_dir/hello.txt"));

        // 5. 搜索文件
        let res_search = search_files(ToolPaths::new(&temp_dir),
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

    fn temp_ws() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("a_da_crlf_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_edit_file_matches_lf_needle_on_crlf_file() {
        let ws = temp_ws();
        fs::write(ws.join("win.rs"), "fn a() {\r\n    let x = 1;\r\n}\r\n").unwrap();

        // 模型只会给出 LF（read_file 的输出里没有 \r）
        let res = edit_file(ToolPaths::new(&ws),
            "win.rs",
            Some("let x = 1;\n}"),
            Some("let x = 2;\n}"),
            None,
        );
        assert!(res.ok, "LF 片段必须能匹配 CRLF 文件：{}", res.output);
        assert!(res.output.contains("CRLF"), "回执要说明用了换行归一化");

        let after = fs::read_to_string(ws.join("win.rs")).unwrap();
        // 关键：不能因为这次编辑把文件改成 LF
        assert_eq!(after, "fn a() {\r\n    let x = 2;\r\n}\r\n");
        assert!(!after.contains("\n\n"), "不得产生混合换行");

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_matches_crlf_needle_on_lf_file() {
        let ws = temp_ws();
        fs::write(ws.join("unix.rs"), "fn a() {\n    let x = 1;\n}\n").unwrap();

        let res = edit_file(ToolPaths::new(&ws),
            "unix.rs",
            Some("let x = 1;\r\n}"),
            Some("let x = 3;\r\n}"),
            None,
        );
        assert!(res.ok, "CRLF 片段必须能匹配 LF 文件：{}", res.output);

        let after = fs::read_to_string(ws.join("unix.rs")).unwrap();
        assert_eq!(after, "fn a() {\n    let x = 3;\n}\n");

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_multiline_and_chinese_text_across_crlf() {
        let ws = temp_ws();
        fs::write(
            ws.join("cn.txt"),
            "第一行\r\n第二行\r\n第三行\r\n",
        )
        .unwrap();

        let res = edit_file(ToolPaths::new(&ws),
            "cn.txt",
            Some("第一行\n第二行"),
            Some("第一行改\n第二行改"),
            None,
        );
        assert!(res.ok, "多字节 + CRLF 跨行必须能匹配：{}", res.output);
        assert_eq!(
            fs::read_to_string(ws.join("cn.txt")).unwrap(),
            "第一行改\r\n第二行改\r\n第三行\r\n"
        );

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_expands_literal_crlf_escape_in_needle() {
        let ws = temp_ws();
        fs::write(ws.join("win.rs"), "a();\r\nb();\r\n").unwrap();

        // 模型把换行写成了字面 `\r\n`（四个字符），整段没有真实换行 → 允许按转义解读
        let res = edit_file(ToolPaths::new(&ws),
            "win.rs",
            Some(r"a();\r\nb();"),
            Some(r"a();\r\nc();"),
            None,
        );
        assert!(res.ok, "字面转义应被还原后匹配：{}", res.output);

        let after = fs::read_to_string(ws.join("win.rs")).unwrap();
        assert_eq!(after, "a();\r\nc();\r\n", "不能把字面 \\r\\n 写进文件");

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_reports_multiple_matches_with_count() {
        let ws = temp_ws();
        fs::write(ws.join("dup.txt"), "x\r\nsame\r\ny\r\nsame\r\n").unwrap();

        let res = edit_file(ToolPaths::new(&ws), "dup.txt", Some("same"), Some("other"), None);
        assert!(!res.ok);
        assert!(res.output.contains("2 次"), "报错要说清命中次数：{}", res.output);

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_not_found_gives_line_hint() {
        let ws = temp_ws();
        fs::write(
            ws.join("hint.txt"),
            "line one\r\nlet target = 42;\r\nline three\r\n",
        )
        .unwrap();

        let res = edit_file(ToolPaths::new(&ws),
            "hint.txt",
            Some("let target = 42;\nline four"),
            Some("nope"),
            None,
        );
        assert!(!res.ok);
        assert!(
            res.output.contains("第 2 行"),
            "落空时应指出首行出现过的地方：{}",
            res.output
        );

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_batch_edits_on_crlf_file() {
        let ws = temp_ws();
        fs::write(ws.join("b.rs"), "one\r\ntwo\r\nthree\r\n").unwrap();

        let edits = vec![
            EditPair {
                old_string: "one".into(),
                new_string: "ONE".into(),
            },
            EditPair {
                old_string: "two\nthree".into(),
                new_string: "TWO\nTHREE".into(),
            },
        ];
        let res = edit_file(ToolPaths::new(&ws), "b.rs", None, None, Some(edits));
        assert!(res.ok, "{}", res.output);
        assert_eq!(
            fs::read_to_string(ws.join("b.rs")).unwrap(),
            "ONE\r\nTWO\r\nTHREE\r\n"
        );

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_edit_file_rejects_empty_old_string() {
        let ws = temp_ws();
        fs::write(ws.join("e.txt"), "abc\n").unwrap();

        let res = edit_file(ToolPaths::new(&ws), "e.txt", Some(""), Some("prefix\n"), None);
        assert!(!res.ok);
        assert!(res.output.contains("不能为空"), "{}", res.output);
        assert_eq!(fs::read_to_string(ws.join("e.txt")).unwrap(), "abc\n");

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_write_file_keeps_existing_crlf_convention() {
        let ws = temp_ws();
        fs::write(ws.join("keep.rs"), "a\r\nb\r\n").unwrap();

        let res = write_file(ToolPaths::new(&ws), "keep.rs", "a\nb\nc\n");
        assert!(res.ok);
        assert_eq!(
            fs::read_to_string(ws.join("keep.rs")).unwrap(),
            "a\r\nb\r\nc\r\n",
            "覆写 CRLF 文件时不得整体退化为 LF"
        );

        // 新文件不猜换行：给什么写什么
        let res_new = write_file(ToolPaths::new(&ws), "new.rs", "x\ny\n");
        assert!(res_new.ok);
        assert_eq!(fs::read_to_string(ws.join("new.rs")).unwrap(), "x\ny\n");

        let _ = fs::remove_dir_all(ws);
    }
}
