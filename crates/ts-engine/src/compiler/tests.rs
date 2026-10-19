use super::diagnostics::line_col_of;
use super::transpile::bracket_delta;
use super::*;

/// 回归：**第一个属性是数组/对象**的 `export default` 不能被行级替换写出语法错误。
///
/// 实地踩到的形态（P1 · SP-10 接通脚本宿主时）：`export default { tools: [ { ... } ] }`
/// 经 oxc codegen 后首行是 `export default { tools: [{`，老实现把它当成写完了的表达式
/// 补 `;` → `{ tools: [{;` → 装载期"TypeScript 解析错误: Unexpected token"。
/// 而"第一个属性是 name"的写法恰好把 `{` 留在行尾，所以**只有一种写法会挂**——
/// 这类"看运气"的 bug 必须用回归用例钉死。
#[test]
fn test_export_default_with_inline_nested_opening_is_valid() {
    let src = r#"export default {
  tools: [
    {
      name: 'echo',
      description: '回显',
      execute: async (args: any) => {
        return { ok: true, output: 'echo:' + args.text }
      },
    },
  ],
}
"#;
    let js = transpile_ts_module(src, Some("plugin.ts")).expect("转译应当成功");

    // 1. 不许出现 `{;` 这种"给没写完的表达式补分号"的产物
    assert!(
        !js.contains("{;"),
        "转译产物里出现了 `{{;`——行级替换又在给未完成的表达式补分号：\n{js}"
    );
    assert!(js.contains("module.exports = exports.default ="), "{js}");

    // 2. 产物**必须真的能再被解析**（这才是"能不能跑"的判据，而不是字符串好看）
    oxc_strip_types(&js, Some("plugin.js"))
        .unwrap_or_else(|e| panic!("转译产物本身不是合法 JS：{e}\n{js}"));

    // 3. 与老写法（`{` 独占行尾）产出的**语义等价**：同样能被解析且带同一个导出
    let src_braced = src.replace("export default {\n", "export default\n{\n");
    let js2 = transpile_ts_module(&src_braced, Some("plugin.ts")).expect("转译应当成功");
    oxc_strip_types(&js2, Some("plugin.js")).expect("另一种换行写法也必须合法");
}

/// 括号计数要**跳过字符串与注释**，否则 `"{ "` 这种字面量会把深度算错。
#[test]
fn test_bracket_delta_ignores_strings_and_comments() {
    assert_eq!(bracket_delta("const a = { x: 1 };"), 0);
    assert_eq!(
        bracket_delta("module.exports = exports.default = { tools: [{"),
        3
    );
    assert_eq!(bracket_delta("}];"), -2);
    assert_eq!(bracket_delta(r#"const s = "}}";"#), 0, "字符串里的括号不算");
    assert_eq!(bracket_delta("// } 注释里的括号不算"), 0);
    assert_eq!(bracket_delta("/* { */"), 0);
    assert_eq!(bracket_delta(r#"const t = `}`;"#), 0, "模板串里的括号不算");
}

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
    assert!(
        !js_code.contains("interface User"),
        "应当擦除 interface 定义"
    );
    assert!(!js_code.contains(": User"), "应当擦除类型注解");
    assert!(!js_code.contains(": string"), "应当擦除返回值类型注解");
    assert!(
        js_code.contains("function greet(user)"),
        "应当保留函数声明主体"
    );
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
    let result = context
        .eval(Source::from_bytes(&js_code))
        .expect("Boa 执行失败");
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

    let eval_res = context
        .eval(Source::from_bytes(&wrapped))
        .expect("评估失败");
    assert!(eval_res.is_object(), "返回值应当是插件描述符对象");

    let obj = eval_res.as_object().unwrap();
    let name_val = obj
        .get(boa_engine::js_string!("name"), &mut context)
        .unwrap();
    assert_eq!(
        name_val
            .to_string(&mut context)
            .unwrap()
            .to_std_string_escaped(),
        "demo-plugin"
    );
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
    let eval_res = context
        .eval(Source::from_bytes(&wrapped))
        .expect("评估失败");
    assert!(eval_res.is_object());
    let obj = eval_res.as_object().unwrap();
    let result_val = obj
        .get(boa_engine::js_string!("result"), &mut context)
        .unwrap();
    assert_eq!(
        result_val
            .to_string(&mut context)
            .unwrap()
            .to_std_string_escaped(),
        "parsed:world"
    );
    let has_named = obj
        .get(boa_engine::js_string!("hasNamed"), &mut context)
        .unwrap();
    assert_eq!(has_named.to_boolean(), true);
}

/// SP-17：缓存必须**真的命中**——靠计数判定，不能靠"输出一样"
/// （纯函数"同样输入同样输出"本来就成立，那种断言分不出"命中了"与"又算了一遍"）。
#[test]
fn test_transpile_cache_hits_is_content_addressed_and_bounded() {
    clear_transpile_cache();
    let src = "export default { tools: [] }";

    let first = transpile_ts_module_cached(src, Some("demo.ts")).expect("首次转译应当成功");
    assert_eq!(transpile_cache_stats().misses, 1, "首次必须是未命中");

    let second = transpile_ts_module_cached(src, Some("demo.ts")).expect("第二次也应当成功");
    let after = transpile_cache_stats();
    assert_eq!(second, first, "缓存不得改变结果");
    assert!(after.hits >= 1, "第二次必须命中缓存（hits={}）", after.hits);

    // 文件名参与键：`.ts` 与 `.tsx` 的解析规则不同，不能互相命中
    let misses_before = transpile_cache_stats().misses;
    let _ = transpile_ts_module_cached(src, Some("demo.tsx")).expect("tsx 也应当成功");
    assert!(
        transpile_cache_stats().misses > misses_before,
        "不同文件名必须是不同的键"
    );

    // 失败**不进缓存**：否则一条语法错误会僵在那里，用户改好了还看到旧错误
    let bad = "export default { tools: [";
    assert!(transpile_ts_module_cached(bad, Some("bad.ts")).is_err());
    let misses_before = transpile_cache_stats().misses;
    assert!(transpile_ts_module_cached(bad, Some("bad.ts")).is_err());
    assert!(
        transpile_cache_stats().misses > misses_before,
        "失败必须每次重算（缓存里不许留错误）"
    );

    // 有界：灌满之后长度必须被上限兜住
    for i in 0..(TRANSPILE_CACHE_CAP + 8) {
        let _ = transpile_ts_module_cached(
            &format!("export default {{ tools: [], n: {i} }}"),
            Some("bulk.ts"),
        )
        .expect("批量转译应当成功");
    }
    let stats = transpile_cache_stats();
    assert!(
        stats.len <= stats.capacity,
        "缓存必须被上限兜住，否则就是内存泄漏：{stats:?}"
    );

    clear_transpile_cache();
}

/// P4 · SP-73 的 DoD 第三条：**插件报错能指回源代码行**。
///
/// 断言的是"位置"与"那一行的原文"真的出现——只断言"报了错"的话，
/// 一句 `Unexpected token` 也能让用例变绿，而那句话对作者毫无帮助。
///
/// 刻意**不**断言具体行号：解析器把错误挂在哪儿（出错处还是它发现问题的收尾处）
/// 是编译器实现细节，写死行号会让这条用例在编译器升级时无谓地变红。
#[test]
fn test_parse_error_points_at_the_source_line() {
    let src = "export default {\n  tools: [\n    { name: 'x', \n  ]\n};\n";
    let err = transpile_ts_module(src, Some("/plugins/broken.ts")).expect_err("语法错误必须报错");
    assert!(
        err.contains("/plugins/broken.ts:"),
        "要点出文件与行列：{err}"
    );
    assert!(
        err.contains("  | "),
        "要带出出错那一行的原文（作者一眼能定位）：{err}"
    );
    assert!(err.contains('^'), "要有插入符指向列：{err}");
    // 指向的那一行必须真的是源码里的某一行（而不是编造的上下文）
    let pointed = err
        .lines()
        .find_map(|l| l.strip_prefix("  | "))
        .unwrap_or_default();
    assert!(
        src.lines().any(|l| l.trim_end() == pointed.trim_end()),
        "带出的原文必须来自源码：{pointed:?}\n{err}"
    );
}

/// 行列按**字符**算而不是字节：中文注释在前时，字节列号会大得离谱。
#[test]
fn test_line_col_counts_characters_not_bytes() {
    let src = "// 这是一段中文注释，占了很宽的位置\nconst x = ;\n";
    let (line, col) = line_col_of(src, src.find(';').unwrap());
    assert_eq!(line, 2, "第二行");
    assert_eq!(col, 11, "`const x = ` 之后是第 11 个字符（按字符算）");
}
