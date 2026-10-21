//! `@script`：Slang 2 的模块脚本与 props 声明。
//!
//! 设计边界（精简版）：
//!
//! - `@script { ... }` 是**模块级**的纯函数/常量，每文件一份、每次渲染执行一次；
//! - 函数是具名函数，没有闭包、没有函数值、没有 lambda；
//! - 组件不执行脚本，只通过 `@props { ... }` 声明 props；
//! - 交互交给浏览器里的 Alpine，服务端只负责“算数据 + 摆结构”。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::ast::{
    AssignOp, ConstValue, Expr, ExprKind, ScriptBlock, ScriptFunction, ScriptStmt, TemplatePart,
};
use super::component::{PropDecl, PropType, PropsDecl};
use super::error::{err, Diagnostic, SlangResult, Span};
use super::lexer::{Lexer, Token, TokenKind};
use super::parser::{line_column_at, ExprParser};

/// 脚本/表达式可用的内置函数。它们都是纯数据函数，不接受 lambda。
pub const BUILTIN_NAMES: &[&str] = &[
    // 字符串
    "len",
    "upper",
    "lower",
    "trim",
    "truncate",
    "split",
    "replace",
    "contains",
    "starts_with",
    "ends_with",
    "concat",
    "urlencode",
    // 集合 / 默认值 / 序列化
    "default",
    "coalesce",
    "join",
    "get",
    "keys",
    "values",
    "entries",
    "slice",
    "append",
    "reverse",
    "unique",
    "sort",
    "range",
    "json",
];

/// 根命名空间，脚本不能给它们赋值，也不能用它们当变量名。
pub const ROOT_NAMES: &[&str] = &["plugin", "page", "slot", "app", "route", "data"];

/// 语言关键字（作为标识符声明时报错）。
pub const RESERVED_WORDS: &[&str] = &[
    "let", "const", "function", "if", "else", "for", "in", "return", "break", "continue", "null",
    "true", "false",
];

pub fn is_reserved(name: &str) -> bool {
    ROOT_NAMES.contains(&name) || BUILTIN_NAMES.contains(&name) || RESERVED_WORDS.contains(&name)
}

/// 编译后的模块脚本。
#[derive(Debug, Clone, Default)]
pub struct CompiledScript {
    /// 顶层具名函数（调用时新建参数作用域，不捕获调用点）。
    pub functions: HashMap<String, Arc<ScriptFunction>>,
    /// 顶层 `let` / `const` / `if` / `for` 语句（函数声明已提升到 `functions`）。
    pub statements: Vec<ScriptStmt>,
    /// 模块级声明名（供诊断/测试使用）。
    pub declares: HashSet<String>,
}

impl CompiledScript {
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty() && self.functions.is_empty()
    }
}

pub fn compile_script(blocks: &[ScriptBlock]) -> SlangResult<CompiledScript> {
    let mut statements = Vec::new();
    for block in blocks {
        statements.extend(block.statements.iter().cloned());
    }

    let mut functions: HashMap<String, Arc<ScriptFunction>> = HashMap::new();
    let mut declares = HashSet::new();
    for statement in &statements {
        match statement {
            ScriptStmt::Function(function) => {
                if is_reserved(&function.name) {
                    return err(
                        "E7101",
                        format!("函数名 `{}` 是保留名", function.name),
                        function.span,
                    );
                }
                if functions.contains_key(&function.name) {
                    return err(
                        "E7101",
                        format!("函数 `{}` 重复定义", function.name),
                        function.span,
                    );
                }
                declares.insert(function.name.clone());
                functions.insert(function.name.clone(), Arc::new(function.clone()));
            }
            ScriptStmt::Let { name, span, .. } => {
                if is_reserved(name) {
                    return err("E7101", format!("变量名 `{name}` 是保留名"), *span);
                }
                if !declares.insert(name.clone()) {
                    return err("E7101", format!("`{name}` 在模块脚本里重复声明"), *span);
                }
            }
            _ => {}
        }
    }

    validate_statements(&statements, &functions)?;

    Ok(CompiledScript {
        functions,
        statements,
        declares,
    })
}

/// 把 `@props { ... }` 的对象字面量转成 props 契约。
pub fn parse_props_decl(expr: &Expr) -> SlangResult<PropsDecl> {
    let mut decl = PropsDecl::default();
    match &expr.kind {
        ExprKind::Object(fields) => {
            for (name, value) in fields {
                let prop = parse_prop(name, value)?;
                if decl.get(&prop.name).is_some() {
                    return err(
                        "E7204",
                        format!("props `{}` 重复声明", prop.name),
                        prop.span,
                    );
                }
                decl.props.push(prop);
            }
        }
        _ => {
            return err(
                "E7202",
                "`@props` 需要一个对象字面量，例如 `@props { label: String }`",
                expr.span,
            )
        }
    }
    Ok(decl)
}

fn parse_prop(name: &str, value: &Expr) -> SlangResult<PropDecl> {
    if is_reserved(name) || matches!(name, "props" | "slots") {
        return err(
            "E7204",
            format!("props 名 `{name}` 与根命名空间/系统绑定/内置函数/关键字冲突"),
            value.span,
        );
    }
    let mut ty = PropType::Any;
    let mut required: Option<bool> = None;
    let mut default: Option<ConstValue> = None;
    let mut enum_values: Option<Vec<String>> = None;

    match &value.kind {
        // `label: String` / `label: "string"` / `label: null`
        ExprKind::Ident(type_name) => ty = parse_type_name(type_name, value.span)?,
        ExprKind::Str(type_name) => ty = parse_type_name(type_name, value.span)?,
        ExprKind::Null => {}
        // `tone: { type: String, default: "neutral" }`
        ExprKind::Object(fields) => {
            for (key, field) in fields {
                match key.as_str() {
                    "type" => match &field.kind {
                        ExprKind::Ident(type_name) | ExprKind::Str(type_name) => {
                            if type_name == "enum" {
                                ty = PropType::Enum(Vec::new());
                            } else {
                                ty = parse_type_name(type_name, field.span)?;
                            }
                        }
                        _ => {
                            return err("E7202", "props 的 `type` 需要类型名或字符串", field.span)
                        }
                    },
                    "required" => match &field.kind {
                        ExprKind::Bool(value) => required = Some(*value),
                        _ => return err("E7202", "props 的 `required` 需要布尔字面量", field.span),
                    },
                    "default" => {
                        default = Some(ConstValue::from_expr(field).ok_or_else(|| {
                            Diagnostic::error(
                                "E7202",
                                "props 的 `default` 必须是字面量（数组/对象/标量）",
                                field.span,
                            )
                        })?);
                    }
                    "values" => {
                        enum_values = Some(string_list(field, "props 的 `values`")?);
                    }
                    other => {
                        return err(
                            "E7202",
                            format!("未知的 props 声明字段 `{other}`（可用：type / required / default / values）"),
                            field.span,
                        )
                    }
                }
            }
        }
        _ => {
            return err(
                "E7202",
                "props 声明需要类型名、字符串、`null` 或 `{ type, default, required }` 对象",
                value.span,
            )
        }
    }

    if let Some(values) = enum_values {
        if values.is_empty() {
            return err("E7202", "`enum` 类型至少需要一个 `values` 取值", value.span);
        }
        ty = PropType::Enum(values);
    }

    let has_default = default.is_some();
    if required == Some(true) && has_default {
        return err(
            "E7202",
            "props 不能同时声明 `required: true` 和 `default`",
            value.span,
        );
    }
    Ok(PropDecl {
        name: name.to_string(),
        ty,
        required: required.unwrap_or(!has_default),
        default,
        span: value.span,
    })
}

fn parse_type_name(name: &str, span: Span) -> SlangResult<PropType> {
    PropType::from_name(name).ok_or_else(|| {
        Diagnostic::error("E7202", format!("未知属性类型 `{name}`"), span).with_hint(
            "可用：String / Int / Float / Bool / List / Object / Any / Url / Icon / Class / enum",
        )
    })
}

fn string_list(expr: &Expr, what: &str) -> SlangResult<Vec<String>> {
    let ExprKind::List(items) = &expr.kind else {
        return err("E7202", format!("{what} 需要字符串数组"), expr.span);
    };
    items
        .iter()
        .map(|item| match &item.kind {
            ExprKind::Str(value) => Ok(value.clone()),
            _ => err("E7202", format!("{what} 只能包含字符串"), item.span),
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// 语句解析
// ─────────────────────────────────────────────────────────────────────────────

pub struct ScriptParser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    index: usize,
    base_offset: usize,
    base_line: u32,
    base_column: u32,
}

impl<'a> ScriptParser<'a> {
    pub fn new(
        src: &'a str,
        base_offset: usize,
        base_line: u32,
        base_column: u32,
    ) -> SlangResult<Self> {
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token()?;
            if matches!(token.kind, TokenKind::Eof) {
                break;
            }
            tokens.push(token);
        }
        Ok(Self {
            src,
            tokens,
            index: 0,
            base_offset,
            base_line: base_line.max(1),
            base_column: base_column.max(1),
        })
    }

    /// 解析整个脚本体（顶层语句，允许 `function`）。
    pub fn parse_statements(mut self) -> SlangResult<Vec<ScriptStmt>> {
        self.parse_statements_until(None, false)
    }

    fn parse_statements_until(
        &mut self,
        end: Option<TokenKind>,
        in_function: bool,
    ) -> SlangResult<Vec<ScriptStmt>> {
        let mut statements = Vec::new();
        loop {
            let Some(token) = self.peek() else {
                if end.is_some() {
                    return err(
                        "E7101",
                        "脚本块没有闭合（缺少 `}`）",
                        self.span_at(self.src.len(), self.src.len()),
                    );
                }
                break;
            };
            if let Some(end_kind) = &end {
                if token.kind == *end_kind {
                    break;
                }
            }
            statements.push(self.parse_stmt(in_function)?);
        }
        Ok(statements)
    }

    fn parse_stmt(&mut self, in_function: bool) -> SlangResult<ScriptStmt> {
        let Some(token) = self.peek().cloned() else {
            return err(
                "E7101",
                "脚本意外结束",
                self.span_at(self.src.len(), self.src.len()),
            );
        };
        let TokenKind::Ident(keyword) = &token.kind else {
            return err(
                "E7101",
                "脚本语句需要以 `let` / `const` / `if` / `for` / `return` / `function` 或赋值开头",
                self.span_for(&token),
            );
        };
        match keyword.as_str() {
            "let" | "const" => self.parse_binding(),
            "if" => self.parse_if(in_function),
            "for" => self.parse_for(in_function),
            "return" => self.parse_return(),
            "break" => self.parse_break_continue(true),
            "continue" => self.parse_break_continue(false),
            "function" => self.parse_function(),
            "else" => err("E7101", "`else` 没有对应的 `if`", self.span_for(&token)),
            "while" => err(
                "E7101",
                "Slang 2 没有 `while`；请用 `for`（循环有预算上限）",
                self.span_for(&token),
            ),
            _ => self.parse_assignment(),
        }
    }

    fn parse_binding(&mut self) -> SlangResult<ScriptStmt> {
        let keyword = self.next().unwrap();
        let mutable = matches!(&keyword.kind, TokenKind::Ident(name) if name == "let");
        let name = self.expect_ident("`let`/`const` 后面需要变量名")?;
        self.expect_simple(TokenKind::Assign, "变量声明需要 `=`")?;
        let expr_start = self.current_start();
        let semi = self.find_top_level(&[TokenKind::Semicolon])?;
        let expr_end = self.tokens[semi].start;
        let value = self.parse_expr_range(expr_start, expr_end)?;
        let span = self.span_at(keyword.start, self.tokens[semi].end);
        self.index = semi + 1;
        Ok(ScriptStmt::Let {
            name,
            mutable,
            value,
            span,
        })
    }

    fn parse_assignment(&mut self) -> SlangResult<ScriptStmt> {
        let start = self.current_start();
        let first_token = self.index;
        let semi = self.find_top_level(&[TokenKind::Semicolon])?;
        let statement_end = self.tokens[semi].start;
        let span = self.span_at(start, self.tokens[semi].end);

        let Some((assign_index, op)) = self.find_assignment(first_token, semi) else {
            return err(
                "E7101",
                "脚本里不允许表达式语句；只支持 `let` / `const` / 赋值 / `if` / `for` / `return` / `function`",
                span,
            );
        };
        let target_end = self.tokens[assign_index].start;
        let value_start = self.tokens[assign_index + op.token_len()].start;
        let target = self.parse_expr_range(start, target_end)?;
        validate_lvalue(&target)?;
        let value = self.parse_expr_range(value_start, statement_end)?;
        self.index = semi + 1;
        Ok(ScriptStmt::Assign {
            target,
            op: op.op(),
            value,
            span,
        })
    }

    /// 在 `[start, end)` 里找深度 0 的赋值运算符。
    fn find_assignment(&self, start: usize, end: usize) -> Option<(usize, CompoundOp)> {
        let mut depth = 0i32;
        let mut index = start;
        while index < end {
            let kind = &self.tokens[index].kind;
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = (depth - 1).max(0)
                }
                TokenKind::Assign if depth == 0 => {
                    return Some((index, CompoundOp::Set));
                }
                TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                    if depth == 0 =>
                {
                    if matches!(
                        self.tokens.get(index + 1).map(|token| &token.kind),
                        Some(TokenKind::Assign)
                    ) && index > start
                    {
                        let op = match kind {
                            TokenKind::Plus => CompoundOp::Add,
                            TokenKind::Minus => CompoundOp::Sub,
                            TokenKind::Star => CompoundOp::Mul,
                            TokenKind::Slash => CompoundOp::Div,
                            _ => CompoundOp::Mod,
                        };
                        return Some((index, op));
                    }
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn parse_if(&mut self, in_function: bool) -> SlangResult<ScriptStmt> {
        let start = self.next().unwrap(); // if
        let brace = self.find_block_open()?;
        let cond_start = self.current_start();
        let cond_end = self.tokens[brace].start;
        let condition = self.parse_expr_range(cond_start, cond_end)?;
        self.index = brace;
        let body = self.parse_block(in_function)?;
        let arms = vec![(condition, body)];
        let mut otherwise = Vec::new();
        if self.eat_ident("else") {
            if self.check_ident("if") {
                let nested = self.parse_if(in_function)?;
                otherwise.push(nested);
            } else {
                otherwise = self.parse_block(in_function)?;
            }
        }
        Ok(ScriptStmt::If {
            arms,
            otherwise,
            span: self.span_at(start.start, self.previous_end()),
        })
    }

    fn parse_for(&mut self, in_function: bool) -> SlangResult<ScriptStmt> {
        let start = self.next().unwrap(); // for
        let (item, index) = if self.eat_simple(TokenKind::LParen) {
            let item = self.expect_ident("`for` 需要迭代变量")?;
            let index = if self.eat_simple(TokenKind::Comma) {
                Some(self.expect_ident("`for (item, index)` 需要索引名")?)
            } else {
                None
            };
            self.expect_simple(TokenKind::RParen, "`for (...)` 缺少 `)`")?;
            (item, index)
        } else {
            (self.expect_ident("`for` 需要迭代变量")?, None)
        };
        if !self.eat_ident("in") {
            return err(
                "E7101",
                "`for` 变量后面需要 `in`",
                self.span_at(self.current_start(), self.current_start()),
            );
        }
        let brace = self.find_block_open()?;
        let iter_start = self.current_start();
        let iter_end = self.tokens[brace].start;
        let iter = self.parse_expr_range(iter_start, iter_end)?;
        self.index = brace;
        let body = self.parse_block(in_function)?;
        Ok(ScriptStmt::For {
            item,
            index,
            iter,
            body,
            span: self.span_at(start.start, self.previous_end()),
        })
    }

    fn parse_return(&mut self) -> SlangResult<ScriptStmt> {
        let start = self.next().unwrap(); // return
        let value = if self.check_simple(TokenKind::Semicolon) {
            None
        } else {
            let expr_start = self.current_start();
            let semi = self.find_top_level(&[TokenKind::Semicolon])?;
            let expr_end = self.tokens[semi].start;
            let expr = self.parse_expr_range(expr_start, expr_end)?;
            self.index = semi;
            Some(expr)
        };
        self.expect_simple(TokenKind::Semicolon, "`return` 需要以 `;` 结束")?;
        Ok(ScriptStmt::Return {
            value,
            span: self.span_at(start.start, self.previous_end()),
        })
    }

    fn parse_break_continue(&mut self, is_break: bool) -> SlangResult<ScriptStmt> {
        let start = self.next().unwrap();
        self.expect_simple(
            TokenKind::Semicolon,
            if is_break {
                "`break` 需要以 `;` 结束"
            } else {
                "`continue` 需要以 `;` 结束"
            },
        )?;
        let span = self.span_at(start.start, self.previous_end());
        Ok(if is_break {
            ScriptStmt::Break(span)
        } else {
            ScriptStmt::Continue(span)
        })
    }

    fn parse_function(&mut self) -> SlangResult<ScriptStmt> {
        let start = self.next().unwrap(); // function
        let name = self.expect_ident("`function` 后面需要函数名")?;
        self.expect_simple(TokenKind::LParen, "函数名后面需要 `(`")?;
        let mut params = Vec::new();
        if !self.check_simple(TokenKind::RParen) {
            loop {
                params.push(self.expect_ident("函数参数需要标识符")?);
                if self.eat_simple(TokenKind::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect_simple(TokenKind::RParen, "函数参数缺少 `)`")?;
        let body = self.parse_block(true)?;
        Ok(ScriptStmt::Function(ScriptFunction {
            name,
            params,
            body,
            span: self.span_at(start.start, self.previous_end()),
        }))
    }

    fn parse_block(&mut self, in_function: bool) -> SlangResult<Vec<ScriptStmt>> {
        self.expect_simple(TokenKind::LBrace, "这里需要 `{`")?;
        let body = self.parse_statements_until(Some(TokenKind::RBrace), in_function)?;
        self.expect_simple(TokenKind::RBrace, "脚本块缺少 `}`")?;
        Ok(body)
    }

    fn parse_expr_range(&self, start: usize, end: usize) -> SlangResult<Expr> {
        if start >= end {
            return err("E7101", "这里需要一个表达式", self.span_at(start, start));
        }
        let (line, column) = self.line_col(start);
        ExprParser::new(
            &self.src[start..end],
            self.base_offset + start,
            line,
            column,
        )
        .parse_expression()
    }

    fn line_col(&self, offset: usize) -> (u32, u32) {
        let (local_line, local_column) = line_column_at(self.src, offset);
        let line = self.base_line + local_line.saturating_sub(1);
        let column = if local_line == 1 {
            self.base_column + local_column.saturating_sub(1)
        } else {
            local_column
        };
        (line, column)
    }

    fn span_at(&self, start: usize, end: usize) -> Span {
        let (line, column) = self.line_col(start);
        Span::new(
            self.base_offset + start,
            self.base_offset + end,
            line,
            column,
        )
    }

    fn span_for(&self, token: &Token) -> Span {
        self.span_at(token.start, token.end)
    }

    fn current_start(&self) -> usize {
        self.peek()
            .map(|token| token.start)
            .unwrap_or(self.src.len())
    }

    fn previous_end(&self) -> usize {
        if self.index == 0 {
            0
        } else {
            self.tokens[self.index - 1].end
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        self.index += 1;
        token
    }

    fn check_simple(&self, kind: TokenKind) -> bool {
        self.peek().is_some_and(|token| token.kind == kind)
    }

    fn eat_simple(&mut self, kind: TokenKind) -> bool {
        if self.check_simple(kind) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect_simple(&mut self, kind: TokenKind, message: &str) -> SlangResult<Token> {
        let Some(token) = self.peek().cloned() else {
            return err(
                "E7101",
                message,
                self.span_at(self.src.len(), self.src.len()),
            );
        };
        if token.kind == kind {
            self.index += 1;
            Ok(token)
        } else {
            err(
                "E7101",
                format!("{message}；得到 {}", token.kind.describe()),
                self.span_for(&token),
            )
        }
    }

    fn check_ident(&self, name: &str) -> bool {
        matches!(self.peek().map(|token| &token.kind), Some(TokenKind::Ident(value)) if value == name)
    }

    fn eat_ident(&mut self, name: &str) -> bool {
        if self.check_ident(name) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self, message: &str) -> SlangResult<String> {
        let Some(token) = self.peek().cloned() else {
            return err(
                "E7101",
                message,
                self.span_at(self.src.len(), self.src.len()),
            );
        };
        let span = self.span_for(&token);
        match token.kind {
            TokenKind::Ident(name) => {
                self.index += 1;
                Ok(name)
            }
            other => err(
                "E7101",
                format!("{message}；得到 {}", other.describe()),
                span,
            ),
        }
    }

    fn find_top_level(&self, kinds: &[TokenKind]) -> SlangResult<usize> {
        self.find_top_level_from(self.index, kinds)
    }

    fn find_top_level_from(&self, start: usize, kinds: &[TokenKind]) -> SlangResult<usize> {
        let mut depth = 0i32;
        for index in start..self.tokens.len() {
            let kind = &self.tokens[index].kind;
            if depth == 0 && kinds.iter().any(|expected| expected == kind) {
                return Ok(index);
            }
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = (depth - 1).max(0)
                }
                _ => {}
            }
        }
        err(
            "E7101",
            "脚本语句没有结束（缺少 `;` 或 `}`）",
            self.span_at(self.src.len(), self.src.len()),
        )
    }

    /// 找 `if`/`for` 的块起始 `{`；条件里的对象字面量不算。
    fn find_block_open(&self) -> SlangResult<usize> {
        let mut depth = 0i32;
        for index in self.index..self.tokens.len() {
            let kind = &self.tokens[index].kind;
            match kind {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                TokenKind::LBrace => {
                    if depth == 0 {
                        let previous = index
                            .checked_sub(1)
                            .and_then(|previous| self.tokens.get(previous))
                            .map(|token| &token.kind);
                        let looks_like_object = previous.is_none_or(is_expr_prefix);
                        if !looks_like_object {
                            return Ok(index);
                        }
                    }
                    depth += 1;
                }
                TokenKind::RBrace => depth = (depth - 1).max(0),
                _ => {}
            }
        }
        err(
            "E7101",
            "`if`/`for` 缺少块起始 `{`",
            self.span_at(self.src.len(), self.src.len()),
        )
    }
}

fn is_expr_prefix(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Assign
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Bang
            | TokenKind::Question
            | TokenKind::QuestionQuestion
            | TokenKind::Pipe
            | TokenKind::Colon
            | TokenKind::Comma
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
    )
}

#[derive(Debug, Clone, Copy)]
enum CompoundOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl CompoundOp {
    fn token_len(self) -> usize {
        match self {
            CompoundOp::Set => 1,
            _ => 2,
        }
    }

    fn op(self) -> AssignOp {
        match self {
            CompoundOp::Set => AssignOp::Set,
            CompoundOp::Add => AssignOp::Add,
            CompoundOp::Sub => AssignOp::Sub,
            CompoundOp::Mul => AssignOp::Mul,
            CompoundOp::Div => AssignOp::Div,
            CompoundOp::Mod => AssignOp::Mod,
        }
    }
}

fn validate_lvalue(expr: &Expr) -> SlangResult<()> {
    match &expr.kind {
        ExprKind::Ident(_) | ExprKind::Field(_, _) | ExprKind::Index(_, _) => Ok(()),
        _ => err(
            "E7105",
            "赋值目标必须是变量、字段或下标（`name` / `a.b` / `a[i]`）",
            expr.span,
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 校验
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct ScopeStack {
    scopes: Vec<HashMap<String, bool>>, // name -> mutable
}

impl ScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, mutable: bool) -> bool {
        let scope = self.scopes.last_mut().expect("scope");
        if scope.contains_key(name) {
            return false;
        }
        scope.insert(name.to_string(), mutable);
        true
    }

    fn is_mutable(&self, name: &str) -> Option<bool> {
        for scope in self.scopes.iter().rev() {
            if let Some(mutable) = scope.get(name) {
                return Some(*mutable);
            }
        }
        None
    }
}

fn validate_statements(
    statements: &[ScriptStmt],
    functions: &HashMap<String, Arc<ScriptFunction>>,
) -> SlangResult<()> {
    let mut scopes = ScopeStack::new();
    validate_block(statements, functions, &mut scopes, false, false)
}

fn validate_block(
    statements: &[ScriptStmt],
    functions: &HashMap<String, Arc<ScriptFunction>>,
    scopes: &mut ScopeStack,
    in_function: bool,
    in_loop: bool,
) -> SlangResult<()> {
    for statement in statements {
        match statement {
            ScriptStmt::Let {
                name,
                mutable,
                value,
                span,
            } => {
                if is_reserved(name) {
                    return err("E7101", format!("变量名 `{name}` 是保留名"), *span);
                }
                validate_expr(value, functions, scopes)?;
                if !scopes.declare(name, *mutable) {
                    return err("E7101", format!("`{name}` 在同一作用域重复声明"), *span);
                }
            }
            ScriptStmt::Assign {
                target,
                value,
                span,
                ..
            } => {
                validate_lvalue(target)?;
                let root = root_name(target);
                if ROOT_NAMES.contains(&root.as_str()) {
                    return err("E7105", format!("不能给根命名空间 `{root}` 赋值"), *span);
                }
                if scopes.is_mutable(&root) == Some(false) {
                    return err("E7105", format!("不能给 `const {root}` 重新赋值"), *span);
                }
                validate_expr(target, functions, scopes)?;
                validate_expr(value, functions, scopes)?;
            }
            ScriptStmt::If {
                arms, otherwise, ..
            } => {
                for (condition, body) in arms {
                    validate_expr(condition, functions, scopes)?;
                    scopes.push();
                    let result = validate_block(body, functions, scopes, in_function, in_loop);
                    scopes.pop();
                    result?;
                }
                scopes.push();
                let result = validate_block(otherwise, functions, scopes, in_function, in_loop);
                scopes.pop();
                result?;
            }
            ScriptStmt::For {
                item,
                index,
                iter,
                body,
                span,
            } => {
                if is_reserved(item) {
                    return err("E7101", format!("循环变量 `{item}` 是保留名"), *span);
                }
                if let Some(index) = index {
                    if is_reserved(index) {
                        return err("E7101", format!("索引变量 `{index}` 是保留名"), *span);
                    }
                }
                validate_expr(iter, functions, scopes)?;
                scopes.push();
                scopes.declare(item, false);
                if let Some(index) = index {
                    scopes.declare(index, false);
                }
                let result = validate_block(body, functions, scopes, in_function, true);
                scopes.pop();
                result?;
            }
            ScriptStmt::Return { value, span } => {
                if !in_function {
                    return err("E7101", "`return` 只能出现在函数里", *span);
                }
                if let Some(value) = value {
                    validate_expr(value, functions, scopes)?;
                }
            }
            ScriptStmt::Break(span) | ScriptStmt::Continue(span) => {
                if !in_loop {
                    return err("E7101", "`break`/`continue` 只能出现在循环里", *span);
                }
            }
            ScriptStmt::Function(function) => {
                if in_function {
                    return err(
                        "E7101",
                        format!(
                            "函数 `{}` 必须声明在模块脚本顶层（没有闭包/嵌套函数）",
                            function.name
                        ),
                        function.span,
                    );
                }
                validate_params(&function.params, function.span)?;
                scopes.push();
                for param in &function.params {
                    scopes.declare(param, false);
                }
                let result = validate_block(&function.body, functions, scopes, true, false);
                scopes.pop();
                result?;
            }
        }
    }
    Ok(())
}

fn validate_params(params: &[String], span: Span) -> SlangResult<()> {
    let mut seen = HashSet::new();
    for param in params {
        if is_reserved(param) {
            return err("E7101", format!("参数名 `{param}` 是保留名"), span);
        }
        if !seen.insert(param) {
            return err("E7101", format!("参数 `{param}` 重复"), span);
        }
    }
    Ok(())
}

fn validate_expr(
    expr: &Expr,
    functions: &HashMap<String, Arc<ScriptFunction>>,
    scopes: &ScopeStack,
) -> SlangResult<()> {
    match &expr.kind {
        ExprKind::Call(name, args) => {
            if !BUILTIN_NAMES.contains(&name.as_str()) && !functions.contains_key(name) {
                return err("E7102", format!("未知函数 `{name}`"), expr.span).map_err(
                    |diagnostic: Diagnostic| {
                        diagnostic.with_hint(
                            "可用内置：len / upper / lower / trim / join / split / json / keys / values / entries / slice / sort / range …；自定义函数请在 `@script` 顶层声明",
                        )
                    },
                );
            }
            for arg in args {
                validate_expr(arg, functions, scopes)?;
            }
        }
        ExprKind::Filter(name, value, args) => {
            if !BUILTIN_NAMES.contains(&name.as_str()) {
                return err("E7102", format!("未知过滤器 `{name}`"), expr.span);
            }
            validate_expr(value, functions, scopes)?;
            for arg in args {
                validate_expr(arg, functions, scopes)?;
            }
        }
        ExprKind::Field(base, _) => validate_expr(base, functions, scopes)?,
        ExprKind::Index(base, index) => {
            validate_expr(base, functions, scopes)?;
            validate_expr(index, functions, scopes)?;
        }
        ExprKind::Unary(_, value) => validate_expr(value, functions, scopes)?,
        ExprKind::Binary(_, left, right) => {
            validate_expr(left, functions, scopes)?;
            validate_expr(right, functions, scopes)?;
        }
        ExprKind::Ternary(condition, then_branch, else_branch) => {
            validate_expr(condition, functions, scopes)?;
            validate_expr(then_branch, functions, scopes)?;
            validate_expr(else_branch, functions, scopes)?;
        }
        ExprKind::List(items) => {
            for item in items {
                validate_expr(item, functions, scopes)?;
            }
        }
        ExprKind::Object(fields) => {
            for (_, value) in fields {
                validate_expr(value, functions, scopes)?;
            }
        }
        ExprKind::Template(parts) => {
            for part in parts {
                if let TemplatePart::Expr(expr) = part {
                    validate_expr(expr, functions, scopes)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn root_name(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(name) => name.clone(),
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => root_name(base),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<ScriptStmt> {
        ScriptParser::new(src, 0, 1, 1)
            .expect("parser")
            .parse_statements()
            .expect("statements")
    }

    #[test]
    fn parses_functions_and_control_flow() {
        let statements = parse(
            r#"
            const tone = "brand";
            function badge(count) {
              if (count > 0) { return { text: `{{ count }} 条`, tone: tone }; }
              return { text: "空", tone: "neutral" };
            }
            for (item, index) in data.items {
              if item.done { continue; }
              let x = item.title;
            }
            "#,
        );
        assert!(matches!(statements[0], ScriptStmt::Let { .. }));
        assert!(matches!(statements[1], ScriptStmt::Function(_)));
        assert!(matches!(statements[2], ScriptStmt::For { .. }));
    }

    #[test]
    fn parses_compound_assignment() {
        let statements = parse("let n = 0; n += 2;");
        assert!(matches!(
            statements[1],
            ScriptStmt::Assign {
                op: AssignOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn rejects_while_unknown_function_and_expression_statements() {
        assert!(ScriptParser::new("while true { }", 0, 1, 1)
            .unwrap()
            .parse_statements()
            .is_err());
        assert!(ScriptParser::new("f();", 0, 1, 1)
            .unwrap()
            .parse_statements()
            .is_err());

        let module = compile_script(&[ScriptBlock {
            statements: parse("function f() { return nope(); }"),
            span: Span::default(),
        }]);
        assert!(module.is_err());
    }

    #[test]
    fn rejects_nested_functions() {
        let module = compile_script(&[ScriptBlock {
            statements: parse(
                "function outer() { function inner() { return 1; } return inner(); }",
            ),
            span: Span::default(),
        }]);
        assert!(module.is_err());
    }

    #[test]
    fn parses_props_contract() {
        let expr = ExprParser::new(
            "{ label: String, tone: { type: String, default: \"neutral\" }, items: { type: \"list\", default: [] } }",
            0,
            1,
            1,
        )
        .parse_expression()
        .expect("expression");
        let decl = parse_props_decl(&expr).expect("props");
        assert_eq!(decl.props.len(), 3);
        assert!(decl.get("label").is_some_and(|prop| prop.required));
        assert!(!decl.get("tone").is_some_and(|prop| prop.required));
    }
}
