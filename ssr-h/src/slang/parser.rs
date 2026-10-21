//! Slang 解析器。
//!
//! 分两层：
//!
//! - [`TextParser`]：扫描正文/块结构，保留空白，识别 `{{ }}`、`::组件`、`@指令`；
//! - [`ExprParser`]：解析一段表达式源码（`{{ }}`、实参、条件共用）。
//!
//! 后者复用 [`super::lexer::Lexer`]，前者直接按字符推进——文本模式必须保留空白，
//! 不能先做“全局去空白”。

use std::path::PathBuf;

use super::ast::*;
use super::error::{err, Diagnostic, SlangResult, Span};
use super::lexer::{Lexer, Token, TokenKind};
use super::script::ScriptParser;

// ─────────────────────────────────────────────────────────────────────────────
// 表达式解析
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExprParser<'a> {
    src: &'a str,
    lexer: Lexer<'a>,
    peeked: Option<Token>,
    base_offset: usize,
    base_line: u32,
    base_column: u32,
}

impl<'a> ExprParser<'a> {
    pub fn new(src: &'a str, base_offset: usize, base_line: u32, base_column: u32) -> Self {
        Self {
            src,
            lexer: Lexer::new(src),
            peeked: None,
            base_offset,
            base_line: base_line.max(1),
            base_column: base_column.max(1),
        }
    }

    pub fn parse_expression(mut self) -> SlangResult<Expr> {
        let expr = self.parse_pipe()?;
        let token = self.peek()?.clone();
        if !matches!(token.kind, TokenKind::Eof) {
            return err(
                "E2001",
                format!("表达式结束后多出 {}", token.kind.describe()),
                self.span(token.start, token.end),
            );
        }
        Ok(expr)
    }

    fn parse_pipe(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_coalesce()?;
        while self.eat_simple(TokenKind::Pipe) {
            let token = self.next()?;
            let name = match token.kind {
                TokenKind::Ident(name) => name,
                other => {
                    return err(
                        "E2002",
                        format!("`|` 后面需要函数名，得到 {}", other.describe()),
                        self.span(token.start, token.end),
                    )
                }
            };
            let mut args = Vec::new();
            if self.eat_simple(TokenKind::LParen) {
                args = self.parse_call_args_until_rparen()?;
            }
            let span = expr.span.merge(self.span(token.start, token.end));
            expr = Expr {
                kind: ExprKind::Filter(name, Box::new(expr), args),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_coalesce(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_ternary()?;
        while self.eat_simple(TokenKind::QuestionQuestion) {
            let right = self.parse_ternary()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(BinOp::Coalesce, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_ternary(&mut self) -> SlangResult<Expr> {
        let condition = self.parse_or()?;
        if self.eat_simple(TokenKind::Question) {
            let then_branch = self.parse_pipe()?;
            let colon = self.next()?;
            if !matches!(colon.kind, TokenKind::Colon) {
                return err(
                    "E2001",
                    format!("三元表达式缺少 `:`，得到 {}", colon.kind.describe()),
                    self.span(colon.start, colon.end),
                );
            }
            let else_branch = self.parse_ternary()?;
            let span = condition
                .span
                .merge(then_branch.span)
                .merge(else_branch.span);
            return Ok(Expr {
                kind: ExprKind::Ternary(
                    Box::new(condition),
                    Box::new(then_branch),
                    Box::new(else_branch),
                ),
                span,
            });
        }
        Ok(condition)
    }

    fn parse_or(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_and()?;
        while self.eat_simple(TokenKind::OrOr) {
            let right = self.parse_and()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(BinOp::Or, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_equality()?;
        while self.eat_simple(TokenKind::AndAnd) {
            let right = self.parse_equality()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(BinOp::And, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_comparison()?;
        loop {
            let op = if self.eat_simple(TokenKind::EqEq) {
                Some(BinOp::Eq)
            } else if self.eat_simple(TokenKind::NotEq) {
                Some(BinOp::Ne)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_comparison()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(op, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_additive()?;
        loop {
            let op = if self.eat_simple(TokenKind::Lt) {
                Some(BinOp::Lt)
            } else if self.eat_simple(TokenKind::LtEq) {
                Some(BinOp::Le)
            } else if self.eat_simple(TokenKind::Gt) {
                Some(BinOp::Gt)
            } else if self.eat_simple(TokenKind::GtEq) {
                Some(BinOp::Ge)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_additive()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(op, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_additive(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_multiplicative()?;
        loop {
            let op = if self.eat_simple(TokenKind::Plus) {
                Some(BinOp::Add)
            } else if self.eat_simple(TokenKind::Minus) {
                Some(BinOp::Sub)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_multiplicative()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(op, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = if self.eat_simple(TokenKind::Star) {
                Some(BinOp::Mul)
            } else if self.eat_simple(TokenKind::Slash) {
                Some(BinOp::Div)
            } else if self.eat_simple(TokenKind::Percent) {
                Some(BinOp::Mod)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_unary()?;
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary(op, Box::new(expr), Box::new(right)),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> SlangResult<Expr> {
        if let Some(token) = self.eat_simple_token(TokenKind::Bang) {
            let value = self.parse_unary()?;
            let span = self.span(token.start, token.end).merge(value.span);
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(value)),
                span,
            });
        }
        if let Some(token) = self.eat_simple_token(TokenKind::Minus) {
            let value = self.parse_unary()?;
            let span = self.span(token.start, token.end).merge(value.span);
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(value)),
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> SlangResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat_simple(TokenKind::Dot) {
                let token = self.next()?;
                let name = match token.kind {
                    TokenKind::Ident(name) => name,
                    other => {
                        return err(
                            "E2001",
                            format!("`.` 后面需要字段名，得到 {}", other.describe()),
                            self.span(token.start, token.end),
                        )
                    }
                };
                let span = expr.span.merge(self.span(token.start, token.end));
                expr = Expr {
                    kind: ExprKind::Field(Box::new(expr), name),
                    span,
                };
                continue;
            }
            if self.eat_simple(TokenKind::LBracket) {
                let index = self.parse_pipe()?;
                let close = self.next()?;
                if !matches!(close.kind, TokenKind::RBracket) {
                    return err(
                        "E2001",
                        format!("索引缺少 `]`，得到 {}", close.kind.describe()),
                        self.span(close.start, close.end),
                    );
                }
                let span = expr.span.merge(self.span(close.start, close.end));
                expr = Expr {
                    kind: ExprKind::Index(Box::new(expr), Box::new(index)),
                    span,
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> SlangResult<Expr> {
        let token = self.next()?;
        let span = self.span(token.start, token.end);
        match token.kind {
            TokenKind::Null => Ok(Expr {
                kind: ExprKind::Null,
                span,
            }),
            TokenKind::True => Ok(Expr {
                kind: ExprKind::Bool(true),
                span,
            }),
            TokenKind::False => Ok(Expr {
                kind: ExprKind::Bool(false),
                span,
            }),
            TokenKind::Int(value) => Ok(Expr {
                kind: ExprKind::Int(value),
                span,
            }),
            TokenKind::Float(value) => Ok(Expr {
                kind: ExprKind::Float(value),
                span,
            }),
            TokenKind::Str(value) => Ok(Expr {
                kind: ExprKind::Str(value),
                span,
            }),
            TokenKind::TripleStr(value) => Ok(Expr {
                kind: ExprKind::Str(value),
                span,
            }),
            TokenKind::TemplateStr(raw) => {
                let parts = self.parse_template_parts(&raw, span)?;
                Ok(Expr {
                    kind: ExprKind::Template(parts),
                    span,
                })
            }
            TokenKind::Ident(name) => {
                if self.eat_simple(TokenKind::LParen) {
                    let args = self.parse_call_args_until_rparen()?;
                    let close_span = self.last_span();
                    return Ok(Expr {
                        kind: ExprKind::Call(name, args),
                        span: span.merge(close_span),
                    });
                }
                Ok(Expr {
                    kind: ExprKind::Ident(name),
                    span,
                })
            }
            TokenKind::LParen => {
                let value = self.parse_pipe()?;
                let close = self.next()?;
                if !matches!(close.kind, TokenKind::RParen) {
                    return err(
                        "E2001",
                        format!("括号没有闭合，期望 `)`，得到 {}", close.kind.describe()),
                        self.span(close.start, close.end),
                    );
                }
                Ok(Expr {
                    kind: value.kind,
                    span: span.merge(self.span(close.start, close.end)),
                })
            }
            TokenKind::LBracket => {
                let mut items = Vec::new();
                if !self.check_simple(TokenKind::RBracket) {
                    loop {
                        items.push(self.parse_pipe()?);
                        if self.eat_simple(TokenKind::Comma) {
                            if self.check_simple(TokenKind::RBracket) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                let close = self.next()?;
                if !matches!(close.kind, TokenKind::RBracket) {
                    return err(
                        "E2001",
                        format!("数组没有闭合，期望 `]`，得到 {}", close.kind.describe()),
                        self.span(close.start, close.end),
                    );
                }
                Ok(Expr {
                    kind: ExprKind::List(items),
                    span: span.merge(self.span(close.start, close.end)),
                })
            }
            TokenKind::LBrace => {
                let mut fields = Vec::new();
                if !self.check_simple(TokenKind::RBrace) {
                    loop {
                        let key_token = self.next()?;
                        let key = match key_token.kind {
                            TokenKind::Ident(key) => key,
                            TokenKind::Str(key) => key,
                            other => {
                                return err(
                                    "E2001",
                                    format!(
                                        "对象字面量的键需要标识符或字符串，得到 {}",
                                        other.describe()
                                    ),
                                    self.span(key_token.start, key_token.end),
                                )
                            }
                        };
                        let colon = self.next()?;
                        if !matches!(colon.kind, TokenKind::Colon) {
                            return err(
                                "E2001",
                                format!("对象字面量缺少 `:`，得到 {}", colon.kind.describe()),
                                self.span(colon.start, colon.end),
                            );
                        }
                        let value = self.parse_pipe()?;
                        fields.push((key, value));
                        if self.eat_simple(TokenKind::Comma) {
                            if self.check_simple(TokenKind::RBrace) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                let close = self.next()?;
                if !matches!(close.kind, TokenKind::RBrace) {
                    return err(
                        "E2001",
                        format!(
                            "对象字面量没有闭合，期望 `}}`，得到 {}",
                            close.kind.describe()
                        ),
                        self.span(close.start, close.end),
                    );
                }
                Ok(Expr {
                    kind: ExprKind::Object(fields),
                    span: span.merge(self.span(close.start, close.end)),
                })
            }
            other => err(
                "E2002",
                format!("这里需要表达式，得到 {}", other.describe()),
                span,
            ),
        }
    }

    fn parse_call_args_until_rparen(&mut self) -> SlangResult<Vec<Expr>> {
        let mut args = Vec::new();
        if self.check_simple(TokenKind::RParen) {
            self.next()?;
            return Ok(args);
        }
        loop {
            args.push(self.parse_pipe()?);
            if self.eat_simple(TokenKind::Comma) {
                if self.check_simple(TokenKind::RParen) {
                    self.next()?;
                    break;
                }
                continue;
            }
            let close = self.next()?;
            if !matches!(close.kind, TokenKind::RParen) {
                return err(
                    "E2001",
                    format!("函数调用缺少 `)`，得到 {}", close.kind.describe()),
                    self.span(close.start, close.end),
                );
            }
            break;
        }
        Ok(args)
    }

    fn parse_template_parts(&self, raw: &str, token_span: Span) -> SlangResult<Vec<TemplatePart>> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut index = 0usize;
        while index < raw.len() {
            if raw[index..].starts_with("\\{{") {
                text.push_str("{{");
                index += 3;
                continue;
            }
            if raw[index..].starts_with("{{") {
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                }
                let inner_start = index + 2;
                let Some(inner_end) = find_template_expr_end(raw, inner_start) else {
                    return err("E1002", "模板字符串里的 `{{` 没有闭合", token_span);
                };
                let inner = &raw[inner_start..inner_end];
                let base = token_span.start + 1 + inner_start;
                let expr = ExprParser::new(inner, base, self.base_line, self.base_column)
                    .parse_expression()?;
                parts.push(TemplatePart::Expr(expr));
                index = inner_end + 2;
                continue;
            }
            let ch = raw[index..].chars().next().unwrap_or(' ');
            text.push(ch);
            index += ch.len_utf8();
        }
        if !text.is_empty() {
            parts.push(TemplatePart::Text(text));
        }
        Ok(parts)
    }

    fn peek(&mut self) -> SlangResult<&Token> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lexer.next_token()?);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    fn next(&mut self) -> SlangResult<Token> {
        if let Some(token) = self.peeked.take() {
            return Ok(token);
        }
        self.lexer.next_token()
    }

    fn check_simple(&mut self, expected: TokenKind) -> bool {
        self.peek()
            .map(|token| token.kind == expected)
            .unwrap_or(false)
    }

    fn eat_simple(&mut self, expected: TokenKind) -> bool {
        if self.check_simple(expected.clone()) {
            let _ = self.next();
            true
        } else {
            false
        }
    }

    fn eat_simple_token(&mut self, expected: TokenKind) -> Option<Token> {
        if self.check_simple(expected) {
            self.next().ok()
        } else {
            None
        }
    }

    fn last_span(&mut self) -> Span {
        // 用 lexer 当前位置做一个兜底：调用点通常已经消费了闭括号。
        let pos = self.lexer.position();
        self.span(pos, pos)
    }

    fn span(&self, start: usize, end: usize) -> Span {
        let start = start.min(self.src.len());
        let end = end.min(self.src.len());
        let prefix = &self.src[..start];
        let line_breaks = prefix.matches('\n').count() as u32;
        let line = self.base_line + line_breaks;
        let column = if line_breaks == 0 {
            self.base_column + prefix.chars().count() as u32
        } else {
            prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1
        };
        Span::new(
            self.base_offset + start,
            self.base_offset + end,
            line,
            column,
        )
    }
}

/// 找到模板字符串里与 `{{` 对应的 `}}`（跳过字符串与嵌套花括号）。
fn find_template_expr_end(raw: &str, start: usize) -> Option<usize> {
    let mut index = start;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while index < raw.len() {
        let ch = raw[index..].chars().next()?;
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            index += ch.len_utf8();
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    if raw[index..].starts_with("}}") {
                        return Some(index);
                    }
                } else {
                    depth -= 1;
                }
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// 文本/块解析
// ─────────────────────────────────────────────────────────────────────────────

pub struct TextParser<'a> {
    src: &'a str,
    pos: usize,
    line: u32,
    column: u32,
    file: Option<PathBuf>,
    depth: usize,
}

#[derive(Debug)]
enum Item {
    Node(Node),
    Def(Definition),
    /// 顶层结构块（`@def` / `@script`）出现的位置：交给 `parse_program` 处理。
    Boundary,
}

impl<'a> TextParser<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            column: 1,
            file: None,
            depth: 0,
        }
    }

    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn parse_program(mut self) -> SlangResult<Program> {
        let mut version = 2u32;
        self.skip_code_ws_and_comments()?;
        if self.keyword_at("@slang") {
            let start = self.pos;
            self.advance_by(6);
            self.skip_inline_ws();
            let number_start = self.pos;
            while self.peek_char().is_some_and(|ch| ch.is_ascii_digit()) {
                self.bump();
            }
            let raw = &self.src[number_start..self.pos];
            if raw.is_empty() {
                return err(
                    "E2001",
                    "`@slang` 后面需要版本号，例如 `@slang 2`",
                    self.span_from(start, start, 1, 1),
                );
            }
            version = raw.parse::<u32>().map_err(|_| {
                Diagnostic::error(
                    "E2001",
                    format!("非法的 Slang 版本 `{raw}`"),
                    self.span_from(number_start, self.pos, self.line, self.column),
                )
            })?;
            if version != 2 {
                return err(
                    "E2001",
                    format!("不支持的 Slang 版本 `{version}`；当前只支持 `slang/2`（v1 已移除）"),
                    self.span_from(start, self.pos, self.line, self.column),
                );
            }
        }

        let mut nodes = Vec::new();
        let mut defs = Vec::new();
        let mut module_scripts = Vec::new();
        // 只清理文件开头的空白；进入循环后**不再**逐项跳过，
        // 否则 `{{ a }} {{ b }}` 之间的空格会被吃掉。
        self.skip_code_ws_and_comments()?;
        loop {
            if self.is_eof() {
                break;
            }
            if self.keyword_at("@def") {
                defs.push(self.parse_definition()?);
                continue;
            }
            if self.keyword_at("@script") {
                module_scripts.push(self.parse_script_block()?);
                continue;
            }
            match self.parse_item(false)? {
                Some(Item::Node(node)) => nodes.push(node),
                Some(Item::Boundary) => continue,
                Some(Item::Def(_)) => {
                    return err(
                        "E2001",
                        "`@def` 只能出现在顶层",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    )
                }
                None => break,
            }
        }
        Ok(Program {
            version,
            nodes,
            defs,
            module_scripts,
        })
    }

    fn parse_item(&mut self, in_block: bool) -> SlangResult<Option<Item>> {
        if self.is_eof() {
            return Ok(None);
        }
        if in_block && self.peek_char() == Some('}') {
            return Ok(None);
        }

        if self.starts_with("@#") {
            self.skip_line_comment();
            return self.parse_item(in_block);
        }
        if self.starts_with("@*") {
            self.skip_block_comment()?;
            return self.parse_item(in_block);
        }
        if self.keyword_at("@props") {
            if in_block {
                return Ok(Some(Item::Boundary));
            }
            return err(
                "E2001",
                "`@props` 只能出现在 `@def` 里",
                self.span_from(self.pos, self.pos + 6, self.line, self.column),
            );
        }
        if self.keyword_at("@def") || self.keyword_at("@script") {
            if in_block {
                return err(
                    "E2001",
                    "`@def` / `@script` 只能出现在文件顶层，不能写在块里",
                    self.span_from(self.pos, self.pos + 4, self.line, self.column),
                );
            }
            // 顶层结构块之前可能隔着空白/注释：交给外层循环按关键字重新分派。
            return Ok(Some(Item::Boundary));
        }

        if self.starts_with("{{") {
            return Ok(Some(Item::Node(self.parse_interpolation()?)));
        }

        if self.starts_with("::") && self.peek_char_after(2).is_some_and(is_ident_start) {
            return Ok(Some(Item::Node(self.parse_component()?)));
        }

        if let Some(_name) = self.directive_name_at() {
            return Ok(Some(Item::Node(self.parse_directive()?)));
        }

        if self.peek_char() == Some('\\') {
            return Ok(Some(Item::Node(self.parse_escape()?)));
        }

        let text = self.scan_text(in_block);
        if text.is_empty() {
            // 防御：理论上 scan_text 至少会推进一个字符；避免死循环。
            let start = self.pos;
            let Some(ch) = self.peek_char() else {
                return Ok(None);
            };
            self.bump();
            return Ok(Some(Item::Node(Node {
                kind: NodeKind::Text(ch.to_string()),
                span: self.span_from(start, self.pos, self.line, self.column),
            })));
        }
        let start = self.pos - text.len();
        Ok(Some(Item::Node(Node {
            kind: NodeKind::Text(text),
            span: self.span_from(start, self.pos, self.line, self.column),
        })))
    }

    fn parse_definition(&mut self) -> SlangResult<Definition> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.advance_by(4); // @def
        self.skip_code_ws_and_comments()?;
        let name = self.parse_ident(true)?;
        self.skip_code_ws_and_comments()?;
        if self.peek_char() == Some('(') {
            return err(
                "E2001",
                "`@def` 不接受参数列表；请在组件体里用 `@props { ... }` 声明 props",
                self.span_from(self.pos, self.pos, self.line, self.column),
            );
        }
        let (props, body) = self.parse_definition_body()?;
        Ok(Definition {
            name,
            props,
            body,
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    /// 解析 `@def` 的块体：可选的 `@props { ... }` + 模板节点。
    fn parse_definition_body(&mut self) -> SlangResult<(Option<Expr>, Vec<Node>)> {
        self.skip_code_ws_and_comments()?;
        self.expect_char('{', "`@def` 后面需要 `{`")?;
        self.skip_code_ws_and_comments()?;
        self.depth += 1;
        if self.depth > 64 {
            self.depth -= 1;
            return err(
                "E6001",
                "块嵌套超过 64 层".to_string(),
                self.span_from(self.pos, self.pos, self.line, self.column),
            );
        }
        let mut props: Option<Expr> = None;
        let mut nodes = Vec::new();
        loop {
            if self.is_eof() {
                self.depth -= 1;
                return err(
                    "E2004",
                    "`@def` 块没有闭合（缺少 `}`）",
                    self.span_from(self.pos, self.pos, self.line, self.column),
                );
            }
            if self.peek_char() == Some('}') {
                self.bump();
                break;
            }
            if self.keyword_at("@props") {
                if props.is_some() {
                    self.depth -= 1;
                    return err(
                        "E2001",
                        "一个组件只能有一个 `@props` 块",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    );
                }
                props = Some(self.parse_props_block()?);
                continue;
            }
            if self.keyword_at("@script") {
                self.depth -= 1;
                return err(
                    "E2001",
                    "`@script` 只能出现在文件顶层；组件里不再支持 `@script setup`，props 用 `@props`",
                    self.span_from(self.pos, self.pos, self.line, self.column),
                );
            }
            match self.parse_item(true)? {
                Some(Item::Node(node)) => nodes.push(node),
                Some(Item::Boundary) => {
                    if self.keyword_at("@props") || self.keyword_at("@script") {
                        continue;
                    }
                    self.depth -= 1;
                    return err(
                        "E2001",
                        "`@def` 只能出现在顶层，不能嵌套",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    );
                }
                Some(Item::Def(_)) => {
                    self.depth -= 1;
                    return err(
                        "E2001",
                        "`@def` 只能出现在顶层，不能嵌套",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    );
                }
                None => break,
            }
        }
        self.depth -= 1;
        Ok((props, nodes))
    }

    /// `@props { label: String, tone: { type: String, default: "neutral" } }`
    ///
    /// 块体就是对象字面量，复用表达式解析器；编译期再转成 props 契约。
    fn parse_props_block(&mut self) -> SlangResult<Expr> {
        self.advance_by("@props".len());
        self.skip_code_ws_and_comments()?;
        let brace_start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.expect_char('{', "`@props` 后面需要 `{`")?;
        let _body = self.read_balanced_brace_body()?;
        let source = &self.src[brace_start..self.pos];
        ExprParser::new(source, brace_start, start_line, start_col).parse_expression()
    }

    fn parse_script_block(&mut self) -> SlangResult<ScriptBlock> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.advance_by("@script".len());
        self.skip_code_ws_and_comments()?;
        if self.keyword_at("setup") {
            return err(
                "E2001",
                "`@script setup` 已移除；props 用 `@props { ... }`，共用逻辑放模块级 `@script`",
                self.span_from(self.pos, self.pos + "setup".len(), self.line, self.column),
            );
        }
        self.expect_char('{', "`@script` 后面需要 `{`")?;
        let body_start = self.pos;
        let (body_line, body_col) = (self.line, self.column);
        let body = self.read_balanced_brace_body()?;
        let statements =
            ScriptParser::new(&body, body_start, body_line, body_col)?.parse_statements()?;
        Ok(ScriptBlock {
            statements,
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn read_balanced_brace_body(&mut self) -> SlangResult<String> {
        let start = self.pos;
        let mut index = self.pos;
        let mut depth = 1i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let mut triple = false;
        while index < self.src.len() {
            let ch = self.src[index..].chars().next().unwrap_or(' ');
            if triple {
                if self.src[index..].starts_with("\"\"\"") {
                    triple = false;
                    index += 3;
                } else {
                    index += ch.len_utf8();
                }
                continue;
            }
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                index += ch.len_utf8();
                continue;
            }
            if self.src[index..].starts_with("//") {
                while index < self.src.len() {
                    let ch = self.src[index..].chars().next().unwrap_or(' ');
                    index += ch.len_utf8();
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.src[index..].starts_with("/*") {
                if let Some(offset) = self.src[index + 2..].find("*/") {
                    index += 2 + offset + 2;
                } else {
                    return err(
                        "E7101",
                        "脚本块注释没有闭合（缺少 `*/`）",
                        self.span_from(index, self.src.len(), self.line, self.column),
                    );
                }
                continue;
            }
            if self.src[index..].starts_with("@#") {
                while index < self.src.len() {
                    let ch = self.src[index..].chars().next().unwrap_or(' ');
                    index += ch.len_utf8();
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.src[index..].starts_with("@*") {
                if let Some(offset) = self.src[index + 2..].find("*@") {
                    index += 2 + offset + 2;
                } else {
                    return err(
                        "E7101",
                        "脚本块注释没有闭合（缺少 `*@`）",
                        self.span_from(index, self.src.len(), self.line, self.column),
                    );
                }
                continue;
            }
            if self.src[index..].starts_with("\"\"\"") {
                triple = true;
                index += 3;
                continue;
            }
            match ch {
                '"' | '\'' | '`' => quote = Some(ch),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = self.src[start..index].to_string();
                        self.advance_to(index + 1);
                        return Ok(body);
                    }
                }
                _ => {}
            }
            index += ch.len_utf8();
        }
        err(
            "E7101",
            "`@script {` 没有闭合（缺少 `}`）",
            self.span_from(start, self.src.len(), self.line, self.column),
        )
    }

    fn parse_interpolation(&mut self) -> SlangResult<Node> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.advance_by(2); // {{
        let (expr_src, expr_line, expr_col) = self.read_until_interp_end()?;
        let expr = ExprParser::new(&expr_src, self.pos - expr_src.len(), expr_line, expr_col)
            .parse_expression()?;
        Ok(Node {
            kind: NodeKind::Interp(expr),
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn parse_component(&mut self) -> SlangResult<Node> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.advance_by(2); // ::
        self.skip_code_ws_and_comments()?;
        let name = self.parse_ident(true)?;

        // `(` 与 `{` 之间允许空白；但如果两者都没有，空白属于正文，要还原。
        let save_pos = self.pos;
        let save_line = self.line;
        let save_col = self.column;
        self.skip_code_ws_and_comments()?;

        let mut args = Vec::new();
        if self.peek_char() == Some('(') {
            self.bump();
            let args_start = self.pos;
            let args_line = self.line;
            let args_col = self.column;
            let args_src = self.read_balanced_paren_body()?;
            args = self.parse_args(&args_src, args_start, args_line, args_col)?;
        } else {
            self.pos = save_pos;
            self.line = save_line;
            self.column = save_col;
        }

        // 参数之后再看 `{`；没有就还原空白。
        let after_args_pos = self.pos;
        let after_args_line = self.line;
        let after_args_col = self.column;
        self.skip_code_ws_and_comments()?;
        // `{{` 是插值，不是 children block；否则 `::Icon(...)` 后面紧跟
        // `{{ name }}` 时会把插值当成组件的子块。
        let children = if self.peek_char() == Some('{') && !self.starts_with("{{") {
            self.parse_block()?
        } else {
            self.pos = after_args_pos;
            self.line = after_args_line;
            self.column = after_args_col;
            Vec::new()
        };

        Ok(Node {
            kind: NodeKind::Component(ComponentCall {
                name,
                args,
                children,
                span: self.span_from(start, self.pos, start_line, start_col),
            }),
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn parse_args(
        &self,
        src: &str,
        base_offset: usize,
        base_line: u32,
        base_col: u32,
    ) -> SlangResult<Vec<Arg>> {
        let mut args = Vec::new();
        for (start, end) in split_top_level(src, ',') {
            let raw = src[start..end].trim();
            if raw.is_empty() {
                continue;
            }
            let leading = raw.len() - raw.trim_start().len();
            let arg_offset = base_offset + start + leading;
            let (name, expr_start) = split_named_arg(raw);
            let expr_src = &raw[expr_start..];
            let expr_leading = expr_src.len() - expr_src.trim_start().len();
            let expr = ExprParser::new(
                expr_src.trim(),
                arg_offset + expr_start + expr_leading,
                base_line,
                base_col,
            )
            .parse_expression()?;
            args.push(Arg {
                name,
                value: expr,
                span: Span::new(arg_offset, base_offset + end, base_line, base_col),
            });
        }
        Ok(args)
    }

    fn parse_directive(&mut self) -> SlangResult<Node> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.bump(); // @
        let name_start = self.pos;
        while self
            .peek_char()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            self.bump();
        }
        let name = self.src[name_start..self.pos].to_string();
        match name.as_str() {
            "if" => self.parse_if(start, start_line, start_col),
            "for" => self.parse_for(start, start_line, start_col),
            "let" => self.parse_let(start, start_line, start_col),
            "include" => {
                self.skip_code_ws_and_comments()?;
                let path = self.parse_string_literal()?;
                Ok(Node {
                    kind: NodeKind::Include {
                        path,
                        body: Vec::new(),
                    },
                    span: self.span_from(start, self.pos, start_line, start_col),
                })
            }
            "use" => {
                self.skip_code_ws_and_comments()?;
                let path = self.parse_string_literal()?;
                Ok(Node {
                    kind: NodeKind::Use { path },
                    span: self.span_from(start, self.pos, start_line, start_col),
                })
            }
            "html" => {
                self.skip_code_ws_and_comments()?;
                if self.peek_char() == Some('(') {
                    self.bump();
                    let expr_start = self.pos;
                    let expr_line = self.line;
                    let expr_col = self.column;
                    let src = self.read_balanced_paren_body()?;
                    let expr = ExprParser::new(&src, expr_start, expr_line, expr_col)
                        .parse_expression()?;
                    Ok(Node {
                        kind: NodeKind::Html {
                            body: None,
                            expr: Some(expr),
                        },
                        span: self.span_from(start, self.pos, start_line, start_col),
                    })
                } else {
                    let body = self.parse_block()?;
                    Ok(Node {
                        kind: NodeKind::Html {
                            body: Some(body),
                            expr: None,
                        },
                        span: self.span_from(start, self.pos, start_line, start_col),
                    })
                }
            }
            "md" => {
                self.skip_code_ws_and_comments()?;
                self.expect_char('(', "`@md` 需要 `(` 包住 Markdown 表达式")?;
                let expr_start = self.pos;
                let expr_line = self.line;
                let expr_col = self.column;
                let src = self.read_balanced_paren_body()?;
                let expr =
                    ExprParser::new(&src, expr_start, expr_line, expr_col).parse_expression()?;
                Ok(Node {
                    kind: NodeKind::Md { expr },
                    span: self.span_from(start, self.pos, start_line, start_col),
                })
            }
            "slot" => {
                self.skip_code_ws_and_comments()?;
                let slot_name = self.parse_ident(false)?;
                self.skip_code_ws_and_comments()?;
                let params = self.parse_slot_args()?;
                self.skip_code_ws_and_comments()?;
                let required = if self.keyword_at("required") {
                    self.advance_by("required".len());
                    true
                } else {
                    false
                };
                self.skip_code_ws_and_comments()?;
                let fallback = self.parse_block()?;
                Ok(Node {
                    kind: NodeKind::Slot {
                        name: slot_name,
                        params,
                        required,
                        fallback,
                    },
                    span: self.span_from(start, self.pos, start_line, start_col),
                })
            }
            "fill" => {
                self.skip_code_ws_and_comments()?;
                let slot_name = self.parse_ident(false)?;
                self.skip_code_ws_and_comments()?;
                let params = self.parse_slot_params()?;
                self.skip_code_ws_and_comments()?;
                let body = self.parse_block()?;
                Ok(Node {
                    kind: NodeKind::Fill {
                        name: slot_name,
                        params,
                        body,
                    },
                    span: self.span_from(start, self.pos, start_line, start_col),
                })
            }
            other => err(
                "E2001",
                format!("未知指令 `@{other}`"),
                self.span_from(start, self.pos, start_line, start_col),
            ),
        }
    }

    fn parse_if(&mut self, start: usize, start_line: u32, start_col: u32) -> SlangResult<Node> {
        let mut arms = Vec::new();
        let mut otherwise = Vec::new();
        loop {
            self.skip_code_ws_and_comments()?;
            let (cond_src, cond_line, cond_col) = self.read_until_block_open()?;
            let condition =
                ExprParser::new(&cond_src, self.pos - cond_src.len(), cond_line, cond_col)
                    .parse_expression()?;
            self.skip_code_ws_and_comments()?;
            let body = self.parse_block()?;
            arms.push((condition, body));

            let save_pos = self.pos;
            let save_line = self.line;
            let save_col = self.column;
            self.skip_code_ws_and_comments()?;
            if self.starts_with("@else") {
                self.advance_by(5);
                self.skip_code_ws_and_comments()?;
                if self.keyword_at("if") {
                    self.advance_by(2);
                    continue;
                }
                otherwise = self.parse_block()?;
                break;
            }
            self.pos = save_pos;
            self.line = save_line;
            self.column = save_col;
            break;
        }
        Ok(Node {
            kind: NodeKind::If { arms, otherwise },
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn parse_for(&mut self, start: usize, start_line: u32, start_col: u32) -> SlangResult<Node> {
        self.skip_code_ws_and_comments()?;
        let (item, index) = if self.peek_char() == Some('(') {
            self.bump();
            self.skip_code_ws_and_comments()?;
            let item = self.parse_ident(false)?;
            self.skip_code_ws_and_comments()?;
            self.expect_char(',', "`@for (item, index)` 需要逗号")?;
            self.skip_code_ws_and_comments()?;
            let index = self.parse_ident(false)?;
            self.skip_code_ws_and_comments()?;
            self.expect_char(')', "`@for (item, index)` 缺少 `)`")?;
            (item, Some(index))
        } else {
            (self.parse_ident(false)?, None)
        };
        self.skip_code_ws_and_comments()?;
        if !self.keyword_at("in") {
            return err(
                "E2001",
                "`@for` 变量后面需要 `in`",
                self.span_from(self.pos, self.pos, self.line, self.column),
            );
        }
        self.advance_by(2);
        self.skip_code_ws_and_comments()?;
        let (iter_src, iter_line, iter_col) = self.read_until_block_open()?;
        let iter = ExprParser::new(&iter_src, self.pos - iter_src.len(), iter_line, iter_col)
            .parse_expression()?;
        self.skip_code_ws_and_comments()?;
        let body = self.parse_block()?;

        let save_pos = self.pos;
        let save_line = self.line;
        let save_col = self.column;
        self.skip_code_ws_and_comments()?;
        let otherwise = if self.starts_with("@else") {
            self.advance_by(5);
            self.skip_code_ws_and_comments()?;
            self.parse_block()?
        } else {
            self.pos = save_pos;
            self.line = save_line;
            self.column = save_col;
            Vec::new()
        };

        Ok(Node {
            kind: NodeKind::For {
                item,
                index,
                iter,
                body,
                otherwise,
            },
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn parse_let(&mut self, start: usize, start_line: u32, start_col: u32) -> SlangResult<Node> {
        self.skip_code_ws_and_comments()?;
        let name = self.parse_ident(false)?;
        self.skip_code_ws_and_comments()?;
        self.expect_char('=', "`@let` 需要 `=`")?;
        self.skip_code_ws_and_comments()?;
        let (expr_src, expr_line, expr_col) = self.read_until_top_level(&[';'])?;
        let expr = ExprParser::new(&expr_src, self.pos - expr_src.len(), expr_line, expr_col)
            .parse_expression()?;
        self.skip_code_ws_and_comments()?;
        self.expect_char(';', "`@let` 语句需要以 `;` 结束")?;
        Ok(Node {
            kind: NodeKind::Let { name, value: expr },
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    /// 插槽上下文表达式：`@slot body(item, index)`。
    fn parse_slot_args(&mut self) -> SlangResult<Vec<Expr>> {
        if self.peek_char() != Some('(') {
            return Ok(Vec::new());
        }
        self.bump();
        let args_start = self.pos;
        let args_line = self.line;
        let args_col = self.column;
        let args_src = self.read_balanced_paren_body()?;
        let mut exprs = Vec::new();
        for (start, end) in split_top_level(&args_src, ',') {
            let slice = &args_src[start..end];
            let raw = slice.trim();
            if raw.is_empty() {
                continue;
            }
            let leading = slice.len() - slice.trim_start().len();
            let offset = args_start + start + leading;
            exprs.push(ExprParser::new(raw, offset, args_line, args_col).parse_expression()?);
        }
        Ok(exprs)
    }

    /// 插槽参数列表：`@slot body(item, index)` / `@fill body(item)`。
    fn parse_slot_params(&mut self) -> SlangResult<Vec<String>> {
        let mut params = Vec::new();
        if self.peek_char() != Some('(') {
            return Ok(params);
        }
        self.bump();
        loop {
            self.skip_code_ws_and_comments()?;
            if self.peek_char() == Some(')') {
                self.bump();
                break;
            }
            params.push(self.parse_ident(false)?);
            self.skip_code_ws_and_comments()?;
            match self.peek_char() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                _ => {
                    return err(
                        "E2001",
                        "插槽参数之间需要 `,`，或在这里结束 `)`",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    )
                }
            }
        }
        Ok(params)
    }

    fn parse_block(&mut self) -> SlangResult<Vec<Node>> {
        self.skip_code_ws_and_comments()?;
        self.expect_char('{', "这里需要 `{` 开始一个块")?;
        // 只跳过块起始的缩进；块内文本/插值之间的空格必须保留。
        self.skip_code_ws_and_comments()?;
        self.depth += 1;
        if self.depth > 64 {
            self.depth -= 1;
            return err(
                "E6001",
                "块嵌套超过 64 层".to_string(),
                self.span_from(self.pos, self.pos, self.line, self.column),
            );
        }
        let mut nodes = Vec::new();
        loop {
            if self.is_eof() {
                self.depth -= 1;
                return err(
                    "E2004",
                    "块没有闭合（缺少 `}`）",
                    self.span_from(self.pos, self.pos, self.line, self.column),
                );
            }
            if self.peek_char() == Some('}') {
                self.bump();
                break;
            }
            match self.parse_item(true)? {
                Some(Item::Node(node)) => nodes.push(node),
                Some(Item::Boundary) => {
                    self.depth -= 1;
                    return err(
                        "E2001",
                        "`@def` / `@script` 只能出现在顶层，不能写在块里",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    );
                }
                Some(Item::Def(_)) => {
                    self.depth -= 1;
                    return err(
                        "E2001",
                        "`@def` 只能出现在顶层",
                        self.span_from(self.pos, self.pos, self.line, self.column),
                    );
                }
                None => break,
            }
        }
        self.depth -= 1;
        Ok(nodes)
    }

    fn parse_escape(&mut self) -> SlangResult<Node> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        self.bump(); // backslash
        let Some(ch) = self.peek_char() else {
            return Ok(Node {
                kind: NodeKind::Text("\\".to_string()),
                span: self.span_from(start, self.pos, start_line, start_col),
            });
        };
        let value = match ch {
            '{' | '}' | '@' | ':' | '\\' => {
                self.bump();
                ch.to_string()
            }
            _ => "\\".to_string(),
        };
        Ok(Node {
            kind: NodeKind::Text(value),
            span: self.span_from(start, self.pos, start_line, start_col),
        })
    }

    fn scan_text(&mut self, in_block: bool) -> String {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if in_block && ch == '}' {
                break;
            }
            if self.starts_with("{{") {
                break;
            }
            if self.starts_with("::") && self.peek_char_after(2).is_some_and(is_ident_start) {
                break;
            }
            if self.starts_with("@#") || self.starts_with("@*") {
                break;
            }
            if ch == '@' && self.directive_name_at().is_some() {
                break;
            }
            if ch == '\\'
                && self
                    .peek_char_after(1)
                    .is_some_and(|next| matches!(next, '{' | '}' | '@' | ':' | '\\'))
            {
                break;
            }
            self.bump();
        }
        self.src[start..self.pos].to_string()
    }

    fn read_until_interp_end(&mut self) -> SlangResult<(String, u32, u32)> {
        let start = self.pos;
        let line = self.line;
        let col = self.column;
        let mut index = self.pos;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while index < self.src.len() {
            let ch = self.src[index..].chars().next().unwrap_or(' ');
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                index += ch.len_utf8();
                continue;
            }
            match ch {
                '"' | '\'' | '`' => quote = Some(ch),
                '{' => depth += 1,
                '}' => {
                    if depth == 0 && self.src[index..].starts_with("}}") {
                        let expr = self.src[start..index].to_string();
                        self.advance_to(index + 2);
                        return Ok((expr, line, col));
                    }
                    depth = (depth - 1).max(0);
                }
                _ => {}
            }
            index += ch.len_utf8();
        }
        err(
            "E1002",
            "`{{` 没有闭合（缺少 `}}`）",
            self.span_from(start, index, line, col),
        )
    }

    fn read_until_block_open(&mut self) -> SlangResult<(String, u32, u32)> {
        let start = self.pos;
        let line = self.line;
        let col = self.column;
        let mut index = self.pos;
        let mut depth = 0i32;
        let mut seen_expr = false;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while index < self.src.len() {
            let ch = self.src[index..].chars().next().unwrap_or(' ');
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                if !ch.is_whitespace() {
                    seen_expr = true;
                }
                index += ch.len_utf8();
                continue;
            }
            match ch {
                '"' | '\'' | '`' => {
                    quote = Some(ch);
                    seen_expr = true;
                }
                '(' | '[' => {
                    depth += 1;
                    seen_expr = true;
                }
                ')' | ']' => {
                    depth = (depth - 1).max(0);
                }
                '{' => {
                    if depth == 0 && seen_expr {
                        // 区分 blocks 与对象字面量：`@if x == {a: 1} {` 里的第一个
                        // `{` 前面是运算符，应按对象字面量处理，而不是块开始。
                        let previous = self.src[..index]
                            .chars()
                            .rev()
                            .find(|ch| !ch.is_whitespace());
                        let looks_like_object = previous.is_none_or(|ch| {
                            matches!(
                                ch,
                                '=' | '+'
                                    | '-'
                                    | '*'
                                    | '/'
                                    | '%'
                                    | '!'
                                    | '&'
                                    | '|'
                                    | '?'
                                    | ':'
                                    | '('
                                    | '['
                                    | ','
                                    | '<'
                                    | '>'
                            )
                        });
                        if !looks_like_object {
                            let expr = self.src[start..index].to_string();
                            self.advance_to(index);
                            return Ok((expr, line, col));
                        }
                    }
                    depth += 1;
                    seen_expr = true;
                }
                '}' => {
                    depth = (depth - 1).max(0);
                }
                _ => {
                    if !ch.is_whitespace() {
                        seen_expr = true;
                    }
                }
            }
            index += ch.len_utf8();
        }
        err(
            "E2003",
            "这里需要 `{` 开始一个块",
            self.span_from(start, index, line, col),
        )
    }

    fn read_until_top_level(&mut self, stops: &[char]) -> SlangResult<(String, u32, u32)> {
        let start = self.pos;
        let line = self.line;
        let col = self.column;
        let mut index = self.pos;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while index < self.src.len() {
            let ch = self.src[index..].chars().next().unwrap_or(' ');
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                index += ch.len_utf8();
                continue;
            }
            if depth == 0 && stops.contains(&ch) {
                let expr = self.src[start..index].to_string();
                self.advance_to(index);
                return Ok((expr, line, col));
            }
            match ch {
                '"' | '\'' | '`' => quote = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = (depth - 1).max(0),
                _ => {}
            }
            index += ch.len_utf8();
        }
        let expr = self.src[start..index].to_string();
        self.advance_to(index);
        Ok((expr, line, col))
    }

    fn read_balanced_paren_body(&mut self) -> SlangResult<String> {
        let start = self.pos;
        let mut index = self.pos;
        let mut depth = 1i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while index < self.src.len() {
            let ch = self.src[index..].chars().next().unwrap_or(' ');
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                index += ch.len_utf8();
                continue;
            }
            match ch {
                '"' | '\'' | '`' => quote = Some(ch),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = self.src[start..index].to_string();
                        self.advance_to(index + 1);
                        return Ok(body);
                    }
                }
                _ => {}
            }
            index += ch.len_utf8();
        }
        err(
            "E2002",
            "`(` 没有闭合",
            self.span_from(start, index, self.line, self.column),
        )
    }

    fn parse_string_literal(&mut self) -> SlangResult<String> {
        self.skip_code_ws_and_comments()?;
        let remaining = &self.src[self.pos..];
        let mut lexer = Lexer::new(remaining);
        let token = lexer.next_token()?;
        let value = match token.kind {
            TokenKind::Str(value) => value,
            TokenKind::TripleStr(value) => value,
            other => {
                return err(
                    "E2001",
                    format!("这里需要字符串路径，得到 {}", other.describe()),
                    self.span_from(self.pos, self.pos + token.end, self.line, self.column),
                )
            }
        };
        self.advance_to(self.pos + token.end);
        Ok(value)
    }

    fn parse_ident(&mut self, allow_dash: bool) -> SlangResult<String> {
        let start = self.pos;
        let Some(first) = self.peek_char() else {
            return err(
                "E2001",
                "这里需要标识符",
                self.span_from(self.pos, self.pos, self.line, self.column),
            );
        };
        if !is_ident_start(first) {
            return err(
                "E2001",
                format!("这里需要标识符，得到 `{first}`"),
                self.span_from(
                    self.pos,
                    self.pos + first.len_utf8(),
                    self.line,
                    self.column,
                ),
            );
        }
        self.bump();
        while self
            .peek_char()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || (allow_dash && ch == '-'))
        {
            self.bump();
        }
        Ok(self.src[start..self.pos].to_string())
    }

    fn expect_char(&mut self, expected: char, message: &str) -> SlangResult<()> {
        match self.peek_char() {
            Some(ch) if ch == expected => {
                self.bump();
                Ok(())
            }
            Some(ch) => err(
                "E2001",
                format!("{message}；得到 `{ch}`"),
                self.span_from(self.pos, self.pos + ch.len_utf8(), self.line, self.column),
            ),
            None => err(
                "E2001",
                message,
                self.span_from(self.pos, self.pos, self.line, self.column),
            ),
        }
    }

    fn directive_name_at(&self) -> Option<&'static str> {
        let rest = self.src.get(self.pos + 1..)?;
        for name in [
            "if", "for", "let", "include", "use", "html", "md", "slot", "fill", "def", "script",
            "props",
        ] {
            if rest.starts_with(name)
                && rest[name.len()..]
                    .chars()
                    .next()
                    .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
            {
                return Some(name);
            }
        }
        None
    }

    fn skip_line_comment(&mut self) {
        while let Some(ch) = self.peek_char() {
            self.bump();
            if ch == '\n' {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self) -> SlangResult<()> {
        let start = self.pos;
        self.advance_by(2);
        if let Some(offset) = self.src[self.pos..].find("*@") {
            self.advance_to(self.pos + offset + 2);
            Ok(())
        } else {
            err(
                "E1002",
                "块注释没有闭合（缺少 `*@`）",
                self.span_from(start, self.pos, self.line, self.column),
            )
        }
    }

    fn skip_inline_ws(&mut self) {
        while self
            .peek_char()
            .is_some_and(|ch| ch == ' ' || ch == '\t' || ch == '\r')
        {
            self.bump();
        }
    }

    /// 只跳过语法空白/注释，不碰正文。解析完语法后如果发现“没有参数/块”，调用方要还原。
    fn skip_code_ws_and_comments(&mut self) -> SlangResult<()> {
        loop {
            while self.peek_char().is_some_and(|ch| ch.is_whitespace()) {
                self.bump();
            }
            if self.starts_with("@#") {
                self.skip_line_comment();
                continue;
            }
            if self.starts_with("@*") {
                self.skip_block_comment()?;
                continue;
            }
            break;
        }
        Ok(())
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_char_after(&self, bytes: usize) -> Option<char> {
        let index = self.pos.checked_add(bytes)?;
        self.src.get(index..)?.chars().next()
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.src[self.pos..].starts_with(needle)
    }

    /// 关键字必须后面跟边界，避免 `@elseif` / `inside` 被当成 `@else if` / `in`。
    fn keyword_at(&self, keyword: &str) -> bool {
        if !self.starts_with(keyword) {
            return false;
        }
        self.peek_char_after(keyword.len())
            .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_' && ch != '-')
    }

    fn bump(&mut self) {
        if let Some(ch) = self.peek_char() {
            self.pos += ch.len_utf8();
            if ch == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    fn advance_by(&mut self, bytes: usize) {
        let target = (self.pos + bytes).min(self.src.len());
        self.advance_to(target);
    }

    fn advance_to(&mut self, target: usize) {
        while self.pos < target {
            self.bump();
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn char_to_string_at(&self, index: usize) -> String {
        self.src
            .get(index..)
            .and_then(|rest| rest.chars().next())
            .map(|ch| ch.to_string())
            .unwrap_or_default()
    }

    fn span_from(&self, start: usize, end: usize, _line: u32, _column: u32) -> Span {
        // 行/列由调用点主动传；这里再用位置计算一次，避免调用点忘记更新。
        let (line, column) = line_column_at(self.src, start);
        let (_, _) = (line, column);
        Span::new(start, end, line, column)
    }
}

/// 在源码里计算某个字节偏移的行/列（1 基）。
pub fn line_column_at(src: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(src.len());
    let prefix = &src[..offset];
    let lines = prefix.matches('\n').count() as u32;
    let line = lines + 1;
    let column = if lines == 0 {
        prefix.chars().count() as u32 + 1
    } else {
        prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1
    };
    (line, column)
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

/// 在顶层按分隔符切分（忽略字符串与括号/花括号里的分隔符）。
fn split_top_level(src: &str, separator: char) -> Vec<(usize, usize)> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while index < src.len() {
        let ch = src[index..].chars().next().unwrap_or(' ');
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            index += ch.len_utf8();
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if ch == separator && depth == 0 => {
                parts.push((start, index));
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    if start <= src.len() {
        parts.push((start, src.len()));
    }
    parts
}

/// 把 `name: expr` 拆成 (Some(name), expr 起始偏移)；不是命名实参则 (None, 0)。
fn split_named_arg(raw: &str) -> (Option<String>, usize) {
    let mut index = 0usize;
    while index < raw.len() {
        let ch = raw[index..].chars().next().unwrap_or(' ');
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    let name_start = index;
    if !raw[name_start..].chars().next().is_some_and(is_ident_start) {
        return (None, 0);
    }
    index += raw[name_start..].chars().next().unwrap().len_utf8();
    while raw[index..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
    {
        index += raw[index..].chars().next().unwrap().len_utf8();
    }
    let name_end = index;
    while raw[index..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace())
    {
        index += raw[index..].chars().next().unwrap().len_utf8();
    }
    if raw[index..].starts_with(':') && !raw[index..].starts_with("::") {
        let name = raw[name_start..name_end].to_string();
        index += 1;
        while raw[index..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace())
        {
            index += raw[index..].chars().next().unwrap().len_utf8();
        }
        (Some(name), index)
    } else {
        (None, 0)
    }
}
