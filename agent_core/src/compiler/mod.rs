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
}
