//! Slang 诊断：位置、错误码、文件与修复提示。
//!
//! 解析、校验、渲染都返回 [`Diagnostic`]；加载器负责补上文件路径。
//! 这样插件页可以把“哪个文件、哪一行、哪一列、为什么”原样显示出来。

use std::fmt;
use std::path::PathBuf;

/// 诊断级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// 源码位置。`line` / `column` 从 1 开始；`start` / `end` 是字节偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            start,
            end: end.max(start),
            line: line.max(1),
            column: column.max(1),
        }
    }

    pub fn point(pos: usize, line: u32, column: u32) -> Self {
        Self::new(pos, pos, line, column)
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            line: self.line,
            column: self.column,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 一条诊断。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub file: Option<PathBuf>,
    pub span: Span,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            file: None,
            span,
            hint: None,
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            file: None,
            span,
            hint: None,
        }
    }

    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }

    /// 面向开发者的多行文本；不包含源码片段（调用方可以用自己的源文件补）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(self.severity.as_str());
        out.push('[');
        out.push_str(self.code);
        out.push_str("]: ");
        out.push_str(&self.message);
        if let Some(file) = &self.file {
            out.push_str(&format!(
                "\n  --> {}:{}:{}",
                file.display(),
                self.span.line,
                self.span.column
            ));
        } else if self.span.line > 0 {
            out.push_str(&format!("\n  --> {}:{}", self.span.line, self.span.column));
        }
        if let Some(hint) = &self.hint {
            out.push_str("\n  = 提示：");
            out.push_str(hint);
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl std::error::Error for Diagnostic {}

/// Slang 的结果别名。
pub type SlangResult<T> = std::result::Result<T, Diagnostic>;

/// 便捷构造错误。
pub fn err<T>(code: &'static str, message: impl Into<String>, span: Span) -> SlangResult<T> {
    Err(Diagnostic::error(code, message, span))
}
