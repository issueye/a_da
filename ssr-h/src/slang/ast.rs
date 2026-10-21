//! Slang 2 的语法树。
//!
//! 设计取舍：
//! - **组件是模板 + props 契约**：`@def Name { @props { ... } ... }`，没有 setup；
//! - **脚本是模块级纯函数**：`@script` 只声明函数/常量，不起闭包、没有函数值；
//! - **slot 是 thunk**：`@slot name(item)` 与 `@fill name(item)` 传递插槽上下文；
//! - **表达式只算数据**：没有 lambda / 方法调用，交互交给浏览器里的 Alpine。
//!
//! AST 只放结构；props 默认值用 [`ConstValue`]，不放运行期值。

use std::collections::BTreeMap;

pub use super::error::Span;

/// 一个 `.slang` 文件解析后的顶层结构。
#[derive(Debug, Clone)]
pub struct Program {
    /// `@slang N`；缺省为 2。
    pub version: u32,
    /// 顶层内容节点（页面/插槽/组件模板的默认组件体）。
    pub nodes: Vec<Node>,
    /// 顶层 `@def` 定义。
    pub defs: Vec<Definition>,
    /// 顶层 `@script { ... }` 模块脚本（按出现顺序拼接）。
    pub module_scripts: Vec<ScriptBlock>,
}

/// 一个 `@script { ... }` 模块脚本块。
#[derive(Debug, Clone)]
pub struct ScriptBlock {
    pub statements: Vec<ScriptStmt>,
    pub span: Span,
}

/// 脚本语句。
#[derive(Debug, Clone)]
pub enum ScriptStmt {
    Let {
        name: String,
        mutable: bool,
        value: Expr,
        span: Span,
    },
    /// 赋值 / 复合赋值：`name = ...`、`obj.field += ...`、`list[i] = ...`。
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
        span: Span,
    },
    If {
        arms: Vec<(Expr, Vec<ScriptStmt>)>,
        otherwise: Vec<ScriptStmt>,
        span: Span,
    },
    For {
        item: String,
        index: Option<String>,
        iter: Expr,
        body: Vec<ScriptStmt>,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    /// 命名函数声明（只在模块脚本顶层；没有闭包/函数值）。
    Function(ScriptFunction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// 脚本函数声明。
#[derive(Debug, Clone)]
pub struct ScriptFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<ScriptStmt>,
    pub span: Span,
}

/// 一个内容/控制节点。
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    /// 普通文本；渲染时默认按当前输出模式转义/原样输出。
    Text(String),
    /// `{{ expr }}`；永远转义。
    Interp(Expr),
    /// `::Name(...) { ... }`。
    Component(ComponentCall),
    /// `@if ... { ... } @else if ... { ... } @else { ... }`。
    If {
        arms: Vec<(Expr, Vec<Node>)>,
        otherwise: Vec<Node>,
    },
    /// `@for item in expr { ... } @else { ... }`。
    For {
        item: String,
        index: Option<String>,
        iter: Expr,
        body: Vec<Node>,
        otherwise: Vec<Node>,
    },
    /// `@let name = expr;`。
    Let { name: String, value: Expr },
    /// `@include "path.slang"`；编译期展开，保留节点用于诊断。
    Include { path: String, body: Vec<Node> },
    /// `@use "path.slang"`；编译期只导入定义。
    Use { path: String },
    /// `@html { ... }` 或 `@html(expr)`。
    Html {
        body: Option<Vec<Node>>,
        expr: Option<Expr>,
    },
    /// `@md(expr)`。
    Md { expr: Expr },
    /// `@slot name(item, index) [required] { fallback }`。
    Slot {
        name: String,
        /// 传给调用点 `@fill` 的上下文值（子组件里求值）。
        params: Vec<Expr>,
        required: bool,
        fallback: Vec<Node>,
    },
    /// `@fill name(item, index) { ... }`。
    Fill {
        name: String,
        /// 调用点声明的接收变量名。
        params: Vec<String>,
        body: Vec<Node>,
    },
}

/// 组件调用。
#[derive(Debug, Clone)]
pub struct ComponentCall {
    pub name: String,
    pub args: Vec<Arg>,
    pub children: Vec<Node>,
    pub span: Span,
}

/// 一个实参；`name == None` 表示位置参数。
#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
    pub span: Span,
}

/// 顶层组件定义。
#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    /// `@props { ... }` 的对象字面量 AST；编译期转成 [`super::component::PropsDecl`]。
    pub props: Option<Expr>,
    pub body: Vec<Node>,
    pub span: Span,
}

/// 表达式。
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// 反引号模板字符串。
    Template(Vec<TemplatePart>),
    /// 变量/根命名空间。
    Ident(String),
    /// `expr.field`。
    Field(Box<Expr>, String),
    /// `expr[index]`。
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// 具名调用：`len(x)` / `badge(n)`。
    Call(String, Vec<Expr>),
    /// `value | filter` / `value | filter(args)`。
    Filter(String, Box<Expr>, Vec<Expr>),
    List(Vec<Expr>),
    Object(Vec<(String, Expr)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Coalesce,
}

/// 模板字符串的片段。
#[derive(Debug, Clone)]
pub enum TemplatePart {
    Text(String),
    Expr(Expr),
}

/// 编译期可安全保存的字面量值。
///
/// props 默认值只允许纯数据（字符串/数字/布尔/数组/对象），不能引用作用域。
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ConstValue>),
    Map(BTreeMap<String, ConstValue>),
}

impl ConstValue {
    /// 把纯字面量表达式转成编译期常量；不是字面量则返回 `None`。
    pub fn from_expr(expr: &Expr) -> Option<ConstValue> {
        match &expr.kind {
            ExprKind::Null => Some(ConstValue::Null),
            ExprKind::Bool(value) => Some(ConstValue::Bool(*value)),
            ExprKind::Int(value) => Some(ConstValue::Int(*value)),
            ExprKind::Float(value) => Some(ConstValue::Float(*value)),
            ExprKind::Str(value) => Some(ConstValue::Str(value.clone())),
            ExprKind::List(items) => items
                .iter()
                .map(ConstValue::from_expr)
                .collect::<Option<Vec<_>>>()
                .map(ConstValue::List),
            ExprKind::Object(fields) => fields
                .iter()
                .map(|(key, value)| ConstValue::from_expr(value).map(|value| (key.clone(), value)))
                .collect::<Option<BTreeMap<_, _>>>()
                .map(ConstValue::Map),
            _ => None,
        }
    }

    pub fn to_value(&self) -> super::value::Value {
        use super::value::Value;
        match self {
            ConstValue::Null => Value::Null,
            ConstValue::Bool(value) => Value::Bool(*value),
            ConstValue::Int(value) => Value::Int(*value),
            ConstValue::Float(value) => Value::Float(*value),
            ConstValue::Str(value) => Value::Str(value.clone()),
            ConstValue::List(items) => {
                Value::List(items.iter().map(ConstValue::to_value).collect())
            }
            ConstValue::Map(map) => Value::Map(
                map.iter()
                    .map(|(key, value)| (key.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }
}
