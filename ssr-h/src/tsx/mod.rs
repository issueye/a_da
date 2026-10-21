//! **TSX 服务端组件（默认启用）**
//!
//! 方案 C：TSX → OXC（去类型 + JSX classic pragma `h`）→ Boa（无宿主环境）
//! → VNode JSON → Rust `ui` 组件渲染。`.tsx` 页面 / 插槽 / 仪表盘已接入插件注册表。
//!
//! 单文件直接 OXC 转译；含 import/export 的插件在加载/重载时递归转译相对模块，
//! 运行时交给 Boa 的 ESM loader（不需要 Bun/Node）。
//! 不做 hooks/事件/hydration；交互仍走 HTMX/Alpine。
//!
//! 约定：
//! - TSX 文件顶层必须声明 `function Page(props) { ... }`；
//! - 大写组件名（`<Button>` / `<Chip>` …）由 Rust 渲染；
//! - 小写标签（`<div>` / `<a>` …）走内置 HTML 白名单、URL 校验与转义；
//! - 服务端不允许函数 prop / `onX` / `dangerouslySetInnerHTML`；
//! - 不支持 import/export、hooks、async、事件回调、hydration。
//!
//! 运行：`cargo test --manifest-path ssr-h/Cargo.toml -- --nocapture`

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::{resolve_module_specifier, Module, ModuleLoader, ModuleRequest, Referrer};
use boa_engine::{js_string, Context, JsError, JsNativeError, JsResult, JsValue, Source};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{JsxRuntime, TransformOptions, Transformer};
use serde_json::{Map, Value};

use crate::html;

use crate::ui::icon;
use crate::ui::theme::escape;

const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_RENDER_DEPTH: usize = 64;
const MAX_SCRIPT_LOOP: u64 = 100_000;
const MAX_SOURCE_BYTES: usize = 512 * 1024;

const BUILTIN_COMPONENTS_KEY: &str = "__ssr_h_components.js";
const BUILTIN_COMPONENT_SPECIFIERS: &[&str] =
    &["ssr-h:components", "ssr-h/components", "@ssr-h/components"];

fn is_builtin_component_specifier(specifier: &str) -> bool {
    BUILTIN_COMPONENT_SPECIFIERS.contains(&specifier)
}

/// 内置组件库的 JS（由 OXC 从 `components.tsx` 预转译一次）。
fn component_library_js() -> &'static str {
    static LIBRARY: OnceLock<String> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        transform_tsx(include_str!("components.tsx"), "components.tsx")
            .expect("内置 TSX 组件库必须能编译")
    })
}

/// 给 ESM 插件用的 `ssr-h:components` 包装模块。
fn component_module_js() -> &'static str {
    static MODULE: OnceLock<String> = OnceLock::new();
    MODULE.get_or_init(|| {
        let names = [
            "Button",
            "IconButton",
            "Chip",
            "Notice",
            "Panel",
            "Disclosure",
            "EmptyState",
            "Icon",
            "TextField",
            "Select",
            "Modal",
            "Form",
            "Todo",
            "UserMessage",
            "AssistantMessage",
        ];
        let mut js = String::from("const __c = globalThis.__SSR_COMPONENTS__;\n");
        for name in names {
            js.push_str(&format!("export const {name} = __c.{name};\n"));
        }
        js
    })
}

/// 把内置组件库注入当前 Context；必须在执行插件脚本/模块之前调用。
fn install_component_library(context: &mut Context) -> Result<(), String> {
    context
        .eval(Source::from_bytes(component_library_js()))
        .map_err(|error| format!("内置 TSX 组件库初始化失败: {error}"))?;
    Ok(())
}

/// JSX classic runtime（组件库由 `components.tsx` 提供）。
const PRELUDE: &str = r##"
const Fragment = "#fragment";

function __collect(value, out) {
  if (value === null || value === undefined || typeof value === "boolean") return;
  if (Array.isArray(value)) {
    for (var i = 0; i < value.length; i += 1) __collect(value[i], out);
    return;
  }
  out.push(value);
}

function __checkProps(props) {
  var keys = Object.keys(props);
  for (var i = 0; i < keys.length; i += 1) {
    var key = keys[i];
    var value = props[key];
    if (typeof value === "function") {
      throw new Error("TSX 服务端渲染不接受函数 prop: " + key);
    }
    if (/^on[A-Z]/.test(key)) {
      throw new Error("TSX 服务端渲染不接受事件 prop: " + key);
    }
    if (key === "dangerouslySetInnerHTML") {
      throw new Error("TSX spike 不接受 dangerouslySetInnerHTML");
    }
  }
}

function h(type, props, ...children) {
  var kids = [];
  __collect(children, kids);
  props = Object.assign({}, props || {});
  if (props.children !== undefined) {
    __collect(props.children, kids);
    delete props.children;
  }
  // React 习惯里的 `key` 只是列表身份，服务端渲染不需要（Rust 也不认它）。
  delete props.key;
  __checkProps(props);
  if (typeof type === "function") {
    var merged = Object.assign({}, props);
    if (kids.length === 1) merged.children = kids[0];
    else if (kids.length > 1) merged.children = kids;
    return type(merged);
  }
  if (typeof type !== "string" || type.length === 0) {
    throw new Error("非法 JSX 类型: " + String(type));
  }
  return { type: type, props: props, children: kids };
}
"##;

/// 一份已编译的 TSX。
#[derive(Debug, Clone)]
pub struct CompiledTsx {
    pub entry: TsxEntry,
}

#[derive(Debug, Clone)]
pub enum TsxEntry {
    /// 单文件：转译后的脚本，入口是全局 `Page`。
    Script(Arc<str>),
    /// 含 import/export：预先转译好的模块集合，入口是模块命名空间里的 `Page`/default。
    Module(Arc<TsxModule>),
}

#[derive(Debug, Clone)]
pub struct TsxModule {
    /// 相对路径 → 已转译的 ESM JS。
    pub modules: std::collections::HashMap<PathBuf, Arc<str>>,
    /// 入口模块的相对路径。
    pub entry: PathBuf,
}

/// 编译一段单文件 TSX（含 import/export 的模块走 [`compile_file`]）。
pub fn compile(source: &str, filename: &str) -> Result<Arc<CompiledTsx>, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!("TSX 源文件超过 {} KB", MAX_SOURCE_BYTES / 1024));
    }
    if has_module_syntax(source) {
        return Err("TSX 含 import/export：请通过插件文件加载，由内置模块加载器处理".to_string());
    }
    let js = transform_tsx(source, filename)?;
    Ok(Arc::new(CompiledTsx {
        entry: TsxEntry::Script(Arc::from(js)),
    }))
}

/// 从插件目录读取并编译一个 `.tsx` 文件（路径必须落在插件根内）。
///
/// 单文件直接 OXC 转译；含 import/export 的源文件会递归转译相对模块，
/// 运行时用 Boa 的模块加载器执行，**不需要 Bun/Node**。
pub fn compile_file(path: &Path, root: &Path) -> Result<Arc<CompiledTsx>, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("插件根目录无效: {error}"))?;
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("读取 TSX 失败: {}: {error}", path.display()))?;
    if !canonical.starts_with(&root) {
        return Err(format!("TSX 路径越界: {}", path.display()));
    }
    let source = std::fs::read_to_string(&canonical)
        .map_err(|error| format!("读取 TSX 失败: {}: {error}", canonical.display()))?;
    if has_module_syntax(&source) {
        let module = compile_module(&canonical, &root)?;
        return Ok(Arc::new(CompiledTsx {
            entry: TsxEntry::Module(Arc::new(module)),
        }));
    }
    compile(&source, &canonical.to_string_lossy())
}

/// 编译入口模块及其相对依赖：预先 OXC 转译每个模块，运行时交给 Boa ESM loader。
fn compile_module(entry: &Path, root: &Path) -> Result<TsxModule, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("插件根目录无效: {error}"))?;
    let entry = entry
        .canonicalize()
        .map_err(|error| format!("TSX 入口无效: {error}"))?;
    let mut modules: HashMap<PathBuf, Arc<str>> = HashMap::new();
    let mut queue = vec![entry.clone()];
    let mut needs_builtin = false;
    while let Some(path) = queue.pop() {
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| format!("TSX 路径越界: {}", path.display()))?
            .to_path_buf();
        if modules.contains_key(&relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取 TSX 模块失败: {}: {error}", path.display()))?;
        if source.len() > MAX_SOURCE_BYTES {
            return Err(format!(
                "TSX 模块 `{}` 超过 {} KB",
                relative.display(),
                MAX_SOURCE_BYTES / 1024
            ));
        }
        let js = transform_tsx(&source, &path.to_string_lossy())?;
        for specifier in module_specifiers(&source, &path)? {
            if is_builtin_component_specifier(&specifier) {
                needs_builtin = true;
                continue;
            }
            let resolved = resolve_ts_module(&path, &specifier, &root)?;
            queue.push(resolved);
        }
        modules.insert(relative, Arc::from(js));
    }
    if needs_builtin {
        modules.insert(
            PathBuf::from(BUILTIN_COMPONENTS_KEY),
            Arc::from(component_module_js()),
        );
    }
    let entry = entry
        .strip_prefix(&root)
        .map_err(|_| format!("TSX 入口越界: {}", entry.display()))?
        .to_path_buf();
    Ok(TsxModule { modules, entry })
}

/// 从源码里收集静态 import/export ... from 的相对说明符。
fn module_specifiers(source: &str, filename: &Path) -> Result<Vec<String>, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(filename)
        .map_err(|error| format!("TSX 源类型识别失败: {error:?}"))?;
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!("TSX 解析失败: {:?}", parsed.diagnostics));
    }
    let mut specifiers = Vec::new();
    for statement in &parsed.program.body {
        match statement {
            oxc_ast::ast::Statement::ImportDeclaration(decl) => {
                specifiers.push(decl.source.value.to_string());
            }
            oxc_ast::ast::Statement::ExportFromDeclaration(decl) => {
                specifiers.push(decl.source.value.to_string());
            }
            oxc_ast::ast::Statement::ExportAllDeclaration(decl) => {
                specifiers.push(decl.source.value.to_string());
            }
            _ => {}
        }
    }
    Ok(specifiers)
}

fn resolve_ts_module(from: &Path, specifier: &str, root: &Path) -> Result<PathBuf, String> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return Err(format!(
            "TSX 模块 `{}` 引用了非相对模块 `{specifier}`：内置 loader 只支持插件目录内的相对导入",
            from.display()
        ));
    }
    let base = from
        .parent()
        .ok_or_else(|| format!("TSX 模块没有父目录: {}", from.display()))?
        .join(specifier);
    let mut candidates = Vec::new();
    candidates.push(base.clone());
    for extension in ["tsx", "ts", "jsx", "js", "mjs", "cjs"] {
        candidates.push(base.with_extension(extension));
    }
    if base.extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or("").to_ascii_lowercase().as_str(),
            "js" | "jsx" | "mjs" | "cjs"
        )
    }) {
        for extension in ["tsx", "ts"] {
            candidates.push(base.with_extension(extension));
        }
    }
    for extension in ["tsx", "ts", "jsx", "js", "mjs", "cjs"] {
        candidates.push(base.join(format!("index.{extension}")));
    }
    for candidate in candidates {
        if candidate.is_file() {
            let canonical = candidate
                .canonicalize()
                .map_err(|error| format!("解析 TSX 模块失败: {}: {error}", candidate.display()))?;
            if !canonical.starts_with(root) {
                return Err(format!("TSX 模块越界: {}", candidate.display()));
            }
            return Ok(canonical);
        }
    }
    Err(format!(
        "找不到 TSX 模块 `{specifier}`（从 `{}` 出发）",
        from.display()
    ))
}

/// Boa 的 ESM loader：按相对路径从预转译表里取模块，不访问网络/Node。
struct TsxModuleLoader {
    modules: HashMap<PathBuf, Arc<str>>,
    cache: RefCell<HashMap<PathBuf, Module>>,
}

impl TsxModuleLoader {
    fn new(modules: HashMap<PathBuf, Arc<str>>) -> Self {
        Self {
            modules,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn find_key(&self, path: &Path) -> Option<PathBuf> {
        if self.modules.contains_key(path) {
            return Some(path.to_path_buf());
        }
        for extension in ["tsx", "ts", "js", "jsx", "mjs", "cjs"] {
            let candidate = path.with_extension(extension);
            if self.modules.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext, "js" | "jsx" | "mjs" | "cjs"))
        {
            for extension in ["tsx", "ts"] {
                let candidate = path.with_extension(extension);
                if self.modules.contains_key(&candidate) {
                    return Some(candidate);
                }
            }
        }
        for extension in ["tsx", "ts", "js", "jsx", "mjs", "cjs"] {
            let candidate = path.join(format!("index.{extension}"));
            if self.modules.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

impl ModuleLoader for TsxModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = (|| {
            let mut context = context.borrow_mut();
            let specifier = request.specifier().to_std_string_escaped();
            let path = if is_builtin_component_specifier(&specifier) {
                PathBuf::from(BUILTIN_COMPONENTS_KEY)
            } else {
                resolve_module_specifier(None, request.specifier(), referrer.path(), &mut context)?
            };
            if let Some(module) = self.cache.borrow().get(&path).cloned() {
                return Ok(module);
            }
            let key = self.find_key(&path).ok_or_else(|| {
                JsError::from(JsNativeError::typ().with_message(format!(
                    "模块 `{}` 未在预编译表里（只支持相对 TS/TSX 模块）",
                    path.display()
                )))
            })?;
            let js = self.modules.get(&key).cloned().ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ()
                        .with_message(format!("模块 `{}` 不在预编译表里", key.display())),
                )
            })?;
            let source = Source::from_bytes(js.as_bytes()).with_path(&key);
            let module = Module::parse(source, None, &mut context)?;
            self.cache.borrow_mut().insert(key, module.clone());
            Ok(module)
        })();
        async { result }
    }
}

fn evaluate_module(module: &TsxModule, props: &Value) -> Result<Value, String> {
    let props_json =
        serde_json::to_string(props).map_err(|error| format!("props 序列化失败: {error}"))?;
    let loader = Rc::new(TsxModuleLoader::new(module.modules.clone()));
    let mut context = Context::builder()
        .module_loader(loader)
        .build()
        .map_err(|error| format!("Boa 模块上下文初始化失败: {error}"))?;
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_SCRIPT_LOOP);
    context.runtime_limits_mut().set_recursion_limit(64);
    context
        .eval(Source::from_bytes(PRELUDE))
        .map_err(|error| format!("JSX runtime 初始化失败: {error}"))?;
    install_component_library(&mut context)?;
    let entry_js = module
        .modules
        .get(&module.entry)
        .ok_or_else(|| "TSX 入口模块缺失".to_string())?;
    let source = Source::from_bytes(entry_js.as_bytes()).with_path(&module.entry);
    let entry_module = Module::parse(source, None, &mut context)
        .map_err(|error| format!("TSX 模块解析失败: {error}"))?;
    let promise = entry_module.load_link_evaluate(&mut context);
    context
        .run_jobs()
        .map_err(|error| format!("TSX 模块作业失败: {error}"))?;
    match promise.state() {
        PromiseState::Fulfilled(_) => {}
        PromiseState::Rejected(reason) => {
            let text = reason
                .to_string(&mut context)
                .map(|value| value.to_std_string_escaped())
                .unwrap_or_else(|_| "未知错误".to_string());
            return Err(format!("TSX 模块执行失败: {text}"));
        }
        PromiseState::Pending => {
            return Err("TSX 模块仍在加载（不支持顶层 await/异步模块）".to_string())
        }
    }
    let namespace = entry_module.namespace(&mut context);
    let mut page = namespace
        .get(js_string!("Page"), &mut context)
        .unwrap_or_else(|_| JsValue::undefined());
    if page.is_undefined() {
        page = namespace
            .get(js_string!("default"), &mut context)
            .unwrap_or_else(|_| JsValue::undefined());
    }
    if !page.is_callable() {
        return Err("TSX 模块必须 export function Page(props) 或 export default 函数".to_string());
    }
    context
        .global_object()
        .set(js_string!("__SSR_PAGE__"), page, false, &mut context)
        .map_err(|error| format!("挂载 TSX 入口失败: {error}"))?;
    let program = format!("JSON.stringify(h(__SSR_PAGE__, {props_json}))");
    let value = context
        .eval(Source::from_bytes(&program))
        .map_err(|error| format!("TSX 执行失败: {error}"))?;
    let text = value
        .to_string(&mut context)
        .map_err(|error| format!("TSX 返回值转换失败: {error}"))?
        .to_std_string_escaped();
    serde_json::from_str(&text)
        .map_err(|error| format!("TSX 返回的 VNode 不是合法 JSON: {error}; {text}"))
}

/// 用已编译的 TSX 渲染 HTML。每次渲染使用新的 Boa Context，模块状态不会跨请求泄漏。
pub fn render(compiled: &CompiledTsx, props: &Value) -> Result<String, String> {
    let vnode = match &compiled.entry {
        TsxEntry::Script(js) => evaluate_page(js, "Page", props)?,
        TsxEntry::Module(module) => evaluate_module(module, props)?,
    };
    let mut out = String::new();
    render_node(&vnode, &mut out, 0)?;
    Ok(out)
}

/// 把一段 TSX 直接渲染成 HTML（测试/工具使用）。
#[cfg(test)]
pub fn render_tsx(source: &str, props: &Value) -> Result<String, String> {
    let compiled = compile(source, "spike.tsx")?;
    render(&compiled, props)
}

fn has_module_syntax(source: &str) -> bool {
    source.lines().map(str::trim_start).any(|line| {
        line.starts_with("import ")
            || line.starts_with("import{")
            || line.starts_with("export ")
            || line.starts_with("export{")
            || line.starts_with("export default")
    })
}

fn transform_tsx(source: &str, filename: &str) -> Result<String, String> {
    let path = Path::new(filename);
    let allocator = Allocator::default();
    let source_type =
        SourceType::from_path(path).map_err(|error| format!("TSX 源类型识别失败: {error:?}"))?;
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!("TSX 解析失败: {:?}", parsed.diagnostics));
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new().build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(format!("TSX 语义分析失败: {:?}", semantic.diagnostics));
    }
    let scoping = semantic.semantic.into_scoping();
    let mut options = TransformOptions::default();
    options.typescript.only_remove_type_imports = false;
    options.jsx.jsx_plugin = true;
    options.jsx.runtime = JsxRuntime::Classic;
    options.jsx.pragma = Some("h".to_string());
    options.jsx.pragma_frag = Some("Fragment".to_string());
    let transformed =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if !transformed.diagnostics.is_empty() {
        return Err(format!("TSX 转换失败: {:?}", transformed.diagnostics));
    }
    Ok(Codegen::new().build(&program).code)
}

fn evaluate_page(js: &str, entry: &str, props: &Value) -> Result<Value, String> {
    let props_json =
        serde_json::to_string(props).map_err(|error| format!("props 序列化失败: {error}"))?;
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_SCRIPT_LOOP);
    context.runtime_limits_mut().set_recursion_limit(64);
    context
        .eval(Source::from_bytes(PRELUDE))
        .map_err(|error| format!("JSX runtime 初始化失败: {error}"))?;
    install_component_library(&mut context)?;
    // 末尾用 `;` 断开上一条语句，避免 ASI 把调用吞进函数声明。
    let program = format!("{js}\n;JSON.stringify(h({entry}, {props_json}));");
    let value = context
        .eval(Source::from_bytes(&program))
        .map_err(|error| format!("TSX 执行失败: {error}"))?;
    let text = value
        .to_string(&mut context)
        .map_err(|error| format!("TSX 返回值转换失败: {error}"))?
        .to_std_string_escaped();
    serde_json::from_str(&text)
        .map_err(|error| format!("TSX 返回的 VNode 不是合法 JSON: {error}; {text}"))
}

fn render_node(node: &Value, out: &mut String, depth: usize) -> Result<(), String> {
    if depth > MAX_RENDER_DEPTH {
        return Err(format!("VNode 嵌套超过 {MAX_RENDER_DEPTH} 层"));
    }
    match node {
        Value::Null => Ok(()),
        Value::Bool(_) => Ok(()),
        Value::String(text) => append(out, &escape(text)),
        Value::Number(number) => append(out, &number.to_string()),
        Value::Array(items) => {
            for item in items {
                render_node(item, out, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            let Some(kind) = object.get("type").and_then(Value::as_str) else {
                return Err("VNode 缺少 type 字段".to_string());
            };
            let props = object.get("props").and_then(Value::as_object);
            let children = object.get("children").cloned().unwrap_or(Value::Null);
            if kind == "#fragment" {
                return render_node(&children, out, depth + 1);
            }
            if kind
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
            {
                return render_component(kind, props, &children, out, depth + 1);
            }
            render_element(kind, props, &children, out, depth + 1)
        }
    }
}

fn render_component(
    name: &str,
    props: Option<&Map<String, Value>>,
    _children: &Value,
    out: &mut String,
    _depth: usize,
) -> Result<(), String> {
    let props = props.ok_or_else(|| format!("组件 <{name}> 缺少 props"))?;
    if name != "Icon" {
        return Err(format!("未知的 Rust 组件 <{name}>"));
    }
    for key in props.keys() {
        if key != "children" && !matches!(key.as_str(), "name" | "size" | "class") {
            return Err(format!("组件 <{name}> 不支持属性 `{key}`"));
        }
    }
    let icon_name = prop_string(props, "name")?;
    let size = prop_usize(props, "size")?.unwrap_or(16).min(64) as u16;
    let svg = icon(&icon_name, size);
    let html = match prop_optional_string(props, "class")? {
        Some(class) if !class.trim().is_empty() => {
            let class = html::sanitize_class(&class)?;
            svg.replace("class=\"ada-icon\"", &format!("class=\"ada-icon {class}\""))
        }
        _ => svg,
    };
    append(out, &html)
}

fn render_element(
    tag: &str,
    props: Option<&Map<String, Value>>,
    children: &Value,
    out: &mut String,
    depth: usize,
) -> Result<(), String> {
    let element = html::builtin_elements()
        .into_iter()
        .find(|element| element.tag == tag)
        .ok_or_else(|| format!("不允许的 HTML 标签 <{tag}>"))?;
    let empty = Map::new();
    let props = props.unwrap_or(&empty);
    let mut present: Vec<String> = props.keys().cloned().collect();
    present.sort();
    html::check_required(&element, &present)?;

    let mut attrs = String::new();
    for key in &present {
        let value = &props[key];
        if value.is_null() {
            continue;
        }
        let key = match key.as_str() {
            "className" => "class",
            "htmlFor" => "for",
            other => other,
        };
        if !html::element_allows_attr(&element, key) {
            return Err(format!("标签 <{tag}> 不允许属性 `{key}`"));
        }
        if key.eq_ignore_ascii_case("hx-on") || key.starts_with("hx-on:") {
            return Err(format!(
                "标签 <{tag}> 不允许内联脚本属性 `{key}`；交互请用 hx-post/hx-get + 服务端动作"
            ));
        }
        match value {
            Value::Bool(true) => attrs.push_str(&format!(" {key}")),
            Value::Bool(false) => {}
            Value::String(text) => {
                let text = if key == "class" {
                    html::sanitize_class(text)?
                } else if matches!(key, "href" | "src" | "action") {
                    html::sanitize_url(text)?
                } else if key == "method" && tag == "form" {
                    let method = text.to_ascii_lowercase();
                    if !matches!(method.as_str(), "get" | "post") {
                        return Err(format!("form.method 只接受 get/post，收到 `{text}`"));
                    }
                    method
                } else {
                    text.clone()
                };
                attrs.push_str(&format!(" {key}=\"{}\"", escape(&text)));
            }
            Value::Number(number) => {
                attrs.push_str(&format!(" {key}=\"{}\"", number));
            }
            _ => return Err(format!("属性 `{key}` 只接受标量值")),
        }
    }

    append(out, &format!("<{tag}{attrs}"))?;
    if element.void {
        append(out, ">")?;
        return Ok(());
    }
    append(out, ">")?;
    render_node(children, out, depth)?;
    append(out, &format!("</{tag}>"))
}

fn append(out: &mut String, text: &str) -> Result<(), String> {
    out.push_str(text);
    if out.len() > MAX_OUTPUT_BYTES {
        return Err(format!("渲染输出超过 {MAX_OUTPUT_BYTES} 字节"));
    }
    Ok(())
}

fn prop_string(props: &Map<String, Value>, key: &str) -> Result<String, String> {
    props
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("缺少字符串 prop `{key}`"))
}

fn prop_optional_string(props: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match props.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("prop `{key}` 必须是字符串")),
    }
}

fn prop_usize(props: &Map<String, Value>, key: &str) -> Result<Option<usize>, String> {
    match props.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(|value| Some(value as usize))
            .ok_or_else(|| format!("prop `{key}` 必须是非负整数")),
        Some(Value::String(text)) => text
            .trim()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| format!("prop `{key}` 必须是非负整数")),
        Some(_) => Err(format!("prop `{key}` 必须是非负整数")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Instant;

    const PAGE: &str = r#"
type Item = { label: string; value: number };

function Row(props: { item: Item; index: number }) {
  return (
    <li class="row">
      <span class="muted">{props.index + 1}. {props.item.label}</span>
      <Chip text={String(props.item.value)} tone="brand" />
    </li>
  );
}

function Page(props: { title: string; items: Item[] }) {
  return (
    <div class="card">
      <h1>{props.title}</h1>
      <ul>
        {props.items.map((item, index) => (
          <Row item={item} index={index} />
        ))}
      </ul>
      <Button label="刷新" variant="primary" size="sm" icon="i-refresh" />
      <Disclosure label="执行命令" detail="rm -rf build" badge="等待批准" badgeTone="warn">
        <Panel title="输出"><span>ok</span></Panel>
      </Disclosure>
    </div>
  );
}
"#;

    fn props() -> Value {
        json!({
            "title": "TSX spike <ok>",
            "items": [
                { "label": "甲", "value": 1 },
                { "label": "乙", "value": 2 }
            ]
        })
    }

    #[test]
    fn renders_tsx_with_loops_composites_and_rust_components() {
        let html = render_tsx(PAGE, &props()).expect("TSX 应当渲染成功");
        println!("\n{html}\n");
        assert!(html.contains("class=\"card\""));
        assert!(html.contains("TSX spike &lt;ok&gt;"));
        assert!(html.contains("1. 甲"));
        assert!(html.contains("2. 乙"));
        assert!(html.contains("ada-chip--brand"));
        assert!(html.contains("ada-btn--primary"));
        assert!(html.contains("ada-btn__label\">刷新"));
        assert!(html.contains("ada-disclosure"));
        assert!(html.contains("等待批准"));
        assert!(html.contains("ada-panel"));
        assert!(html.contains("<span>ok</span>"));
        assert!(!html.contains("<Button"));
        assert!(!html.contains("<Chip"));
    }

    #[test]
    fn escapes_text_and_rejects_javascript_urls() {
        let source = r#"
function Page() {
  return <div class="x"><a href="javascript:alert(1)">点我</a></div>;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(error.contains("URL"), "{error}");
    }

    #[test]
    fn rejects_event_props_instead_of_silently_dropping_them() {
        let source = r#"
function Page() {
  return <div onClick={() => 1}>x</div>;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(
            error.contains("函数 prop") || error.contains("事件 prop"),
            "{error}"
        );
    }

    #[test]
    fn unknown_rust_component_is_an_error() {
        let source = r#"
function Page() {
  return <Nope label="x" />;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(
            error.contains("Nope") || error.contains("执行失败"),
            "{error}"
        );
    }

    #[test]
    fn renders_form_and_data_driven_components() {
        let source = r#"
function Page() {
  return (
    <Form action="/providers/x" method="post" confirm="保存吗？">
      <TextField name="title" placeholder="标题" value="深度" />
      <Select name="protocol" value="openai_chat" options={[
        { value: "openai_chat", label: "OpenAI Chat" },
        { value: "anthropic", label: "Anthropic" }
      ]} />
      <Todo steps={[{ id: "1", title: "甲", status: "completed" }, { id: "2", title: "乙" }]} notes="备注" />
      <Icon name="i-check" size={18} />
      <IconButton icon="i-x" label="关闭" href="/" />
      <Modal id="m1" title="弹窗"><p>body</p></Modal>
      <EmptyState title="空" description="没有内容" />
      <UserMessage text="你好" images={2} />
      <AssistantMessage><p>回答</p></AssistantMessage>
      <Button label="保存" kind="submit" />
    </Form>
  );
}
"#;
        let html = render_tsx(source, &Value::Null).expect("form render");
        assert!(html.starts_with("<form"), "{html}");
        assert!(html.contains("name=\"title\""), "{html}");
        assert!(html.contains("data-select-input"), "{html}");
        assert!(html.contains("ada-todo"), "{html}");
        assert!(html.contains("ada-modal"), "{html}");
        assert!(html.contains("ada-empty"), "{html}");
        assert!(html.contains("含 2 张图片"), "{html}");
        assert!(html.ends_with("</form>"), "{html}");
    }

    #[test]
    fn rejects_inline_script_attributes() {
        let source = r#"
function Page() {
  return <div hx-on:click="alert(1)">x</div>;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(error.contains("hx-on"), "{error}");
    }

    #[test]
    fn fragment_is_flattened() {
        let source = r#"
function Page() {
  return <><span>a</span><span>b</span></>;
}
"#;
        let html = render_tsx(source, &Value::Null).expect("fragment render");
        assert_eq!(html, "<span>a</span><span>b</span>");
    }

    #[test]
    fn fresh_context_isolates_module_state_between_renders() {
        let source = r#"
let hits = 0;
function Page() {
  hits += 1;
  return <div>{hits}</div>;
}
"#;
        let first = render_tsx(source, &Value::Null).expect("first render");
        let second = render_tsx(source, &Value::Null).expect("second render");
        assert!(first.contains(">1<"), "{first}");
        assert!(second.contains(">1<"), "{second}");
    }

    #[test]
    fn single_file_compile_rejects_module_syntax() {
        let source = r#"
import { label } from "./labels.ts";
function Page() {
  return <div>{label}</div>;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(!error.is_empty(), "单文件编译遇到 import 必须明确失败");
        println!("import error: {error}");
    }

    #[test]
    fn unknown_component_prop_is_rejected() {
        let source = r#"
function Page() {
  return <Button label="x" typo="y" />;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(error.contains("typo"), "{error}");
    }

    #[test]
    fn boa_loop_budget_stops_runaway_page() {
        let source = r#"
function Page() {
  while (true) {}
  return <div />;
}
"#;
        let error = render_tsx(source, &Value::Null).unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn render_latency_snapshot() {
        const N: u32 = 50;
        let props = props();

        let started = Instant::now();
        for _ in 0..N {
            render_tsx(PAGE, &props).expect("render");
        }
        let total = started.elapsed();

        let started = Instant::now();
        let mut js = String::new();
        for _ in 0..N {
            js = transform_tsx(PAGE, "spike.tsx").expect("transform");
        }
        let transform_time = started.elapsed();

        let started = Instant::now();
        let mut vnode = Value::Null;
        for _ in 0..N {
            vnode = evaluate_page(&js, "Page", &props).expect("eval");
        }
        let eval_time = started.elapsed();

        let started = Instant::now();
        for _ in 0..N {
            let mut out = String::new();
            render_node(&vnode, &mut out, 0).expect("render");
        }
        let render_time = started.elapsed();

        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        println!(
            "\n{N} iterations ({profile}): total avg {:?}; transform avg {:?}; boa eval avg {:?}; rust render avg {:?}\n",
            total / N,
            transform_time / N,
            eval_time / N,
            render_time / N,
        );
    }

    #[test]
    fn render_latency_reused_boa_context() {
        const N: u32 = 50;
        let props = props();
        let js = transform_tsx(PAGE, "spike.tsx").expect("transform");
        let props_json = serde_json::to_string(&props).expect("props");
        let mut context = Context::default();
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(MAX_SCRIPT_LOOP);
        context.eval(Source::from_bytes(PRELUDE)).expect("prelude");
        install_component_library(&mut context).expect("library");
        let started = Instant::now();
        for _ in 0..N {
            let program = format!("{js}\n;JSON.stringify(h(Page, {props_json}));");
            let value = context.eval(Source::from_bytes(&program)).expect("eval");
            let text = value
                .to_string(&mut context)
                .expect("to string")
                .to_std_string_escaped();
            let vnode: Value = serde_json::from_str(&text).expect("vnode");
            let mut out = String::new();
            render_node(&vnode, &mut out, 0).expect("render");
            assert!(out.contains("ada-btn--primary"));
        }
        let elapsed = started.elapsed();
        println!(
            "\n{N} iterations with a reused Boa context: avg {:?}\n",
            elapsed / N
        );
    }

    #[test]
    fn module_render_latency_snapshot() {
        const N: u32 = 50;
        let root = std::env::temp_dir().join(format!("ssr_h_module_lat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("label.ts"),
            "export const label = \"来自 label.ts\";",
        )
        .unwrap();
        std::fs::write(
            root.join("page.tsx"),
            r#"
import { label } from "./label";
export function Page() {
  return <div><span>{label}</span><Chip text="module" tone="brand" /></div>;
}
"#,
        )
        .unwrap();
        let compiled = compile_file(&root.join("page.tsx"), &root).expect("compile module");
        let props = serde_json::json!({});
        for _ in 0..5 {
            render(&compiled, &props).expect("warm module");
        }
        let started = Instant::now();
        for _ in 0..N {
            let html = render(&compiled, &props).expect("render module");
            assert!(html.contains("ada-chip--brand"), "{html}");
        }
        let elapsed = started.elapsed();
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        println!(
            "\n{N} module iterations ({profile}, OXC precompiled): avg {:?}\n",
            elapsed / N
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
