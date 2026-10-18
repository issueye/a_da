//! 换行符（EOL）归一化：文件工具的**匹配容忍层**。
//!
//! 为什么需要它（真事故，2026-10-18）：`read_file` 用 `str::lines()` 输出，CRLF 文件里的
//! `\r` 根本不会出现在工具输出里——模型看到的、抄回来的 `old_string` 只带 LF，而磁盘上的
//! 文件是 CRLF。`edit_file` 早先直接 `after.find(&old_s)`，于是**跨行匹配必然失败**，
//! 报"原文本在 xxx 中不存在"；模型只能反复换写法重试（实测两次尝试才成功一次）。
//! 反向（LF 文件 + 带 CRLF 的片段）同样失败。
//!
//! 这里的策略一句话：**匹配按 LF 归一化，写回按文件原有 EOL 还原**。
//! 归一化是为了"找得到"，还原是为了"别把整个文件改成另一种换行"
//! （否则 diff 会把每一行都算成改动，评审全是噪声）。

use std::borrow::Cow;

/// Windows 换行
pub const CRLF: &str = "\r\n";
/// Unix 换行
pub const LF: &str = "\n";

/// 文本的**主导**换行：`\r\n` 多于裸 `\n` 时返回 [`CRLF`]，否则 [`LF`]。
///
/// 混排文件取"哪种多算哪种"，不做单行级别的猜测——决定的是**写回**用哪种换行。
pub fn dominant_eol(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    let (mut crlf, mut lf) = (0usize, 0usize);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            crlf += 1;
            i += 2;
        } else {
            if bytes[i] == b'\n' {
                lf += 1;
            }
            i += 1;
        }
    }
    if crlf > lf {
        CRLF
    } else {
        LF
    }
}

/// 把 `\r\n` 与裸 `\r` 一律折成 `\n`（没有 `\r` 时零拷贝）。
pub fn to_lf(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// 把任意换行统一成 `eol`（只接受 `"\n"` 或 `"\r\n"`，其它值按 `"\n"` 处理）。
pub fn apply_eol(text: &str, eol: &str) -> String {
    if eol == CRLF {
        to_lf(text).replace('\n', CRLF)
    } else {
        to_lf(text).into_owned()
    }
}

/// 让 `content` 的换行**跟随** `reference` 的主导换行（`reference` 为空则原样返回）。
///
/// 用途：`write_file` 覆写一个已存在的 CRLF 文件时，别因为模型给的是 LF 就把它整体改成 LF。
pub fn follow_eol<'a>(content: &'a str, reference: &str) -> Cow<'a, str> {
    if reference.is_empty() {
        return Cow::Borrowed(content);
    }
    if dominant_eol(reference) == CRLF {
        // 只有"纯 LF"才需要提升；已经带 CRLF 的内容原样放行（避免二次转换）
        if content.contains('\n') && !content.contains(CRLF) {
            return Cow::Owned(apply_eol(content, CRLF));
        }
        Cow::Borrowed(content)
    } else if content.contains('\r') {
        Cow::Owned(apply_eol(content, LF))
    } else {
        Cow::Borrowed(content)
    }
}

/// LF 归一化视图：折成 LF 的同时**记住每个归一化字节位置对应的原文位置**。
///
/// 为什么不能按长度加减把命中区间换算回原文：`\r\n` 是 2 字节而 `\n` 是 1 字节，
/// 一处 CRLF 就错位 1 字节，跨行片段必然切错位置（切在字符中间会直接 panic）。
pub struct LfView<'a> {
    normalized: Cow<'a, str>,
    /// 归一化文本的字节下标 → 原文字节下标；长度 = `normalized.len() + 1`。
    /// 原文不含 `\r` 时为空（此时归一化文本就是原文，下标一一对应）。
    map: Vec<usize>,
}

impl<'a> LfView<'a> {
    pub fn new(hay: &'a str) -> Self {
        if !hay.contains('\r') {
            return Self {
                normalized: Cow::Borrowed(hay),
                map: Vec::new(),
            };
        }

        let mut normalized = String::with_capacity(hay.len());
        let mut map = Vec::with_capacity(hay.len() + 1);
        let mut chars = hay.char_indices().peekable();
        while let Some((idx, c)) = chars.next() {
            if c == '\r' {
                if chars.peek().map(|(_, n)| *n) == Some('\n') {
                    chars.next();
                }
                normalized.push('\n');
                map.push(idx);
            } else {
                let start = normalized.len();
                normalized.push(c);
                // 多字节字符的每个字节都指向该字符的起始位置（命中点必然是字符边界）
                for _ in start..normalized.len() {
                    map.push(idx);
                }
            }
        }
        map.push(hay.len());

        Self {
            normalized: Cow::Owned(normalized),
            map,
        }
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// 在归一化文本里找**全部**命中，返回**原文**中的字节区间。
    pub fn occurrences(&self, needle_lf: &str) -> Vec<(usize, usize)> {
        if needle_lf.is_empty() {
            return Vec::new();
        }
        if self.map.is_empty() {
            return self
                .normalized
                .match_indices(needle_lf)
                .map(|(s, m)| (s, s + m.len()))
                .collect();
        }
        self.normalized
            .match_indices(needle_lf)
            .map(|(s, m)| (self.map[s], self.map[s + m.len()]))
            .collect()
    }
}

/// `old_string` 是否"把换行写成了字面转义序列"（`\r\n` 四个字符，而不是真实换行）。
///
/// 只在**精确匹配失败之后**才用得上，并且要求整段**不含真实换行**——否则一段正常的
/// Rust 代码（`println!("a\nb")`）会被误判成"漏了转义"，把代码改坏。
pub fn looks_newline_escaped(text: &str) -> bool {
    !text.contains('\n')
        && !text.contains('\r')
        && (text.contains("\\n") || text.contains("\\r"))
}

/// 把字面的 `\r\n` / `\n` / `\r` 转义序列还原成真实换行（先长后短，避免拆错）。
pub fn expand_newline_escapes(text: &str) -> String {
    text.replace("\\r\\n", LF)
        .replace("\\n", LF)
        .replace("\\r", LF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dominant_eol() {
        assert_eq!(dominant_eol("a\r\nb\r\nc"), CRLF);
        assert_eq!(dominant_eol("a\nb\nc"), LF);
        assert_eq!(dominant_eol(""), LF);
        // 混排取多数：2 个 CRLF vs 1 个 LF
        assert_eq!(dominant_eol("a\r\nb\r\nc\nd"), CRLF);
        // 一个 `\r` 被算成 CRLF 时不能再重复计成 LF
        assert_eq!(dominant_eol("\r\n"), CRLF);
    }

    #[test]
    fn test_to_lf_and_apply_eol() {
        assert_eq!(to_lf("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(to_lf("a\rb"), "a\nb");
        assert_eq!(to_lf("a\nb"), "a\nb");
        assert!(matches!(to_lf("a\nb"), Cow::Borrowed(_)));

        assert_eq!(apply_eol("a\nb\n", CRLF), "a\r\nb\r\n");
        assert_eq!(apply_eol("a\r\nb\r\n", CRLF), "a\r\nb\r\n");
        assert_eq!(apply_eol("a\r\nb\r\n", LF), "a\nb\n");
    }

    #[test]
    fn test_follow_eol() {
        assert_eq!(follow_eol("a\nb\n", "x\r\ny\r\n"), "a\r\nb\r\n");
        assert_eq!(follow_eol("a\nb\n", "x\ny\n"), "a\nb\n");
        assert_eq!(follow_eol("a\r\nb\r\n", "x\ny\n"), "a\nb\n");
        // 参考为空（新建文件）：原样返回
        assert_eq!(follow_eol("a\nb\n", ""), "a\nb\n");
    }

    #[test]
    fn test_lf_view_maps_back_to_original_offsets() {
        let hay = "头\r\nbody\r\ntail";
        let view = LfView::new(hay);
        assert_eq!(view.normalized(), "头\nbody\ntail");

        let hits = view.occurrences("body\ntail");
        assert_eq!(hits.len(), 1);
        let (start, end) = hits[0];
        // 必须切在字符边界上，且覆盖到原文里的真实 CRLF
        assert_eq!(&hay[start..end], "body\r\ntail");

        // 多字节字符 + CRLF 组合：区间同样要能切
        let hits_zh = view.occurrences("头\nbody");
        assert_eq!(hits_zh.len(), 1);
        assert_eq!(&hay[hits_zh[0].0..hits_zh[0].1], "头\r\nbody");

        // 命中末尾（映射表最后一项 = 原文长度）
        let tail = view.occurrences("tail");
        assert_eq!(&hay[tail[0].0..tail[0].1], "tail");
    }

    #[test]
    fn test_lf_view_without_cr_uses_raw_offsets() {
        let hay = "aa\nbb\ncc";
        let view = LfView::new(hay);
        let hits = view.occurrences("bb\ncc");
        assert_eq!(&hay[hits[0].0..hits[0].1], "bb\ncc");
    }

    #[test]
    fn test_newline_escape_detection_and_expansion() {
        assert!(looks_newline_escaped(r"a\r\nb"));
        assert!(looks_newline_escaped(r"a\nb"));
        // 有真实换行 → 不猜
        assert!(!looks_newline_escaped("a\nb"));
        // 纯代码里的普通文本没有转义 → 不猜
        assert!(!looks_newline_escaped("let x = 1;"));

        assert_eq!(expand_newline_escapes(r"a\r\nb"), "a\nb");
        assert_eq!(expand_newline_escapes(r"a\nb"), "a\nb");
        assert_eq!(expand_newline_escapes(r"a\rb"), "a\nb");
    }
}
