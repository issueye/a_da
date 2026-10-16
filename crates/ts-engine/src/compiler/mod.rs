use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// 把 oxc 诊断渲染成**带源码位置与上下文行**的说明（P4 · SP-73）。
///
/// # 为什么值得单独做
///
/// 插件作者看到的原文是 `TypeScript 解析错误: Unexpected token`——**没有行号**。
/// 一个几百行的插件里，这句话等于什么都没说：他得自己猜是哪一行。有了行列与那一行的原文，
/// 大部分语法错误一眼就能改。
///
/// 输出形如：
/// ```text
/// Unexpected token
///   → /path/to/p.ts:12:5
///   |     const x = ;
///   |             ^
/// ```
fn render_oxc_diagnostics(
    source: &str,
    filename: Option<&str>,
    diagnostics: &[OxcDiagnostic],
) -> String {
    let file = filename.unwrap_or("<inline>");
    let mut rendered = Vec::new();
    for d in diagnostics {
        let mut block = d.to_string();
        // 取第一条带 span 的标签来定位（oxc 的解析/语义诊断都至少带一条）
        if let Some(label) = d.labels.first() {
            let offset = label.offset() as usize;
            let (line_no, col_no) = line_col_of(source, offset);
            block.push_str(&format!("\n  → {file}:{line_no}:{col_no}"));
            if let Some(text) = source.lines().nth(line_no.saturating_sub(1)) {
                block.push_str(&format!("\n  | {text}"));
                // 插入符按**字符**数对齐（列号是从 1 开始的字符位置）
                block.push_str(&format!("\n  | {}^", " ".repeat(col_no.saturating_sub(1))));
            }
        }
        rendered.push(block);
    }
    rendered.join("\n")
}

/// 字节偏移 → `(行号, 列号)`，都从 1 开始（给人看的，与编辑器一致）。
///
/// 列号按**字符**算而不是字节：中文注释在插件里很常见，按字节会得到"第 37 列"这种
/// 编辑器里根本对不上的数字。
fn line_col_of(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    let col = source[line_start..offset].chars().count() + 1;
    (line, col)
}

/// 转译结果缓存（P1 · SP-17）。
///
/// # 为什么需要
///
/// 沙箱**每次调用**都要重新读文件 + 重新转译（`PluginSandbox::load_and_transpile` 在
/// 每次 `call_tool` 里都跑一遍），而转译是纯函数：同样的源 + 同样的文件名 + 同样的编译器
/// 版本，必然得到同样的 JS。插件被模型调 10 次就白转 9 次。
///
/// # 三条约束
///
/// 1. **内容寻址**：键 = `sha256(源)` + 文件名 + 引擎版本。不含路径之外的环境依赖，
///    所以不会出现"文件改了却命中旧产物"；
/// 2. **有界**：容量上限 [`TRANSPILE_CACHE_CAP`]，超了按插入顺序淘汰（不做 LRU，
///    省一次表维护；这里没有"热点会变"的场景）；
/// 3. **只缓存成功**：失败必须每次重算——否则一条语法错误会僵在缓存里，
///    用户改好文件后仍然看到旧错误。
///
/// 进程内缓存（不落盘）：跨进程复用要等真有那个需求（P4 的子进程档）；在那之前，
/// 内存命中比落盘读回还快，也就没有理由先引入 IO。
struct TranspileCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
    hits: u64,
    misses: u64,
}

/// 缓存容量上限（条目数）。
pub const TRANSPILE_CACHE_CAP: usize = 64;

fn cache() -> &'static Mutex<TranspileCache> {
    static CACHE: OnceLock<Mutex<TranspileCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TranspileCache {
            map: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
        })
    })
}

/// 缓存统计（**没有它，缓存测试等于空转**：纯函数"同样输入同样输出"本来就成立，
/// 分不出"命中了"与"又算了一遍"）。同时它也是埋点数据的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranspileCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub len: usize,
    pub capacity: usize,
}

pub fn transpile_cache_stats() -> TranspileCacheStats {
    let guard = cache().lock().expect("转译缓存锁中毒");
    TranspileCacheStats {
        hits: guard.hits,
        misses: guard.misses,
        len: guard.map.len(),
        capacity: TRANSPILE_CACHE_CAP,
    }
}

/// 清空缓存（测试用：让计数可预期）。
pub fn clear_transpile_cache() {
    let mut guard = cache().lock().expect("转译缓存锁中毒");
    guard.map.clear();
    guard.order.clear();
    guard.hits = 0;
    guard.misses = 0;
}

fn cache_key(source: &str, filename: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    // 文件名参与键：转译的 `SourceType` 由扩展名决定（`.ts` / `.tsx` / `.js` 解析规则不同）
    format!(
        "{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        filename.unwrap_or("<inline>"),
        hex
    )
}

/// 带缓存的转译（热路径用这个）。
///
/// 语义与 [`transpile_ts_module`] **完全一致**——缓存只影响耗时，不影响结果。
pub fn transpile_ts_module_cached(source: &str, filename: Option<&str>) -> Result<String, String> {
    let key = cache_key(source, filename);
    {
        let mut guard = cache().lock().expect("转译缓存锁中毒");
        if let Some(hit) = guard.map.get(&key).cloned() {
            guard.hits += 1;
            return Ok(hit);
        }
        guard.misses += 1;
    }

    // 转译在锁外做：转译本身可能要几毫秒到几十毫秒，不该把别人堵在临界区外
    let produced = transpile_ts_module(source, filename)?;

    let mut guard = cache().lock().expect("转译缓存锁中毒");
    if !guard.map.contains_key(&key) {
        guard.map.insert(key.clone(), produced.clone());
        guard.order.push_back(key);
        while guard.order.len() > TRANSPILE_CACHE_CAP {
            if let Some(oldest) = guard.order.pop_front() {
                guard.map.remove(&oldest);
            }
        }
    }
    Ok(produced)
}

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

    let transform_ret = Transformer::new(&allocator, path, &options)
        .build_with_scoping(scoping, &mut program);

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
fn bracket_delta(line: &str) -> i32 {
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
        if trimmed.starts_with("export const ") || trimmed.starts_with("export let ") || trimmed.starts_with("export var ") {
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

#[cfg(test)]
mod tests {
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
    fn test_bracket_delta_ignores_strings_and_comments() {        assert_eq!(bracket_delta("const a = { x: 1 };"), 0);
        assert_eq!(bracket_delta("module.exports = exports.default = { tools: [{" ), 3);
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
        let err = transpile_ts_module(src, Some("/plugins/broken.ts"))
            .expect_err("语法错误必须报错");
        assert!(err.contains("/plugins/broken.ts:"), "要点出文件与行列：{err}");
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
}

