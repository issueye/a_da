use oxc_diagnostics::OxcDiagnostic;

pub(super) fn render_oxc_diagnostics(
    source: &str,
    filename: Option<&str>,
    diagnostics: &[OxcDiagnostic],
) -> String {
    let file = filename.unwrap_or("<inline>");
    let mut rendered = Vec::new();
    for d in diagnostics {
        let mut block = d.to_string();
        // 取第一条带 span 的标签来定位（oxc 的解析/语义诊断都至少带一条）
        if let Some(label) = d.labels.first() {
            let offset = label.offset() as usize;
            let (line_no, col_no) = line_col_of(source, offset);
            block.push_str(&format!("\n  → {file}:{line_no}:{col_no}"));
            if let Some(text) = source.lines().nth(line_no.saturating_sub(1)) {
                block.push_str(&format!("\n  | {text}"));
                // 插入符按**字符**数对齐（列号是从 1 开始的字符位置）
                block.push_str(&format!("\n  | {}^", " ".repeat(col_no.saturating_sub(1))));
            }
        }
        rendered.push(block);
    }
    rendered.join("\n")
}

/// 字节偏移 → `(行号, 列号)`，都从 1 开始（给人看的，与编辑器一致）。
///
/// 列号按**字符**算而不是字节：中文注释在插件里很常见，按字节会得到"第 37 列"这种
/// 编辑器里根本对不上的数字。
pub(super) fn line_col_of(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    let col = source[line_start..offset].chars().count() + 1;
    (line, col)
}
