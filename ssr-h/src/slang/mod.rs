//! Slang 2（俚语）：ssr-h 的运行时组件语言。
//!
//! 设计见 `docs/SLANG-V2.md`（脚本见 `docs/SLANG-SCRIPT.md`）。本模块刻意与
//! `ui` / `plugins` 解耦：解析器只产生 AST；组件注册表与原生适配器在后续模块里接入。
//!
//! 只支持 `slang/2`：`@script` 模块纯函数、`@props` 契约、`@slot` 作用域插槽；
//! 交互交给浏览器里的 Alpine，服务端不做 setup/闭包/ref/computed/事件。

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod ast;
pub mod component;
pub mod error;
pub mod eval;
pub mod html;
pub mod lexer;
pub mod native;
pub mod parser;
pub mod runtime;
pub mod script;
pub mod value;

pub use component::{Args, ComponentDef, ComponentRegistry, ComponentSpec, RenderedSlots};
pub use error::{Diagnostic, Severity, SlangResult, Span};
pub use eval::{
    default_engine, AppMeta, Budgets, CompiledTemplate, Engine, Loader, PageMeta, PluginMeta,
    RenderContext, RouteMeta, SlotMeta, Trust,
};
pub use parser::{ExprParser, TextParser};
pub use value::Value;
