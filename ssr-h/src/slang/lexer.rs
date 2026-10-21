//! 表达式词法器。
//!
//! 文本/指令的扫描在 [`super::parser`] 里直接做（它需要保留空白与块结构）；
//! 这里只负责“一段表达式源码 → token”。这样 `{{ ... }}`、组件实参、`@if`
//! 条件可以共用同一套表达式语法。

use super::error::{err, SlangResult, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Str(String),
    TripleStr(String),
    /// 反引号模板字符串的原始内容（尚未解析 `{{ }}`）。
    TemplateStr(String),
    Int(i64),
    Float(f64),
    True,
    False,
    Null,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    Semicolon,
    /// 单个 `=`；只给 `@script` 的赋值用，表达式里出现会被拒绝。
    Assign,
    Pipe,
    PipePipe,
    Question,
    QuestionQuestion,
    Bang,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AndAnd,
    OrOr,
    Eof,
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(value) => format!("`{value}`"),
            TokenKind::Str(_) => "字符串".to_string(),
            TokenKind::TripleStr(_) => "多行字符串".to_string(),
            TokenKind::TemplateStr(_) => "模板字符串".to_string(),
            TokenKind::Int(_) => "整数".to_string(),
            TokenKind::Float(_) => "浮点数".to_string(),
            TokenKind::True | TokenKind::False => "布尔值".to_string(),
            TokenKind::Null => "null".to_string(),
            TokenKind::LParen => "`(`".to_string(),
            TokenKind::RParen => "`)`".to_string(),
            TokenKind::LBrace => "`{`".to_string(),
            TokenKind::RBrace => "`}`".to_string(),
            TokenKind::LBracket => "`[`".to_string(),
            TokenKind::RBracket => "`]`".to_string(),
            TokenKind::Comma => "`,`".to_string(),
            TokenKind::Colon => "`:`".to_string(),
            TokenKind::Dot => "`.`".to_string(),
            TokenKind::Semicolon => "`;`".to_string(),
            TokenKind::Assign => "`=`".to_string(),
            TokenKind::Pipe => "`|`".to_string(),
            TokenKind::PipePipe => "`||`".to_string(),
            TokenKind::Question => "`?`".to_string(),
            TokenKind::QuestionQuestion => "`??`".to_string(),
            TokenKind::Bang => "`!`".to_string(),
            TokenKind::Plus => "`+`".to_string(),
            TokenKind::Minus => "`-`".to_string(),
            TokenKind::Star => "`*`".to_string(),
            TokenKind::Slash => "`/`".to_string(),
            TokenKind::Percent => "`%`".to_string(),
            TokenKind::EqEq => "`==`".to_string(),
            TokenKind::NotEq => "`!=`".to_string(),
            TokenKind::Lt => "`<`".to_string(),
            TokenKind::LtEq => "`<=`".to_string(),
            TokenKind::Gt => "`>`".to_string(),
            TokenKind::GtEq => "`>=`".to_string(),
            TokenKind::AndAnd => "`&&`".to_string(),
            TokenKind::OrOr => "`||`".to_string(),
            TokenKind::Eof => "表达式结尾".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn next_token(&mut self) -> SlangResult<Token> {
        self.skip_ws_and_comments()?;
        let start = self.pos;
        let Some(ch) = self.peek_char() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                start,
                end: start,
            });
        };

        let kind = match ch {
            '(' => {
                self.bump();
                TokenKind::LParen
            }
            ')' => {
                self.bump();
                TokenKind::RParen
            }
            '{' => {
                self.bump();
                TokenKind::LBrace
            }
            '}' => {
                self.bump();
                TokenKind::RBrace
            }
            '[' => {
                self.bump();
                TokenKind::LBracket
            }
            ']' => {
                self.bump();
                TokenKind::RBracket
            }
            ',' => {
                self.bump();
                TokenKind::Comma
            }
            ':' => {
                self.bump();
                TokenKind::Colon
            }
            '.' => {
                // 支持 `.5` 这种浮点写法。
                if self
                    .peek_next_char()
                    .is_some_and(|next| next.is_ascii_digit())
                {
                    self.scan_number()?
                } else {
                    self.bump();
                    TokenKind::Dot
                }
            }
            ';' => {
                self.bump();
                TokenKind::Semicolon
            }
            '|' => {
                if self.starts_with("||") {
                    self.bump();
                    self.bump();
                    TokenKind::PipePipe
                } else {
                    self.bump();
                    TokenKind::Pipe
                }
            }
            '?' => {
                if self.starts_with("??") {
                    self.bump();
                    self.bump();
                    TokenKind::QuestionQuestion
                } else {
                    self.bump();
                    TokenKind::Question
                }
            }
            '!' => {
                if self.starts_with("!=") {
                    self.bump();
                    self.bump();
                    TokenKind::NotEq
                } else {
                    self.bump();
                    TokenKind::Bang
                }
            }
            '=' => {
                if self.starts_with("==") {
                    self.bump();
                    self.bump();
                    TokenKind::EqEq
                } else {
                    self.bump();
                    TokenKind::Assign
                }
            }
            '<' => {
                if self.starts_with("<=") {
                    self.bump();
                    self.bump();
                    TokenKind::LtEq
                } else {
                    self.bump();
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.starts_with(">=") {
                    self.bump();
                    self.bump();
                    TokenKind::GtEq
                } else {
                    self.bump();
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.starts_with("&&") {
                    self.bump();
                    self.bump();
                    TokenKind::AndAnd
                } else {
                    return err(
                        "E1001",
                        "单个 `&` 不是合法运算符；逻辑与请用 `&&`",
                        self.span(start, start + 1),
                    );
                }
            }
            '+' => {
                self.bump();
                TokenKind::Plus
            }
            '-' => {
                self.bump();
                TokenKind::Minus
            }
            '*' => {
                self.bump();
                TokenKind::Star
            }
            '/' => {
                self.bump();
                TokenKind::Slash
            }
            '%' => {
                self.bump();
                TokenKind::Percent
            }
            '"' => {
                if self.starts_with("\"\"\"") {
                    TokenKind::TripleStr(self.scan_triple_string()?)
                } else {
                    TokenKind::Str(self.scan_string('"')?)
                }
            }
            '\'' => TokenKind::Str(self.scan_string('\'')?),
            '`' => TokenKind::TemplateStr(self.scan_template_string()?),
            ch if ch.is_ascii_digit() => self.scan_number()?,
            ch if is_ident_start(ch) => {
                let ident = self.scan_ident();
                match ident.as_str() {
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "null" => TokenKind::Null,
                    _ => TokenKind::Ident(ident),
                }
            }
            other => {
                return err(
                    "E1001",
                    format!("表达式里出现非法字符 `{other}`"),
                    self.span(start, start + other.len_utf8()),
                );
            }
        };

        Ok(Token {
            kind,
            start,
            end: self.pos,
        })
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_next_char(&self) -> Option<char> {
        let mut iter = self.src[self.pos..].chars();
        iter.next();
        iter.next()
    }

    fn bump(&mut self) {
        if let Some(ch) = self.peek_char() {
            self.pos += ch.len_utf8();
        }
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.src[self.pos..].starts_with(needle)
    }

    fn skip_ws_and_comments(&mut self) -> SlangResult<()> {
        loop {
            while self.peek_char().is_some_and(|ch| ch.is_whitespace()) {
                self.bump();
            }
            if self.starts_with("//") {
                while let Some(ch) = self.peek_char() {
                    self.bump();
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.starts_with("/*") {
                let start = self.pos;
                self.pos += 2;
                if let Some(offset) = self.src[self.pos..].find("*/") {
                    self.pos += offset + 2;
                } else {
                    return err(
                        "E1002",
                        "块注释没有闭合（缺少 `*/`）",
                        self.span(start, self.pos),
                    );
                }
                continue;
            }
            if self.starts_with("@#") {
                while let Some(ch) = self.peek_char() {
                    self.bump();
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.starts_with("@*") {
                let start = self.pos;
                self.pos += 2;
                if let Some(offset) = self.src[self.pos..].find("*@") {
                    self.pos += offset + 2;
                } else {
                    return err(
                        "E1002",
                        "块注释没有闭合（缺少 `*@`）",
                        self.span(start, self.pos),
                    );
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn scan_ident(&mut self) -> String {
        let start = self.pos;
        while self
            .peek_char()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            self.bump();
        }
        self.src[start..self.pos].to_string()
    }

    fn scan_number(&mut self) -> SlangResult<TokenKind> {
        let start = self.pos;
        let mut has_dot = false;
        let mut has_exp = false;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                self.bump();
            } else if ch == '.' && !has_dot && !has_exp {
                // 后面必须跟数字，避免把 `1.foo` 吃掉。
                if self.src[self.pos + 1..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_digit())
                {
                    has_dot = true;
                    self.bump();
                } else {
                    break;
                }
            } else if (ch == 'e' || ch == 'E') && !has_exp {
                has_exp = true;
                self.bump();
                if self
                    .peek_char()
                    .is_some_and(|sign| sign == '+' || sign == '-')
                {
                    self.bump();
                }
            } else {
                break;
            }
        }
        let raw = &self.src[start..self.pos];
        if has_dot || has_exp {
            raw.parse::<f64>().map(TokenKind::Float).map_err(|_| {
                super::error::Diagnostic::error(
                    "E1001",
                    format!("非法数字 `{raw}`"),
                    self.span(start, self.pos),
                )
            })
        } else {
            raw.parse::<i64>().map(TokenKind::Int).map_err(|_| {
                super::error::Diagnostic::error(
                    "E1001",
                    format!("整数 `{raw}` 超出范围"),
                    self.span(start, self.pos),
                )
            })
        }
    }

    fn scan_string(&mut self, quote: char) -> SlangResult<String> {
        let start = self.pos;
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return err("E1002", "字符串没有闭合", self.span(start, self.pos));
            };
            if ch == quote {
                self.bump();
                return Ok(out);
            }
            if ch == '\\' {
                self.bump();
                out.push(self.scan_escape(start)?);
            } else {
                out.push(ch);
                self.bump();
            }
        }
    }

    fn scan_triple_string(&mut self) -> SlangResult<String> {
        let start = self.pos;
        self.pos += 3;
        if let Some(offset) = self.src[self.pos..].find("\"\"\"") {
            let text = self.src[self.pos..self.pos + offset].to_string();
            self.pos += offset + 3;
            Ok(text)
        } else {
            err(
                "E1002",
                "多行字符串没有闭合（缺少 `\"\"\"`）",
                self.span(start, self.pos),
            )
        }
    }

    fn scan_template_string(&mut self) -> SlangResult<String> {
        let start = self.pos;
        self.bump(); // opening backtick
        let mut out = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return err(
                    "E1002",
                    "模板字符串没有闭合（缺少反引号）",
                    self.span(start, self.pos),
                );
            };
            if ch == '`' {
                self.bump();
                return Ok(out);
            }
            if ch == '\\' {
                self.bump();
                match self.peek_char() {
                    Some('`') | Some('\\') => {
                        out.push(self.peek_char().unwrap());
                        self.bump();
                    }
                    Some('{') => {
                        // 保留反斜杠，交给模板字符串解析器把 `\{{` 当字面量。
                        out.push('\\');
                        out.push('{');
                        self.bump();
                    }
                    Some(next) => {
                        return err(
                            "E1003",
                            format!("模板字符串里不认识的转义 `\\{next}`"),
                            self.span(self.pos.saturating_sub(1), self.pos + next.len_utf8()),
                        );
                    }
                    None => {
                        return err("E1002", "模板字符串没有闭合", self.span(start, self.pos));
                    }
                }
            } else {
                out.push(ch);
                self.bump();
            }
        }
    }

    fn scan_escape(&mut self, string_start: usize) -> SlangResult<char> {
        let Some(ch) = self.peek_char() else {
            return err("E1002", "字符串没有闭合", self.span(string_start, self.pos));
        };
        self.bump();
        let value = match ch {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' => '\\',
            '"' => '"',
            '\'' => '\'',
            '{' => '{',
            '}' => '}',
            'u' => {
                if self.peek_char() != Some('{') {
                    return err(
                        "E1003",
                        "`\\u` 转义必须写成 `\\u{1F600}`",
                        self.span(self.pos.saturating_sub(2), self.pos),
                    );
                }
                self.bump();
                let digits_start = self.pos;
                while self.peek_char().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.bump();
                }
                let digits = &self.src[digits_start..self.pos];
                if self.peek_char() != Some('}') {
                    return err(
                        "E1003",
                        "`\\u{...}` 缺少 `}`",
                        self.span(digits_start, self.pos),
                    );
                }
                self.bump();
                let code = u32::from_str_radix(digits, 16).map_err(|_| {
                    super::error::Diagnostic::error(
                        "E1003",
                        format!("非法的 Unicode 转义 `\\u{{{digits}}}`"),
                        self.span(digits_start, self.pos),
                    )
                })?;
                char::from_u32(code).ok_or_else(|| {
                    super::error::Diagnostic::error(
                        "E1003",
                        format!("`\\u{{{digits}}}` 不是合法字符"),
                        self.span(digits_start, self.pos),
                    )
                })?
            }
            other => {
                return err(
                    "E1003",
                    format!("不认识的转义 `\\{other}`"),
                    self.span(self.pos.saturating_sub(2), self.pos),
                );
            }
        };
        Ok(value)
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(start, end, 1, 1)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}
