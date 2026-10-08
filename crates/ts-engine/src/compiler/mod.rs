use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use std::path::Path;

/// 使用 100% 纯 Rust 的 OXC 编译器套件，将 TypeScript 源码极速擦除类型并转换为标准 JavaScript。
pub fn oxc_strip_types(source: &str, filename: Option<&str>) -> Result<String, String> {
    let allocator = Allocator::default();
    let source_type = if let Some(name) = filename {
        SourceType::from_path(Path::new(name)).unwrap_or_else(|_| SourceType::ts())
    } else {
        SourceType::ts()
    };

    let parse_ret = Parser::new(&allocator, source, source_type).parse();
    if !parse_ret.diagnostics.is_empty() {
        let err_msgs: Vec<String> = parse_ret.diagnostics.into_iter().map(|e| e.to_string()).collect();
        return Err(format!("TypeScript 解析错误: {}", err_msgs.join("; ")));
    }

    let mut program = parse_ret.program;
    let semantic_ret = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);

    if !semantic_ret.diagnostics.is_empty() {
        let err_msgs: Vec<String> = semantic_ret.diagnostics.into_iter().map(|e| e.to_string()).collect();
        return Err(format!("TypeScript 语义分析错误: {}", err_msgs.join("; ")));
    }

    let scoping = semantic_ret.semantic.into_scoping();
    let path = Path::new(filename.unwrap_or("index.ts"));
    let mut options = TransformOptions::default();
    options.typescript.only_remove_type_imports = false;

    let transform_ret = Transformer::new(&allocator, path, &options)
        .build_with_scoping(scoping, &mut program);

    if !transform_ret.diagnostics.is_empty() {
        let err_msgs: Vec<String> = transform_ret.diagnostics.into_iter().map(|e| e.to_string()).collect();
        return Err(format!("TypeScript 转换错误: {}", err_msgs.join("; ")));
    }

    let js_code = Codegen::new().build(&program).code;
    Ok(js_code)
}

/// 将 TypeScript 模块源码转译并转换为可在 CommonJS / 全局闭包沙箱中运行的标准 JS 脚本
pub fn transpile_ts_module(source: &str, filename: Option<&str>) -> Result<String, String> {
    let stripped = oxc_strip_types(source, filename)?;
    let mut out_lines = Vec::new();
    let mut trailing_exports = Vec::new();

    for line in stripped.lines() {
        let trimmed = line.trim();
        // 处理 import 语句
        if trimmed.starts_with("import ") {
            if let Some(rest) = trimmed.strip_prefix("import ") {
                let rest = rest.trim_end_matches(';').trim();
                if let Some(from_idx) = rest.rfind(" from ") {
                    let clause = rest[..from_idx].trim();
                    let spec = rest[from_idx + 6..].trim().trim_matches(|c| c == '\'' || c == '"');
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
                            out_lines.push(format!("const {} = (require('{}').default ?? require('{}'));", default_name, spec, spec));
                            out_lines.push(format!("const {} = require('{}');", named_part, spec));
                            continue;
                        }
                    } else {
                        out_lines.push(format!("const {} = (require('{}').default ?? require('{}'));", clause, spec, spec));
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
            if rest == "{" {
                out_lines.push("module.exports = exports.default = {".to_string());
            } else {
                let clean = rest.trim_end_matches(';').trim();
                out_lines.push(format!("module.exports = exports.default = {};", clean));
            }
            continue;
        }

        // 处理 export const / let / var
        if trimmed.starts_with("export const ") || trimmed.starts_with("export let ") || trimmed.starts_with("export var ") {
            let decl = trimmed.strip_prefix("export ").unwrap();
            if let Some(eq_idx) = decl.find('=') {
                let left = decl[..eq_idx].trim();
                let var_name = left.split_whitespace().last().unwrap_or("");
                let right = decl[eq_idx + 1..].trim();
                out_lines.push(format!("{} = exports.{} = {};", left, var_name, right));
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
                out_lines.push(format!("{leading_ws}async function {}{}", fn_name, &rest[paren_idx..]));
                continue;
            }
        } else if trimmed.starts_with("export function ") {
            let rest = trimmed.strip_prefix("export function ").unwrap();
            if let Some(paren_idx) = rest.find('(') {
                let fn_name = rest[..paren_idx].trim();
                trailing_exports.push(format!("exports.{0} = {0};", fn_name));
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                out_lines.push(format!("{leading_ws}function {}{}", fn_name, &rest[paren_idx..]));
                continue;
            }
        } else if trimmed.starts_with("export class ") {
            let rest = trimmed.strip_prefix("export class ").unwrap();
            let class_name = rest.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
            if !class_name.is_empty() {
                trailing_exports.push(format!("exports.{0} = {0};", class_name));
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                out_lines.push(format!("{leading_ws}class {}", rest));
                continue;
            }
        }

        // 处理 export {} 或 export { a, b as c } 具名导出
        if trimmed.starts_with("export ") {
            let rest = trimmed.strip_prefix("export ").unwrap().trim_end_matches(';').trim();
            if rest.starts_with('{') && rest.ends_with('}') {
                let inner = rest[1..rest.len() - 1].trim();
                if inner.is_empty() {
                    // 空 export {}; 直接忽略跳过
                    continue;
                }
                for item in inner.split(',') {
                    let item = item.trim();
                    if item.is_empty() { continue; }
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

        out_lines.push(line.to_string());
    }

    out_lines.extend(trailing_exports);

    Ok(out_lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_basic_typescript() {
        let ts_code = r#"
            interface User {
                id: number;
                name: string;
            }

            function greet(user: User): string {
                return `Hello, ${user.name} (id: ${user.id})!`;
            }

            const u: User = { id: 42, name: "Antigravity" };
            greet(u);
        "#;

        let js_code = oxc_strip_types(ts_code, Some("test.ts")).expect("转译失败");
        assert!(!js_code.contains("interface User"), "应当擦除 interface 定义");
        assert!(!js_code.contains(": User"), "应当擦除类型注解");
        assert!(!js_code.contains(": string"), "应当擦除返回值类型注解");
        assert!(js_code.contains("function greet(user)"), "应当保留函数声明主体");
    }

    #[test]
    fn test_execute_ts_in_boa() {
        use boa_engine::{Context, Source};

        let ts_code = r#"
            type NumberPair = [number, number];
            const calculate = (pair: NumberPair): number => {
                const [a, b] = pair;
                return a * 10 + b;
            };
            calculate([3, 7]);
        "#;

        let js_code = oxc_strip_types(ts_code, Some("calc.ts")).expect("转译失败");
        let mut context = Context::default();
        let result = context.eval(Source::from_bytes(&js_code)).expect("Boa 执行失败");
        let res_num = result.as_number().expect("返回值应为数字");
        assert_eq!(res_num as i64, 37);
    }

    #[test]
    fn test_transpile_ts_module_and_eval() {
        use boa_engine::{Context, Source};
        let ts_plugin = r#"
            import { join } from 'node:path';

            interface PluginConfig {
                tag: string;
            }

            export default {
                name: "demo-plugin",
                description: "TS 转译插件测试",
                tools: [
                    (ws: string) => ({
                        name: "demo_tool",
                        description: "测试工具",
                        parameters: { type: "object" },
                        execute: async (callId: string, args: any) => {
                            return { ok: true, output: `Hello from ${join(ws, "sub")}` };
                        }
                    })
                ]
            };
        "#;

        let js_body = transpile_ts_module(ts_plugin, Some("plugin.ts")).expect("模块转译失败");
        println!("Transpiled JS Body:\n{}", js_body);
        let wrapped = format!(
            r#"
            (function() {{
                const module = {{ exports: {{}} }};
                const exports = module.exports;
                {}
                return module.exports.default || module.exports;
            }})();
            "#,
            js_body
        );

        let mut context = Context::default();
        // 注入 path polyfill 以便 require('node:path') 正常工作
        crate::env::inject_node_environment(&mut context, None).expect("注入环境失败");

        let eval_res = context.eval(Source::from_bytes(&wrapped)).expect("评估失败");
        assert!(eval_res.is_object(), "返回值应当是插件描述符对象");

        let obj = eval_res.as_object().unwrap();
        let name_val = obj.get(boa_engine::js_string!("name"), &mut context).unwrap();
        assert_eq!(name_val.to_string(&mut context).unwrap().to_std_string_escaped(), "demo-plugin");
    }

    #[test]
    fn test_transpile_export_function_scoping() {
        use boa_engine::{Context, Source};
        let ts_code = r#"
            export function parseResults(raw: string): string {
                return `parsed:${raw}`;
            }

            export default function run(api: string): string {
                return parseResults(api);
            }
        "#;

        let js_body = transpile_ts_module(ts_code, Some("test.ts")).expect("模块转译失败");
        let wrapped = format!(
            r#"
            (function() {{
                const module = {{ exports: {{}} }};
                const exports = module.exports;
                {}
                return {{
                    result: (module.exports.default || module.exports)("world"),
                    hasNamed: typeof exports.parseResults === 'function'
                }};
            }})();
            "#,
            js_body
        );

        let mut context = Context::default();
        crate::env::inject_node_environment(&mut context, None).expect("注入环境失败");
        let eval_res = context.eval(Source::from_bytes(&wrapped)).expect("评估失败");
        assert!(eval_res.is_object());
        let obj = eval_res.as_object().unwrap();
        let result_val = obj.get(boa_engine::js_string!("result"), &mut context).unwrap();
        assert_eq!(result_val.to_string(&mut context).unwrap().to_std_string_escaped(), "parsed:world");
        let has_named = obj.get(boa_engine::js_string!("hasNamed"), &mut context).unwrap();
        assert_eq!(has_named.to_boolean(), true);
    }
}

