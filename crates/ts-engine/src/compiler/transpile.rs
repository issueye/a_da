use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use std::path::Path;

use super::diagnostics::render_oxc_diagnostics;
pub fn oxc_strip_types(source: &str, filename: Option<&str>) -> Result<String, String> {
    let allocator = Allocator::default();
    let source_type = if let Some(name) = filename {
        SourceType::from_path(Path::new(name)).unwrap_or_else(|_| SourceType::ts())
    } else {
        SourceType::ts()
    };

    let parse_ret = Parser::new(&allocator, source, source_type).parse();
    if !parse_ret.diagnostics.is_empty() {
        return Err(format!(
            "TypeScript 解析错误: {}",
            render_oxc_diagnostics(source, filename, &parse_ret.diagnostics)
        ));
    }

    let mut program = parse_ret.program;
    let semantic_ret = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);

    if !semantic_ret.diagnostics.is_empty() {
        return Err(format!(
            "TypeScript 语义分析错误: {}",
            render_oxc_diagnostics(source, filename, &semantic_ret.diagnostics)
        ));
    }

    let scoping = semantic_ret.semantic.into_scoping();
    let path = Path::new(filename.unwrap_or("index.ts"));
    let mut options = TransformOptions::default();
    options.typescript.only_remove_type_imports = false;

    let transform_ret =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);

    if !transform_ret.diagnostics.is_empty() {
        return Err(format!(
            "TypeScript 转换错误: {}",
            render_oxc_diagnostics(source, filename, &transform_ret.diagnostics)
        ));
    }

    let js_code = Codegen::new().build(&program).code;
    Ok(js_code)
}

/// 数一行里的括号净增量（`{`/`[`/`(` 记 +1，对应闭括号记 -1）。
///
/// **跳过字符串与注释**：`"{ "` 这种字面量里的括号会把深度算错，而深度算错的后果是
/// 给一条没写完的语句补 `;`（正式版就是这么坏掉的，见下面 `export default` 分支的注释）。
pub(super) fn bracket_delta(line: &str) -> i32 {
    #[derive(PartialEq)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }

    let mut state = State::Code;
    let mut delta = 0i32;
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        match state {
            State::Code => match c {
                '\'' => state = State::Single,
                '"' => state = State::Double,
                '`' => state = State::Template,
                '/' if next == Some('/') => state = State::LineComment,
                '/' if next == Some('*') => state = State::BlockComment,
                '{' | '[' | '(' => delta += 1,
                '}' | ']' | ')' => delta -= 1,
                _ => {}
            },
            State::Single => {
                if c == '\\' {
                    i += 1;
                } else if c == '\'' {
                    state = State::Code;
                }
            }
            State::Double => {
                if c == '\\' {
                    i += 1;
                } else if c == '"' {
                    state = State::Code;
                }
            }
            State::Template => {
                if c == '\\' {
                    i += 1;
                } else if c == '`' {
                    state = State::Code;
                }
            }
            State::LineComment => {}
            State::BlockComment => {
                if c == '*' && next == Some('/') {
                    state = State::Code;
                    i += 1;
                }
            }
        }
        i += 1;
    }
    delta
}

/// 将 TypeScript 模块源码转译并转换为可在 CommonJS / 全局闭包沙箱中运行的标准 JS 脚本
pub fn transpile_ts_module(source: &str, filename: Option<&str>) -> Result<String, String> {
    let stripped = oxc_strip_types(source, filename)?;
    let mut out_lines = Vec::new();
    let mut trailing_exports = Vec::new();
    // 还差几个闭括号，当前这条 `export …` 语句才算写完。
    //
    // 🔴 为什么需要它：本函数是**行级文本替换**，而 oxc 的 codegen 会把对象的第一个属性
    // 留在 `export default {` 的同一行（`export default { tools: [{`）。老实现只判
    // `rest == "{"`，于是那种行会被当成"写完了的表达式"补一个 `;`，产出 `{ tools: [{;`
    // ——**语法错误**。表现就是"插件写得好好的却装载失败"，且只在第一个属性是数组/对象时复现。
    let mut pending_closers = 0i32;

    for line in stripped.lines() {
        let trimmed = line.trim();
        // 处理 import 语句
        if trimmed.starts_with("import ") {
            if let Some(rest) = trimmed.strip_prefix("import ") {
                let rest = rest.trim_end_matches(';').trim();
                if let Some(from_idx) = rest.rfind(" from ") {
                    let clause = rest[..from_idx].trim();
                    let spec = rest[from_idx + 6..]
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"');
                    if clause.starts_with('*') {
                        if let Some(as_idx) = clause.find(" as ") {
                            let name = clause[as_idx + 4..].trim();
                            out_lines.push(format!("const {} = require('{}');", name, spec));
                            continue;
                        }
                    } else if clause.starts_with('{') && clause.ends_with('}') {
                        out_lines.push(format!("const {} = require('{}');", clause, spec));
                        continue;
                    } else if clause.contains('{') {
                        if let Some(comma_idx) = clause.find(',') {
                            let default_name = clause[..comma_idx].trim();
                            let named_part = clause[comma_idx + 1..].trim();
                            out_lines.push(format!(
                                "const {} = (require('{}').default ?? require('{}'));",
                                default_name, spec, spec
                            ));
                            out_lines.push(format!("const {} = require('{}');", named_part, spec));
                            continue;
                        }
                    } else {
                        out_lines.push(format!(
                            "const {} = (require('{}').default ?? require('{}'));",
                            clause, spec, spec
                        ));
                        continue;
                    }
                } else {
                    let spec = rest.trim_matches(|c| c == '\'' || c == '"');
                    out_lines.push(format!("require('{}');", spec));
                    continue;
                }
            }
        }

        // 处理 export default
        if trimmed.starts_with("export default ") {
            let rest = trimmed.strip_prefix("export default ").unwrap().trim();
            let body = rest.trim_end_matches(';').trim();
            let delta = bracket_delta(body);
            if delta > 0 {
                // 表达式还没写完（后面的行会把它收尾）——**先不补分号**
                out_lines.push(format!("module.exports = exports.default = {body}"));
                pending_closers += delta;
            } else {
                out_lines.push(format!("module.exports = exports.default = {body};"));
            }
            continue;
        }

        // 处理 export const / let / var
        if trimmed.starts_with("export const ")
            || trimmed.starts_with("export let ")
            || trimmed.starts_with("export var ")
        {
            let decl = trimmed.strip_prefix("export ").unwrap();
            if let Some(eq_idx) = decl.find('=') {
                let left = decl[..eq_idx].trim();
                let var_name = left.split_whitespace().last().unwrap_or("");
                let right = decl[eq_idx + 1..].trim();
                let right = right.trim_end_matches(';').trim();
                let delta = bracket_delta(right);
                if delta > 0 {
                    out_lines.push(format!("{} = exports.{} = {}", left, var_name, right));
                    pending_closers += delta;
                } else {
                    out_lines.push(format!("{} = exports.{} = {};", left, var_name, right));
                }
                continue;
            }
        }

        // 处理 export function
        if trimmed.starts_with("export async function ") {
            let rest = trimmed.strip_prefix("export async function ").unwrap();
            if let Some(paren_idx) = rest.find('(') {
                let fn_name = rest[..paren_idx].trim();
                trailing_exports.push(format!("exports.{0} = {0};", fn_name));
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                out_lines.push(format!(
                    "{leading_ws}async function {}{}",
                    fn_name,
                    &rest[paren_idx..]
                ));
                continue;
            }
        } else if trimmed.starts_with("export function ") {
            let rest = trimmed.strip_prefix("export function ").unwrap();
            if let Some(paren_idx) = rest.find('(') {
                let fn_name = rest[..paren_idx].trim();
                trailing_exports.push(format!("exports.{0} = {0};", fn_name));
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                out_lines.push(format!(
                    "{leading_ws}function {}{}",
                    fn_name,
                    &rest[paren_idx..]
                ));
                continue;
            }
        } else if trimmed.starts_with("export class ") {
            let rest = trimmed.strip_prefix("export class ").unwrap();
            let class_name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('{')
                .trim();
            if !class_name.is_empty() {
                trailing_exports.push(format!("exports.{0} = {0};", class_name));
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                out_lines.push(format!("{leading_ws}class {}", rest));
                continue;
            }
        }

        // 处理 export {} 或 export { a, b as c } 具名导出
        if trimmed.starts_with("export ") {
            let rest = trimmed
                .strip_prefix("export ")
                .unwrap()
                .trim_end_matches(';')
                .trim();
            if rest.starts_with('{') && rest.ends_with('}') {
                let inner = rest[1..rest.len() - 1].trim();
                if inner.is_empty() {
                    // 空 export {}; 直接忽略跳过
                    continue;
                }
                for item in inner.split(',') {
                    let item = item.trim();
                    if item.is_empty() {
                        continue;
                    }
                    if let Some(as_idx) = item.find(" as ") {
                        let orig = item[..as_idx].trim();
                        let alias = item[as_idx + 4..].trim();
                        out_lines.push(format!("exports.{} = {};", alias, orig));
                    } else {
                        out_lines.push(format!("exports.{} = {};", item, item));
                    }
                }
                continue;
            }
        }

        // 普通行：若正在等闭括号，累计深度；**归零那一行才补分号**
        if pending_closers > 0 {
            pending_closers += bracket_delta(line);
            if pending_closers <= 0 {
                pending_closers = 0;
                out_lines.push(format!("{};", line.trim_end_matches(';')));
            } else {
                out_lines.push(line.to_string());
            }
            continue;
        }

        out_lines.push(line.to_string());
    }

    out_lines.extend(trailing_exports);

    Ok(out_lines.join("\n"))
}
