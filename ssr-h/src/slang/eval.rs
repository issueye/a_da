//! Slang 2 的编译与渲染（精简版）。
//!
//! 编译：解析 → 展开 `@use`/`@include` → 编译模块脚本/`@props` → 校验组件契约。
//! 渲染：每次请求创建作用域链（root → module → instance → block → call）；
//! 组件是“模板 + props”，插槽是 thunk，模块函数是纯函数；渲染期不做任何 IO。
//!
//! 没有 `@script setup`、没有闭包/ref/computed/事件：交互由浏览器里的 Alpine 负责。

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ast::*;
use super::component::{
    Args, ComponentDef, ComponentRegistry, ComponentSpec, CompositeDef, ElementDef, RenderedSlots,
};
use super::error::{err, Diagnostic, SlangResult, Span};
use super::html;
use super::native;
use super::parser::TextParser;
use super::runtime::{AssignError, Binding, BindingKind, BudgetState, Env, SlotThunk};
use super::script::{compile_script, parse_props_decl, CompiledScript, BUILTIN_NAMES, ROOT_NAMES};
use super::value::Value;

const MAX_SOURCE_BYTES: usize = 512 * 1024;

fn attach_file(mut error: Diagnostic, file: &Option<PathBuf>) -> Diagnostic {
    if error.file.is_none() {
        error.file = file.clone();
    }
    error
}

fn count_program_nodes(program: &Program) -> usize {
    let mut count = count_nodes(&program.nodes);
    for def in &program.defs {
        count += count_nodes(&def.body);
    }
    count
}

fn count_nodes(nodes: &[Node]) -> usize {
    let mut count = 0usize;
    for node in nodes {
        count += 1;
        match &node.kind {
            NodeKind::Component(call) => count += count_nodes(&call.children),
            NodeKind::If { arms, otherwise } => {
                for (_, body) in arms {
                    count += count_nodes(body);
                }
                count += count_nodes(otherwise);
            }
            NodeKind::For {
                body, otherwise, ..
            } => {
                count += count_nodes(body);
                count += count_nodes(otherwise);
            }
            NodeKind::Html { body, .. } => {
                if let Some(body) = body {
                    count += count_nodes(body);
                }
            }
            NodeKind::Slot { fallback, .. } => count += count_nodes(fallback),
            NodeKind::Fill { body, .. } => count += count_nodes(body),
            NodeKind::Include { body, .. } => count += count_nodes(body),
            _ => {}
        }
    }
    count
}

// ─────────────────────────────────────────────────────────────────────────────
// 上下文
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trust {
    Core,
    #[default]
    Local,
    Sandboxed,
}

#[derive(Debug, Clone)]
pub struct Budgets {
    pub max_output_bytes: usize,
    pub max_loop_iterations: usize,
    pub max_depth: usize,
    pub max_component_depth: usize,
    /// 函数调用深度（具名函数递归）。
    pub max_call_depth: usize,
    /// 脚本语句步数上限。
    pub max_script_steps: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_output_bytes: 1024 * 1024,
            max_loop_iterations: 10_000,
            max_depth: 64,
            max_component_depth: 32,
            max_call_depth: 64,
            max_script_steps: 100_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct PageMeta {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone, Default)]
pub struct SlotMeta {
    pub name: String,
    pub order: i32,
    pub index: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AppMeta {
    pub workspace: String,
    pub gateway: String,
    pub product: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct RouteMeta {
    pub path: String,
    pub query: String,
}

/// 渲染上下文：只暴露显式投影，不把整个 `AppState` 丢给模板。
#[derive(Debug, Clone, Default)]
pub struct RenderContext {
    pub plugin: Option<PluginMeta>,
    pub page: Option<PageMeta>,
    pub slot: Option<SlotMeta>,
    pub app: AppMeta,
    pub route: RouteMeta,
    pub data: Value,
    pub trust: Trust,
    pub budgets: Budgets,
}

impl RenderContext {
    pub fn new(app: AppMeta) -> Self {
        Self {
            app,
            trust: Trust::Local,
            budgets: Budgets::default(),
            ..Default::default()
        }
    }

    pub fn with_plugin(mut self, plugin: PluginMeta) -> Self {
        self.plugin = Some(plugin);
        self
    }

    pub fn with_page(mut self, page: PageMeta) -> Self {
        self.page = Some(page);
        self
    }

    pub fn with_slot(mut self, slot: SlotMeta) -> Self {
        self.slot = Some(slot);
        self
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn with_trust(mut self, trust: Trust) -> Self {
        self.trust = trust;
        self
    }

    pub fn with_route_path(mut self, path: impl Into<String>) -> Self {
        self.route.path = path.into();
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 源码加载
// ─────────────────────────────────────────────────────────────────────────────

pub trait Loader {
    fn load(&self, path: &Path) -> SlangResult<String>;
    fn canonicalize(&self, path: &Path) -> SlangResult<PathBuf>;
}

/// 不加载任何外部文件；用于 `compile_str`。
pub struct NoopLoader;

impl Loader for NoopLoader {
    fn load(&self, path: &Path) -> SlangResult<String> {
        err(
            "E5001",
            format!("这个模板不允许 @include/@use：{}", path.display()),
            Span::default(),
        )
    }

    fn canonicalize(&self, path: &Path) -> SlangResult<PathBuf> {
        Ok(path.to_path_buf())
    }
}

/// 文件系统加载器：只允许插件根目录内的文件。
pub struct FsLoader {
    pub root: PathBuf,
}

impl Loader for FsLoader {
    fn load(&self, path: &Path) -> SlangResult<String> {
        let meta = std::fs::metadata(path).map_err(|error| {
            Diagnostic::error(
                "E5001",
                format!("读取 {} 失败：{error}", path.display()),
                Span::default(),
            )
        })?;
        if meta.len() as usize > MAX_SOURCE_BYTES {
            return err(
                "E5001",
                format!(
                    "{} 超过 {} KB 上限",
                    path.display(),
                    MAX_SOURCE_BYTES / 1024
                ),
                Span::default(),
            );
        }
        std::fs::read_to_string(path).map_err(|error| {
            Diagnostic::error(
                "E5001",
                format!("读取 {} 失败：{error}", path.display()),
                Span::default(),
            )
        })
    }

    fn canonicalize(&self, path: &Path) -> SlangResult<PathBuf> {
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            Diagnostic::error(
                "E5001",
                format!("{} 不存在或不可读：{error}", path.display()),
                Span::default(),
            )
        })?;
        if !canonical.starts_with(&self.root) {
            return err(
                "E5001",
                format!("路径越界：{}", path.display()),
                Span::default(),
            );
        }
        Ok(canonical)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 编译产物与引擎
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    pub program: Program,
    pub defs: HashMap<String, Arc<CompositeDef>>,
    /// 模块脚本（`@script`），每次渲染执行一次。
    pub module: Arc<CompiledScript>,
    pub dependencies: Vec<PathBuf>,
    pub path: Option<PathBuf>,
    pub source: Arc<str>,
}

#[derive(Debug, Clone, Default)]
pub struct Engine {
    registry: ComponentRegistry,
}

impl Engine {
    pub fn new(registry: ComponentRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    pub fn compile_str(&self, source: &str) -> SlangResult<Arc<CompiledTemplate>> {
        self.compile(source, None, None, &NoopLoader)
    }

    pub fn compile_file(
        &self,
        path: &Path,
        root: &Path,
        _trust: Trust,
    ) -> SlangResult<Arc<CompiledTemplate>> {
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            Diagnostic::error(
                "E5001",
                format!("{} 不存在或不可读：{error}", path.display()),
                Span::default(),
            )
        })?;
        if !canonical.starts_with(root) {
            return err(
                "E5001",
                format!("路径越界：{}", path.display()),
                Span::default(),
            );
        }
        let loader = FsLoader {
            root: root.to_path_buf(),
        };
        let source = loader.load(&canonical)?;
        self.compile(&source, Some(canonical), Some(root.to_path_buf()), &loader)
    }

    pub fn compile(
        &self,
        source: &str,
        path: Option<PathBuf>,
        root: Option<PathBuf>,
        loader: &dyn Loader,
    ) -> SlangResult<Arc<CompiledTemplate>> {
        if source.len() > MAX_SOURCE_BYTES {
            return err(
                "E1001",
                format!("源文件超过 {} KB 上限", MAX_SOURCE_BYTES / 1024),
                Span::default(),
            );
        }
        let mut parser = TextParser::new(source);
        if let Some(path) = &path {
            parser = parser.with_file(path.clone());
        }
        let program = parser
            .parse_program()
            .map_err(|error| attach_file(error, &path))?;

        let mut resolver = Resolver::new(loader, root, path.clone());
        let program = resolver
            .expand_program(program, path.as_deref())
            .map_err(|error| attach_file(error, &path))?;
        let node_count = count_program_nodes(&program);
        if node_count > 10_000 {
            return err(
                "E6001",
                format!("AST 节点数 {node_count} 超过 10,000 上限"),
                Span::default(),
            );
        }

        let module = Arc::new(
            compile_script(&program.module_scripts).map_err(|error| attach_file(error, &path))?,
        );

        let mut defs: HashMap<String, Arc<CompositeDef>> = HashMap::new();
        for def in &program.defs {
            if !def.name.chars().next().is_some_and(|ch| ch.is_uppercase()) {
                return err(
                    "E2001",
                    format!(
                        "组合组件名 `{}` 必须首字母大写（避免和内置 HTML 元素混淆）",
                        def.name
                    ),
                    def.span,
                );
            }
            if defs.contains_key(&def.name) {
                return err("E2001", format!("组件 `{}` 重复定义", def.name), def.span);
            }
            let props = match &def.props {
                Some(expr) => parse_props_decl(expr).map_err(|error| attach_file(error, &path))?,
                None => Default::default(),
            };
            defs.insert(
                def.name.clone(),
                Arc::new(CompositeDef {
                    name: def.name.clone(),
                    props,
                    body: def.body.clone(),
                    span: def.span,
                    source: path.clone(),
                }),
            );
        }

        validate_program(&program, &defs, &self.registry)
            .map_err(|error| attach_file(error, &path))?;
        detect_component_cycles(&defs).map_err(|error| attach_file(error, &path))?;

        Ok(Arc::new(CompiledTemplate {
            program,
            defs,
            module,
            dependencies: resolver.dependencies,
            path,
            source: Arc::from(source),
        }))
    }

    pub fn render(&self, template: &CompiledTemplate, cx: &RenderContext) -> SlangResult<String> {
        Renderer::new(self, template, cx).render()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// @use / @include 展开
// ─────────────────────────────────────────────────────────────────────────────

struct Resolver<'a> {
    loader: &'a dyn Loader,
    root: Option<PathBuf>,
    stack: Vec<PathBuf>,
    dependencies: Vec<PathBuf>,
}

impl<'a> Resolver<'a> {
    fn new(loader: &'a dyn Loader, root: Option<PathBuf>, current: Option<PathBuf>) -> Self {
        Self {
            loader,
            root,
            stack: current.into_iter().collect(),
            dependencies: Vec::new(),
        }
    }

    fn expand_program(&mut self, program: Program, current: Option<&Path>) -> SlangResult<Program> {
        let (nodes, mut defs, mut module_scripts) = self.expand_nodes(program.nodes, current)?;
        for mut def in program.defs {
            let (body, nested_defs, nested_scripts) = self.expand_nodes(def.body, current)?;
            def.body = body;
            defs.extend(nested_defs);
            module_scripts.extend(nested_scripts);
            defs.push(def);
        }
        module_scripts.extend(program.module_scripts);
        Ok(Program {
            version: program.version,
            nodes,
            defs,
            module_scripts,
        })
    }

    fn expand_nodes(
        &mut self,
        nodes: Vec<Node>,
        current: Option<&Path>,
    ) -> SlangResult<(Vec<Node>, Vec<Definition>, Vec<ScriptBlock>)> {
        let mut out = Vec::new();
        let mut defs = Vec::new();
        let mut scripts = Vec::new();
        for node in nodes {
            match node.kind {
                NodeKind::Use { path } => {
                    // @use 只导入定义与模块脚本；它的内容不渲染。
                    let included = self.load_module(&path, current, node.span)?;
                    defs.extend(included.defs);
                    scripts.extend(included.module_scripts);
                }
                NodeKind::Include { path, .. } => {
                    let included = self.load_module(&path, current, node.span)?;
                    defs.extend(included.defs);
                    scripts.extend(included.module_scripts);
                    out.extend(included.nodes);
                }
                NodeKind::Component(mut call) => {
                    let (children, child_defs, child_scripts) =
                        self.expand_nodes(call.children, current)?;
                    call.children = children;
                    defs.extend(child_defs);
                    scripts.extend(child_scripts);
                    out.push(Node {
                        kind: NodeKind::Component(call),
                        span: node.span,
                    });
                }
                NodeKind::If { arms, otherwise } => {
                    let mut new_arms = Vec::new();
                    for (condition, body) in arms {
                        let (body, body_defs, body_scripts) = self.expand_nodes(body, current)?;
                        defs.extend(body_defs);
                        scripts.extend(body_scripts);
                        new_arms.push((condition, body));
                    }
                    let (otherwise, otherwise_defs, otherwise_scripts) =
                        self.expand_nodes(otherwise, current)?;
                    defs.extend(otherwise_defs);
                    scripts.extend(otherwise_scripts);
                    out.push(Node {
                        kind: NodeKind::If {
                            arms: new_arms,
                            otherwise,
                        },
                        span: node.span,
                    });
                }
                NodeKind::For {
                    item,
                    index,
                    iter,
                    body,
                    otherwise,
                } => {
                    let (body, body_defs, body_scripts) = self.expand_nodes(body, current)?;
                    let (otherwise, otherwise_defs, otherwise_scripts) =
                        self.expand_nodes(otherwise, current)?;
                    defs.extend(body_defs);
                    defs.extend(otherwise_defs);
                    scripts.extend(body_scripts);
                    scripts.extend(otherwise_scripts);
                    out.push(Node {
                        kind: NodeKind::For {
                            item,
                            index,
                            iter,
                            body,
                            otherwise,
                        },
                        span: node.span,
                    });
                }
                NodeKind::Html { body, expr } => {
                    let body = match body {
                        Some(body) => {
                            let (body, body_defs, body_scripts) =
                                self.expand_nodes(body, current)?;
                            defs.extend(body_defs);
                            scripts.extend(body_scripts);
                            Some(body)
                        }
                        None => None,
                    };
                    out.push(Node {
                        kind: NodeKind::Html { body, expr },
                        span: node.span,
                    });
                }
                NodeKind::Slot {
                    name,
                    params,
                    required,
                    fallback,
                } => {
                    let (fallback, fallback_defs, fallback_scripts) =
                        self.expand_nodes(fallback, current)?;
                    defs.extend(fallback_defs);
                    scripts.extend(fallback_scripts);
                    out.push(Node {
                        kind: NodeKind::Slot {
                            name,
                            params,
                            required,
                            fallback,
                        },
                        span: node.span,
                    });
                }
                NodeKind::Fill { name, params, body } => {
                    let (body, body_defs, body_scripts) = self.expand_nodes(body, current)?;
                    defs.extend(body_defs);
                    scripts.extend(body_scripts);
                    out.push(Node {
                        kind: NodeKind::Fill { name, params, body },
                        span: node.span,
                    });
                }
                other => out.push(Node {
                    kind: other,
                    span: node.span,
                }),
            }
        }
        Ok((out, defs, scripts))
    }

    fn load_module(
        &mut self,
        relative: &str,
        current: Option<&Path>,
        span: Span,
    ) -> SlangResult<Program> {
        let base = current
            .and_then(|path| path.parent())
            .or(self.root.as_deref())
            .unwrap_or_else(|| Path::new("."));
        let joined = base.join(relative);
        let canonical = self.loader.canonicalize(&joined).map_err(|error| {
            Diagnostic::error(
                "E5001",
                format!("@include/@use 路径不可用 `{relative}`：{}", error.message),
                span,
            )
        })?;
        if let Some(root) = &self.root {
            if !canonical.starts_with(root) {
                return err("E5001", format!("@include/@use 路径越界：{relative}"), span);
            }
        }
        if self.stack.len() > 16 {
            return err("E6001", "@include/@use 嵌套超过 16 层".to_string(), span);
        }
        if self.stack.iter().any(|path| path == &canonical) {
            return err(
                "E5002",
                format!("@include/@use 循环：{}", canonical.display()),
                span,
            );
        }
        let source = self.loader.load(&canonical)?;
        self.stack.push(canonical.clone());
        self.dependencies.push(canonical.clone());
        let parser = TextParser::new(&source).with_file(canonical.clone());
        let program = parser
            .parse_program()
            .map_err(|error| attach_file(error, &Some(canonical.clone())))?;
        let expanded = self.expand_program(program, Some(&canonical));
        self.stack.pop();
        expanded
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 编译期校验
// ─────────────────────────────────────────────────────────────────────────────

fn validate_program(
    program: &Program,
    defs: &HashMap<String, Arc<CompositeDef>>,
    registry: &ComponentRegistry,
) -> SlangResult<()> {
    let validator = Validator { defs, registry };
    validator.validate_nodes(&program.nodes, false, false)?;
    for def in defs.values() {
        validator.validate_nodes(&def.body, true, false)?;
    }
    Ok(())
}

struct Validator<'a> {
    defs: &'a HashMap<String, Arc<CompositeDef>>,
    registry: &'a ComponentRegistry,
}

impl<'a> Validator<'a> {
    fn validate_nodes(&self, nodes: &[Node], in_def: bool, in_component: bool) -> SlangResult<()> {
        for node in nodes {
            match &node.kind {
                NodeKind::Component(call) => {
                    let def = self.resolve(&call.name).ok_or_else(|| {
                        let hint = self
                            .suggest(&call.name)
                            .map(|name| format!("是不是 `{name}`？"))
                            .unwrap_or_else(|| "可用组件见 docs/SLANG-V2.md".to_string());
                        Diagnostic::error("E3001", format!("未知组件 `{}`", call.name), call.span)
                            .with_hint(hint)
                    })?;
                    match def.as_ref() {
                        ComponentDef::Native(native) => {
                            self.validate_native_call(&native.spec, call)?;
                        }
                        ComponentDef::Composite(composite) => {
                            self.validate_composite_call(composite, call)?;
                        }
                        ComponentDef::Element(element) => {
                            self.validate_element_call(element, call)?;
                        }
                    }
                    self.validate_nodes(&call.children, in_def, true)?;
                }
                NodeKind::If { arms, otherwise } => {
                    for (_, body) in arms {
                        self.validate_nodes(body, in_def, in_component)?;
                    }
                    self.validate_nodes(otherwise, in_def, in_component)?;
                }
                NodeKind::For {
                    body, otherwise, ..
                } => {
                    self.validate_nodes(body, in_def, in_component)?;
                    self.validate_nodes(otherwise, in_def, in_component)?;
                }
                NodeKind::Html { body, .. } => {
                    if let Some(body) = body {
                        self.validate_nodes(body, in_def, in_component)?;
                    }
                }
                NodeKind::Slot {
                    name,
                    required: _,
                    fallback,
                    ..
                } => {
                    if !in_def {
                        return err(
                            "E2001",
                            format!("`@slot {name}` 只能出现在 `@def` 里"),
                            node.span,
                        );
                    }
                    self.validate_nodes(fallback, in_def, in_component)?;
                }
                NodeKind::Fill { name, params, body } => {
                    if !in_component {
                        return err(
                            "E2001",
                            format!("`@fill {name}` 只能出现在组件调用块里"),
                            node.span,
                        );
                    }
                    check_unique_names(params, "插槽参数", node.span)?;
                    self.validate_nodes(body, in_def, in_component)?;
                }
                NodeKind::Use { .. } => {
                    return err(
                        "E5001",
                        "`@use` 没有被展开（内部错误）".to_string(),
                        node.span,
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_native_call(&self, spec: &ComponentSpec, call: &ComponentCall) -> SlangResult<()> {
        let provided = provided_arg_names(call, &spec.positional)?;
        if call.args.iter().filter(|arg| arg.name.is_none()).count() > spec.positional.len() {
            return err(
                "E3002",
                format!(
                    "`{}` 最多接受 {} 个位置参数",
                    spec.name,
                    spec.positional.len()
                ),
                call.span,
            );
        }
        let mut named = HashSet::new();
        for arg in &call.args {
            if let Some(name) = &arg.name {
                if !named.insert(name.clone()) {
                    return err(
                        "E3002",
                        format!("`{}` 的属性 `{name}` 重复", spec.name),
                        arg.span,
                    );
                }
                if spec.prop_spec(name).is_none() {
                    return err(
                        "E3002",
                        format!("`{}` 不支持属性 `{name}`", spec.name),
                        arg.span,
                    );
                }
            }
        }
        for prop in &spec.props {
            if prop.required && !provided.contains(&prop.name) {
                return err(
                    "E3003",
                    format!("`{}` 缺少必填属性 `{}`", spec.name, prop.name),
                    call.span,
                );
            }
        }
        let fills = fill_names(call);
        for name in &fills {
            if spec.slot_spec(name).is_none() {
                return err(
                    "E3004",
                    format!("`{}` 没有插槽 `{name}`", spec.name),
                    call.span,
                );
            }
        }
        for child in &call.children {
            if let NodeKind::Fill { params, .. } = &child.kind {
                if !params.is_empty() {
                    return err(
                        "E3004",
                        format!("`{}` 是原生组件，不支持作用域插槽参数", spec.name),
                        child.span,
                    );
                }
            }
        }
        for slot in &spec.slots {
            if slot.required
                && !fills.contains(&slot.name)
                && !(spec.primary_slot == slot.name && has_plain_children(call))
            {
                return err(
                    "E3005",
                    format!("`{}` 缺少必填插槽 `{}`", spec.name, slot.name),
                    call.span,
                );
            }
        }
        if has_plain_children(call) && spec.slot_spec(&spec.primary_slot).is_none() {
            return err("E3004", format!("`{}` 不接受子节点", spec.name), call.span);
        }
        Ok(())
    }

    fn validate_composite_call(&self, def: &CompositeDef, call: &ComponentCall) -> SlangResult<()> {
        let positional = def.props.names();
        let mut provided: HashSet<String> = HashSet::new();
        let mut positional_index = 0usize;
        for arg in &call.args {
            let name = if let Some(name) = &arg.name {
                name.clone()
            } else {
                let Some(name) = positional.get(positional_index) else {
                    return err(
                        "E3002",
                        format!("`{}` 最多接受 {} 个位置参数", def.name, positional.len()),
                        arg.span,
                    );
                };
                positional_index += 1;
                name.clone()
            };
            if !provided.insert(name.clone()) {
                return err(
                    "E3002",
                    format!("`{}` 的参数 `{name}` 重复", def.name),
                    arg.span,
                );
            }
            if def.props.get(&name).is_none() {
                return err(
                    "E3002",
                    format!(
                        "`{}` 不支持参数 `{name}`（可用：{}）",
                        def.name,
                        if positional.is_empty() {
                            "无".to_string()
                        } else {
                            positional.join(" / ")
                        }
                    ),
                    arg.span,
                );
            }
        }
        for prop in &def.props.props {
            if prop.required && !provided.contains(&prop.name) {
                return err(
                    "E3003",
                    format!("`{}` 缺少必填参数 `{}`", def.name, prop.name),
                    call.span,
                );
            }
        }

        let slots = collect_slot_info(&def.body);
        let mut filled: HashSet<String> = HashSet::new();
        for child in &call.children {
            if let NodeKind::Fill { name, params, .. } = &child.kind {
                let Some(info) = slots.get(name) else {
                    return err(
                        "E3004",
                        format!("`{}` 没有插槽 `{name}`", def.name),
                        child.span,
                    );
                };
                if !filled.insert(name.clone()) {
                    return err(
                        "E3004",
                        format!("插槽 `{name}` 在 `{}` 里被填充了两次", def.name),
                        child.span,
                    );
                }
                if params.len() > info.params {
                    return err(
                        "E3004",
                        format!(
                            "插槽 `{name}` 只提供 {} 个上下文值，`@fill {name}(...)` 声明了 {} 个",
                            info.params,
                            params.len()
                        ),
                        child.span,
                    );
                }
            }
        }
        let primary = primary_slot(&slots);
        if has_plain_children(call) {
            let Some(primary) = primary else {
                return err(
                    "E3004",
                    format!(
                        "`{}` 没有默认插槽，不能直接放子节点（可用插槽：{}）",
                        def.name,
                        slot_list(&slots)
                    ),
                    call.span,
                );
            };
            let info = &slots[primary];
            if info.params > 0 {
                return err(
                    "E3004",
                    format!(
                        "`{}` 的默认插槽需要 {} 个上下文值；裸子节点不能接收，请用 `@fill {primary}(...)`",
                        def.name, info.params
                    ),
                    call.span,
                );
            }
            if filled.contains(primary) {
                return err(
                    "E2001",
                    format!("插槽 `{primary}` 同时被子节点和 @fill 填充"),
                    call.span,
                );
            }
        }
        for (name, info) in &slots {
            if info.required
                && !filled.contains(name)
                && !(primary == Some(name) && has_plain_children(call))
            {
                return err(
                    "E3005",
                    format!("`{}` 缺少必填插槽 `{name}`", def.name),
                    call.span,
                );
            }
        }
        Ok(())
    }

    fn validate_element_call(&self, element: &ElementDef, call: &ComponentCall) -> SlangResult<()> {
        let positional = call.args.iter().filter(|arg| arg.name.is_none()).count();
        if positional > 1 {
            return err(
                "E3002",
                format!("`::{}` 最多接受一个位置文本参数", element.tag),
                call.span,
            );
        }
        if positional == 1 && !call.children.is_empty() {
            return err(
                "E2001",
                format!("`::{}` 不能同时使用位置文本和子节点", element.tag),
                call.span,
            );
        }
        let mut present = Vec::new();
        for arg in &call.args {
            if let Some(name) = &arg.name {
                if !html::element_allows_attr(element, name) {
                    return err(
                        "E3008",
                        format!("`::{}` 不允许属性 `{name}`", element.tag),
                        arg.span,
                    );
                }
                present.push(name.clone());
            }
        }
        html::check_required(element, &present, call.span)?;
        for child in &call.children {
            if matches!(child.kind, NodeKind::Fill { .. }) {
                return err(
                    "E3004",
                    format!("`::{}` 不支持命名插槽", element.tag),
                    child.span,
                );
            }
        }
        Ok(())
    }

    fn resolve(&self, name: &str) -> Option<Arc<ComponentDef>> {
        self.defs
            .get(name)
            .cloned()
            .map(|def| ComponentDef::Composite((*def).clone()))
            .map(Arc::new)
            .or_else(|| self.registry.get(name))
    }

    fn suggest(&self, name: &str) -> Option<String> {
        let candidates = self
            .registry
            .names()
            .into_iter()
            .chain(self.defs.keys().map(String::as_str));
        candidates
            .filter_map(|candidate| {
                let distance = levenshtein(name, candidate);
                (distance <= 2 || candidate.to_ascii_lowercase() == name.to_ascii_lowercase())
                    .then_some((distance, candidate.to_string()))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, candidate)| candidate)
    }
}

fn check_unique_names(names: &[String], what: &str, span: Span) -> SlangResult<()> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return err("E2001", format!("{what} `{name}` 重复"), span);
        }
    }
    Ok(())
}

fn provided_arg_names<'a>(
    call: &'a ComponentCall,
    positional: &'a [String],
) -> SlangResult<HashSet<String>> {
    let mut provided = HashSet::new();
    let mut index = 0usize;
    for arg in &call.args {
        let name = if let Some(name) = &arg.name {
            name.clone()
        } else {
            let Some(name) = positional.get(index) else {
                return err("E3002", "位置参数过多".to_string(), arg.span);
            };
            index += 1;
            name.clone()
        };
        if !provided.insert(name.clone()) {
            return err("E3002", format!("参数 `{name}` 重复"), arg.span);
        }
    }
    Ok(provided)
}

fn fill_names(call: &ComponentCall) -> HashSet<String> {
    call.children
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Fill { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn has_plain_children(call: &ComponentCall) -> bool {
    call.children
        .iter()
        .any(|node| !matches!(node.kind, NodeKind::Fill { .. }) && !is_ignorable_node(node))
}

/// 只有空白文本的节点不算“内容”。
fn is_ignorable_node(node: &Node) -> bool {
    matches!(&node.kind, NodeKind::Text(text) if text.trim().is_empty())
}

#[derive(Debug, Clone)]
struct SlotInfo {
    required: bool,
    params: usize,
}

fn collect_slot_info(nodes: &[Node]) -> HashMap<String, SlotInfo> {
    let mut slots = HashMap::new();
    collect_slot_info_into(nodes, &mut slots);
    slots
}

fn collect_slot_info_into(nodes: &[Node], slots: &mut HashMap<String, SlotInfo>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Slot {
                name,
                params,
                required,
                ..
            } => {
                let entry = slots.entry(name.clone()).or_insert(SlotInfo {
                    required: false,
                    params: 0,
                });
                entry.required |= *required;
                entry.params = entry.params.max(params.len());
            }
            NodeKind::If { arms, otherwise } => {
                for (_, body) in arms {
                    collect_slot_info_into(body, slots);
                }
                collect_slot_info_into(otherwise, slots);
            }
            NodeKind::For {
                body, otherwise, ..
            } => {
                collect_slot_info_into(body, slots);
                collect_slot_info_into(otherwise, slots);
            }
            NodeKind::Html { body, .. } => {
                if let Some(body) = body {
                    collect_slot_info_into(body, slots);
                }
            }
            // 插槽通常嵌在 `::div` / `::ul` 这类元素里；元素也是 ComponentCall。
            NodeKind::Component(call) => collect_slot_info_into(&call.children, slots),
            _ => {}
        }
    }
}

/// 组合组件的默认插槽名固定为 `default`。
fn primary_slot(slots: &HashMap<String, SlotInfo>) -> Option<&'static str> {
    if slots.contains_key("default") {
        Some("default")
    } else {
        None
    }
}

fn slot_list(slots: &HashMap<String, SlotInfo>) -> String {
    let mut names: Vec<&str> = slots.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() {
        "无".to_string()
    } else {
        names.join(" / ")
    }
}

fn detect_component_cycles(defs: &HashMap<String, Arc<CompositeDef>>) -> SlangResult<()> {
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for name in defs.keys() {
        detect_cycle(name, defs, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn detect_cycle(
    name: &str,
    defs: &HashMap<String, Arc<CompositeDef>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> SlangResult<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if !visiting.insert(name.to_string()) {
        return err(
            "E5003",
            format!("组件定义循环：{name} → … → {name}"),
            Span::default(),
        );
    }
    if let Some(def) = defs.get(name) {
        let mut called = HashSet::new();
        collect_called_composites(&def.body, defs, &mut called);
        for child in called {
            detect_cycle(&child, defs, visiting, visited)?;
        }
    }
    visiting.remove(name);
    visited.insert(name.to_string());
    Ok(())
}

fn collect_called_composites(
    nodes: &[Node],
    defs: &HashMap<String, Arc<CompositeDef>>,
    out: &mut HashSet<String>,
) {
    for node in nodes {
        match &node.kind {
            NodeKind::Component(call) => {
                if defs.contains_key(&call.name) {
                    out.insert(call.name.clone());
                }
                collect_called_composites(&call.children, defs, out);
            }
            NodeKind::If { arms, otherwise } => {
                for (_, body) in arms {
                    collect_called_composites(body, defs, out);
                }
                collect_called_composites(otherwise, defs, out);
            }
            NodeKind::For {
                body, otherwise, ..
            } => {
                collect_called_composites(body, defs, out);
                collect_called_composites(otherwise, defs, out);
            }
            NodeKind::Html { body, .. } => {
                if let Some(body) = body {
                    collect_called_composites(body, defs, out);
                }
            }
            NodeKind::Slot { fallback, .. } => {
                collect_called_composites(fallback, defs, out);
            }
            NodeKind::Fill { body, .. } => {
                collect_called_composites(body, defs, out);
            }
            _ => {}
        }
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

// ─────────────────────────────────────────────────────────────────────────────
// 渲染
// ─────────────────────────────────────────────────────────────────────────────

pub type RenderCx<'a> = Renderer<'a>;

#[derive(Debug)]
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

struct InstanceFrame {
    name: String,
    slots: BTreeMap<String, SlotThunk>,
}

pub struct Renderer<'a> {
    engine: &'a Engine,
    template: &'a CompiledTemplate,
    cx: &'a RenderContext,
    module_env: Arc<Env>,
    env: Arc<Env>,
    instances: Vec<InstanceFrame>,
    budgets: BudgetState,
    module_running: bool,
}

impl<'a> Renderer<'a> {
    fn new(engine: &'a Engine, template: &'a CompiledTemplate, cx: &'a RenderContext) -> Self {
        let root = build_roots(cx);
        let module_env = Env::child(&root);
        Self {
            engine,
            template,
            cx,
            env: module_env.clone(),
            module_env,
            instances: Vec::new(),
            budgets: BudgetState::new(),
            module_running: false,
        }
    }

    fn render(mut self) -> SlangResult<String> {
        // 1. 模块脚本：一次渲染一次。
        self.module_running = true;
        let mut result = self.exec_script_top(&self.template.module.statements);
        self.module_running = false;
        if let Err(error) = result {
            return Err(error);
        }

        // 2. 文件级组件：页面/插槽模板。
        self.instances.push(InstanceFrame {
            name: "<page>".to_string(),
            slots: BTreeMap::new(),
        });
        let instance_env = Env::child(&self.module_env);
        instance_env.define(
            "props",
            Binding::readonly(Value::Map(BTreeMap::new()), BindingKind::System),
        );
        instance_env.define(
            "slots",
            Binding::readonly(Value::Map(BTreeMap::new()), BindingKind::System),
        );
        self.env = instance_env;

        let mut out = String::new();
        if result.is_ok() {
            result = self.render_nodes(&self.template.program.nodes, &mut out, false, 0);
        }
        self.instances.pop();
        result.map(|_| out)
    }

    // ── 作用域与执行 ─────────────────────────────────────────────────────────

    fn exec_script_top(&mut self, statements: &[ScriptStmt]) -> SlangResult<()> {
        match self.exec_block(statements)? {
            Flow::Normal => Ok(()),
            _ => err(
                "E7101",
                "模块脚本顶层不能 `return` / `break` / `continue`",
                Span::default(),
            ),
        }
    }

    fn exec_block(&mut self, statements: &[ScriptStmt]) -> SlangResult<Flow> {
        for statement in statements {
            match self.exec_stmt(statement)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_scoped_block(&mut self, statements: &[ScriptStmt]) -> SlangResult<Flow> {
        let saved = self.env.clone();
        self.env = Env::child(&saved);
        let result = self.exec_block(statements);
        self.env = saved;
        result
    }

    fn exec_stmt(&mut self, statement: &ScriptStmt) -> SlangResult<Flow> {
        self.budgets.script_steps += 1;
        if self.budgets.script_steps > self.cx.budgets.max_script_steps {
            return err(
                "E7103",
                format!("脚本步数超过 {}", self.cx.budgets.max_script_steps),
                statement_span(statement),
            );
        }
        match statement {
            ScriptStmt::Let {
                name,
                mutable,
                value,
                ..
            } => {
                let value = self.eval(value)?;
                // 模块脚本顶层的 `let` 是 Module 绑定（跨组件共享、只读）；
                // 函数/块里的 `let` 是局部绑定。
                let kind = if self.module_running && Arc::ptr_eq(&self.env, &self.module_env) {
                    BindingKind::Module
                } else {
                    BindingKind::Local
                };
                self.env
                    .define(name.clone(), Binding::new(value, *mutable, kind));
                Ok(Flow::Normal)
            }
            ScriptStmt::Assign {
                target,
                op,
                value,
                span,
            } => {
                let value = match op {
                    AssignOp::Set => self.eval(value)?,
                    other => {
                        let current = self.eval(target)?;
                        let right = self.eval(value)?;
                        apply_assign_op(*other, current, right, *span)?
                    }
                };
                self.assign_target(target, value, *span)?;
                Ok(Flow::Normal)
            }
            ScriptStmt::If {
                arms, otherwise, ..
            } => {
                for (condition, body) in arms {
                    if self.eval(condition)?.is_truthy() {
                        return self.exec_scoped_block(body);
                    }
                }
                self.exec_scoped_block(otherwise)
            }
            ScriptStmt::For {
                item,
                index,
                iter,
                body,
                span,
            } => {
                let entries = iteration_entries(self.eval(iter)?, *span)?;
                for (position, entry) in entries.into_iter().enumerate() {
                    self.budgets.loop_iterations += 1;
                    if self.budgets.loop_iterations > self.cx.budgets.max_loop_iterations {
                        return err(
                            "E7103",
                            format!("循环迭代超过 {} 次", self.cx.budgets.max_loop_iterations),
                            *span,
                        );
                    }
                    let saved = self.env.clone();
                    self.env = Env::child(&saved);
                    self.env
                        .define(item.clone(), Binding::readonly(entry, BindingKind::Local));
                    if let Some(index) = index {
                        self.env.define(
                            index.clone(),
                            Binding::readonly(Value::Int(position as i64), BindingKind::Local),
                        );
                    }
                    let flow = self.exec_block(body);
                    self.env = saved;
                    match flow? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                    }
                }
                Ok(Flow::Normal)
            }
            ScriptStmt::Return { value, .. } => {
                let value = match value {
                    Some(value) => self.eval(value)?,
                    None => Value::Null,
                };
                Ok(Flow::Return(value))
            }
            ScriptStmt::Break(_) => Ok(Flow::Break),
            ScriptStmt::Continue(_) => Ok(Flow::Continue),
            // 函数声明已编译进 module.functions；执行期跳过。
            ScriptStmt::Function(_) => Ok(Flow::Normal),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>, span: Span) -> SlangResult<Value> {
        let Some(function) = self.template.module.functions.get(name).cloned() else {
            return err("E7102", format!("未知函数 `{name}`"), span);
        };
        if args.len() != function.params.len() {
            return err(
                "E7102",
                format!(
                    "函数 `{name}` 需要 {} 个参数，得到 {}",
                    function.params.len(),
                    args.len()
                ),
                span,
            );
        }
        if self.budgets.call_depth >= self.cx.budgets.max_call_depth {
            return err(
                "E7103",
                format!("函数调用深度超过 {}", self.cx.budgets.max_call_depth),
                span,
            );
        }
        self.budgets.call_depth += 1;
        let saved_env = self.env.clone();
        let call_env = Env::child(&self.module_env);
        for (param, value) in function.params.iter().zip(args) {
            call_env.define(param.clone(), Binding::readonly(value, BindingKind::Param));
        }
        self.env = call_env;
        let result = match self.exec_scoped_block(&function.body) {
            Ok(Flow::Return(value)) => Ok(value),
            Ok(Flow::Normal) => Ok(Value::Null),
            Ok(_) => err("E7101", "函数里不能 `break` / `continue` 到调用者", span),
            Err(error) => Err(error),
        };
        self.env = saved_env;
        self.budgets.call_depth -= 1;
        result
    }

    fn assign_target(&mut self, target: &Expr, value: Value, span: Span) -> SlangResult<()> {
        let (root, segments) = self.collect_path(target)?;
        let Some(current) = self.env.get(&root) else {
            return err("E7101", format!("未知变量 `{root}`"), span);
        };
        if segments.is_empty() {
            self.env
                .assign(&root, value, self.module_running)
                .map_err(|error: AssignError| {
                    Diagnostic::error(error.code(), error.message(&root), span)
                })?;
        } else {
            let mut root_value = current;
            set_path(&mut root_value, &segments, value, span)?;
            self.env
                .assign(&root, root_value, self.module_running)
                .map_err(|error: AssignError| {
                    Diagnostic::error(error.code(), error.message(&root), span)
                })?;
        }
        Ok(())
    }

    fn collect_path(&mut self, expr: &Expr) -> SlangResult<(String, Vec<PathSeg>)> {
        match &expr.kind {
            ExprKind::Ident(name) => Ok((name.clone(), Vec::new())),
            ExprKind::Field(base, name) => {
                let (root, mut segments) = self.collect_path(base)?;
                segments.push(PathSeg::Field(name.clone()));
                Ok((root, segments))
            }
            ExprKind::Index(base, index) => {
                let (root, mut segments) = self.collect_path(base)?;
                let index = self.eval(index)?;
                segments.push(PathSeg::Index(index));
                Ok((root, segments))
            }
            _ => err("E7105", "赋值目标必须是变量、字段或下标", expr.span),
        }
    }

    // ── 模板渲染 ─────────────────────────────────────────────────────────────

    fn render_nodes(
        &mut self,
        nodes: &[Node],
        out: &mut String,
        html_mode: bool,
        depth: usize,
    ) -> SlangResult<()> {
        if depth > self.cx.budgets.max_depth {
            return err(
                "E6001",
                format!("渲染嵌套超过 {} 层", self.cx.budgets.max_depth),
                Span::default(),
            );
        }
        let saved = self.env.clone();
        self.env = Env::child(&saved);
        let mut result = Ok(());
        for node in nodes {
            if let Err(error) = self.render_node(node, out, html_mode, depth + 1) {
                result = Err(error);
                break;
            }
        }
        self.env = saved;
        result
    }

    fn render_node(
        &mut self,
        node: &Node,
        out: &mut String,
        html_mode: bool,
        depth: usize,
    ) -> SlangResult<()> {
        match &node.kind {
            NodeKind::Text(text) => {
                if html_mode {
                    self.append_html(out, text, node.span)
                } else {
                    self.append_text(out, text, node.span)
                }
            }
            NodeKind::Interp(expr) => {
                let value = self.eval(expr)?;
                self.append_text(out, &value.to_text(), node.span)
            }
            NodeKind::Component(call) => self.render_component(call, out, depth),
            NodeKind::If { arms, otherwise } => {
                for (condition, body) in arms {
                    if self.eval(condition)?.is_truthy() {
                        return self.render_nodes(body, out, html_mode, depth);
                    }
                }
                self.render_nodes(otherwise, out, html_mode, depth)
            }
            NodeKind::For {
                item,
                index,
                iter,
                body,
                otherwise,
            } => {
                let entries = iteration_entries(self.eval(iter)?, node.span)?;
                if entries.is_empty() {
                    return self.render_nodes(otherwise, out, html_mode, depth);
                }
                for (position, entry) in entries.into_iter().enumerate() {
                    self.budgets.loop_iterations += 1;
                    if self.budgets.loop_iterations > self.cx.budgets.max_loop_iterations {
                        return err(
                            "E6001",
                            format!("循环迭代超过 {} 次", self.cx.budgets.max_loop_iterations),
                            node.span,
                        );
                    }
                    let saved = self.env.clone();
                    self.env = Env::child(&saved);
                    self.env
                        .define(item.clone(), Binding::readonly(entry, BindingKind::Local));
                    if let Some(index) = index {
                        self.env.define(
                            index.clone(),
                            Binding::readonly(Value::Int(position as i64), BindingKind::Local),
                        );
                    }
                    let result = self.render_nodes(body, out, html_mode, depth);
                    self.env = saved;
                    result?;
                }
                Ok(())
            }
            NodeKind::Let { name, value } => {
                let value = self.eval(value)?;
                self.env
                    .define(name.clone(), Binding::readonly(value, BindingKind::Local));
                Ok(())
            }
            NodeKind::Include { path, body } => {
                if body.is_empty() {
                    return err(
                        "E5001",
                        format!("`@include \"{path}\"` 没有被展开"),
                        node.span,
                    );
                }
                self.render_nodes(body, out, html_mode, depth)
            }
            NodeKind::Use { path } => {
                err("E5001", format!("`@use \"{path}\"` 没有被展开"), node.span)
            }
            NodeKind::Html { body, expr } => {
                if let Some(expr) = expr {
                    let value = self.eval(expr)?;
                    self.append_trusted_html(out, &value.to_text(), node.span)
                } else if let Some(body) = body {
                    if self.cx.trust == Trust::Sandboxed {
                        let mut temp = String::new();
                        self.render_nodes(body, &mut temp, true, depth)?;
                        let clean = ammonia::clean(&temp);
                        self.append_html(out, &clean, node.span)
                    } else {
                        self.render_nodes(body, out, true, depth)
                    }
                } else {
                    Ok(())
                }
            }
            NodeKind::Md { expr } => {
                let value = self.eval(expr)?;
                let html = crate::markdown::to_html(&value.to_text());
                self.append_html(out, &html, node.span)
            }
            NodeKind::Slot {
                name,
                params,
                required,
                fallback,
            } => {
                let thunk = self
                    .instances
                    .last()
                    .and_then(|frame| frame.slots.get(name))
                    .cloned();
                if let Some(thunk) = thunk {
                    let mut args = Vec::with_capacity(params.len());
                    for param in params {
                        args.push(self.eval(param)?);
                    }
                    return self.render_slot_thunk(&thunk, args, out, depth);
                }
                if *required {
                    return err("E3005", format!("缺少必填插槽 `{name}`"), node.span);
                }
                self.render_nodes(fallback, out, html_mode, depth)
            }
            NodeKind::Fill { name, .. } => err(
                "E2001",
                format!("`@fill {name}` 只能出现在组件调用里"),
                node.span,
            ),
        }
    }

    fn render_slot_thunk(
        &mut self,
        thunk: &SlotThunk,
        args: Vec<Value>,
        out: &mut String,
        depth: usize,
    ) -> SlangResult<()> {
        let saved = self.env.clone();
        self.env = Env::child(&thunk.env);
        for (position, name) in thunk.params.iter().enumerate() {
            self.env.define(
                name.clone(),
                Binding::readonly(
                    args.get(position).cloned().unwrap_or(Value::Null),
                    BindingKind::Param,
                ),
            );
        }
        let result = self.render_nodes(&thunk.body, out, false, depth);
        self.env = saved;
        result
    }

    fn render_component(
        &mut self,
        call: &ComponentCall,
        out: &mut String,
        depth: usize,
    ) -> SlangResult<()> {
        let def = self.resolve_component(&call.name).ok_or_else(|| {
            Diagnostic::error("E3001", format!("未知组件 `{}`", call.name), call.span)
        })?;
        if self.budgets.component_depth >= self.cx.budgets.max_component_depth {
            return err(
                "E6001",
                format!("组件展开深度超过 {}", self.cx.budgets.max_component_depth),
                call.span,
            );
        }
        self.budgets.component_depth += 1;
        let result = match def.as_ref() {
            ComponentDef::Native(native) => {
                self.render_native(&native.spec, native.render, call, out)
            }
            ComponentDef::Composite(composite) => {
                self.render_composite(composite, call, out, depth)
            }
            ComponentDef::Element(element) => self.render_element(element, call, out, depth),
        };
        self.budgets.component_depth -= 1;
        result
    }

    fn resolve_component(&self, name: &str) -> Option<Arc<ComponentDef>> {
        self.template
            .defs
            .get(name)
            .cloned()
            .map(|def| ComponentDef::Composite((*def).clone()))
            .map(Arc::new)
            .or_else(|| self.engine.registry.get(name))
    }

    fn render_native(
        &mut self,
        spec: &ComponentSpec,
        render: super::component::NativeRenderFn,
        call: &ComponentCall,
        out: &mut String,
    ) -> SlangResult<()> {
        let args = self.bind_native_args(spec, call)?;
        let slots = self.render_native_slots(call, spec)?;
        let html = render(self, &args, &slots)?;
        self.append_html(out, &html, call.span)
    }

    fn render_composite(
        &mut self,
        def: &CompositeDef,
        call: &ComponentCall,
        out: &mut String,
        depth: usize,
    ) -> SlangResult<()> {
        let caller_env = self.env.clone();

        // 1. 实参（在调用点作用域求值）。
        let mut provided: BTreeMap<String, Value> = BTreeMap::new();
        let positional = def.props.names();
        let mut positional_index = 0usize;
        for arg in &call.args {
            let value = self.eval(&arg.value)?;
            let name = if let Some(name) = &arg.name {
                name.clone()
            } else {
                let Some(name) = positional.get(positional_index) else {
                    return err("E3002", "位置参数过多", arg.span);
                };
                positional_index += 1;
                name.clone()
            };
            if def.props.get(&name).is_none() {
                return err(
                    "E3002",
                    format!("`{}` 不支持参数 `{name}`", def.name),
                    arg.span,
                );
            }
            if provided.insert(name.clone(), value).is_some() {
                return err(
                    "E3002",
                    format!("`{}` 的参数 `{name}` 重复", def.name),
                    arg.span,
                );
            }
        }

        // 2. 插槽 thunk（捕获调用点作用域）。
        let slots = self.collect_slot_thunks(def, call, &caller_env)?;

        // 3. 实例作用域：props / slots / 模板。
        let instance_env = Env::child(&self.module_env);
        let mut props_map = BTreeMap::new();
        let mut bind_result = Ok(());
        for prop in &def.props.props {
            let value = if let Some(value) = provided.remove(&prop.name) {
                value
            } else if let Some(default) = &prop.default {
                default.to_value()
            } else if prop.required {
                bind_result = err(
                    "E3003",
                    format!("`{}` 缺少必填参数 `{}`", def.name, prop.name),
                    call.span,
                );
                break;
            } else {
                continue;
            };
            // 显式传 `null` 视为“空值”，不做类型检查。
            if !value.is_null() && !prop.ty.accepts(&value) {
                bind_result = err(
                    "E3006",
                    format!(
                        "`{}` 的参数 `{}` 需要 {}，得到 {}",
                        def.name,
                        prop.name,
                        prop.ty.describe(),
                        value.type_name()
                    ),
                    call.span,
                );
                break;
            }
            props_map.insert(prop.name.clone(), value.clone());
            instance_env.define(
                prop.name.clone(),
                Binding::readonly(value, BindingKind::Prop),
            );
        }

        let mut slots_map = BTreeMap::new();
        for name in slots.keys() {
            slots_map.insert(name.clone(), Value::Bool(true));
        }
        instance_env.define(
            "props",
            Binding::readonly(Value::Map(props_map), BindingKind::System),
        );
        instance_env.define(
            "slots",
            Binding::readonly(Value::Map(slots_map), BindingKind::System),
        );

        let mut result = bind_result;
        if result.is_ok() {
            self.instances.push(InstanceFrame {
                name: def.name.clone(),
                slots,
            });
            let saved_env = self.env.clone();
            self.env = instance_env;
            result = self.render_nodes(&def.body, out, false, depth);
            self.env = saved_env;
            self.instances.pop();
        }
        result
    }

    fn collect_slot_thunks(
        &mut self,
        def: &CompositeDef,
        call: &ComponentCall,
        caller_env: &Arc<Env>,
    ) -> SlangResult<BTreeMap<String, SlotThunk>> {
        let declared = collect_slot_info(&def.body);
        let mut slots = BTreeMap::new();
        let mut defaults: Vec<Node> = Vec::new();
        for child in &call.children {
            match &child.kind {
                NodeKind::Fill { name, params, body } => {
                    let Some(info) = declared.get(name) else {
                        return err(
                            "E3004",
                            format!("`{}` 没有插槽 `{name}`", def.name),
                            child.span,
                        );
                    };
                    if params.len() > info.params {
                        return err(
                            "E3004",
                            format!(
                                "插槽 `{name}` 只提供 {} 个上下文值，`@fill` 声明了 {} 个",
                                info.params,
                                params.len()
                            ),
                            child.span,
                        );
                    }
                    if slots
                        .insert(
                            name.clone(),
                            SlotThunk {
                                params: params.clone(),
                                body: body.clone(),
                                env: caller_env.clone(),
                                span: child.span,
                            },
                        )
                        .is_some()
                    {
                        return err("E3004", format!("插槽 `{name}` 被填充了两次"), child.span);
                    }
                }
                _ if is_ignorable_node(child) => {}
                _ => defaults.push(child.clone()),
            }
        }

        if !defaults.is_empty() {
            let Some(primary) = primary_slot(&declared) else {
                return err(
                    "E3004",
                    format!(
                        "`{}` 没有默认插槽，不能直接放子节点（可用插槽：{}）",
                        def.name,
                        slot_list(&declared)
                    ),
                    call.span,
                );
            };
            if declared[primary].params > 0 {
                return err(
                    "E3004",
                    format!(
                        "`{}` 的默认插槽需要 {} 个上下文值；请用 `@fill {primary}(...)`",
                        def.name, declared[primary].params
                    ),
                    call.span,
                );
            }
            if slots.contains_key(primary) {
                return err(
                    "E2001",
                    format!("插槽 `{primary}` 同时被子节点和 @fill 填充"),
                    call.span,
                );
            }
            slots.insert(
                primary.to_string(),
                SlotThunk {
                    params: Vec::new(),
                    body: defaults,
                    env: caller_env.clone(),
                    span: call.span,
                },
            );
        }

        for (name, info) in &declared {
            if info.required && !slots.contains_key(name) {
                return err(
                    "E3005",
                    format!("`{}` 缺少必填插槽 `{name}`", def.name),
                    call.span,
                );
            }
        }
        Ok(slots)
    }

    fn render_native_slots(
        &mut self,
        call: &ComponentCall,
        spec: &ComponentSpec,
    ) -> SlangResult<RenderedSlots> {
        let mut slots = RenderedSlots::default();
        let mut primary = String::new();
        let mut has_primary = false;
        for child in &call.children {
            match &child.kind {
                NodeKind::Fill { name, params, body } => {
                    if !params.is_empty() {
                        return err(
                            "E3004",
                            format!("`{}` 是原生组件，不支持作用域插槽参数", spec.name),
                            child.span,
                        );
                    }
                    if spec.slot_spec(name).is_none() {
                        return err("E3004", format!("没有插槽 `{name}`"), child.span);
                    }
                    let mut html = String::new();
                    let result = self.render_nodes(body, &mut html, false, 0);
                    result?;
                    slots.insert(name.clone(), html);
                }
                _ if is_ignorable_node(child) => {}
                _ => {
                    has_primary = true;
                    let result = self.render_node(child, &mut primary, false, 0);
                    result?;
                }
            }
        }
        if has_primary {
            if slots.has(&spec.primary_slot) {
                return err(
                    "E2001",
                    format!("插槽 `{}` 同时被子节点和 @fill 填充", spec.primary_slot),
                    call.span,
                );
            }
            slots.insert(spec.primary_slot.clone(), primary);
        }
        for slot in &spec.slots {
            if slot.required && !slots.has(&slot.name) {
                return err("E3005", format!("缺少必填插槽 `{}`", slot.name), call.span);
            }
        }
        Ok(slots)
    }

    fn render_element(
        &mut self,
        element: &ElementDef,
        call: &ComponentCall,
        out: &mut String,
        depth: usize,
    ) -> SlangResult<()> {
        let mut attrs: Vec<(String, Value)> = Vec::new();
        let mut positional_text: Option<Value> = None;
        for arg in &call.args {
            let value = self.eval(&arg.value)?;
            if let Some(name) = &arg.name {
                if !html::element_allows_attr(element, name) {
                    return err(
                        "E3008",
                        format!("`::{}` 不允许属性 `{name}`", element.tag),
                        arg.span,
                    );
                }
                if self.cx.trust == Trust::Sandboxed
                    && (name.starts_with("data-") || name.starts_with("hx-"))
                {
                    return err("E3008", format!("沙箱层级不允许属性 `{name}`"), arg.span);
                }
                attrs.push((name.clone(), value));
            } else {
                if positional_text.is_some() {
                    return err(
                        "E3002",
                        format!("`::{}` 最多接受一个位置文本参数", element.tag),
                        arg.span,
                    );
                }
                positional_text = Some(value);
            }
        }

        let mut body = String::new();
        if let Some(text) = positional_text {
            self.append_text(&mut body, &text.to_text(), call.span)?;
        }
        self.render_nodes(&call.children, &mut body, false, depth)?;

        if element.void && !body.is_empty() {
            return err(
                "E2001",
                format!("`::{}` 是空元素，不能有子节点", element.tag),
                call.span,
            );
        }

        let mut rendered_attrs = String::new();
        let mut present = Vec::new();
        for (name, value) in &attrs {
            present.push(name.clone());
            if let Some(rendered) = render_element_attr(name, value, call.span)? {
                rendered_attrs.push(' ');
                rendered_attrs.push_str(&rendered);
            }
        }
        html::check_required(element, &present, call.span)?;

        if element.tag == "a"
            && attrs
                .iter()
                .any(|(name, value)| name == "target" && value.as_str() == Some("_blank"))
            && !attrs.iter().any(|(name, _)| name == "rel")
        {
            rendered_attrs.push_str(" rel=\"noopener noreferrer\"");
        }

        if element.void {
            self.append_html(
                out,
                &format!("<{}{}>", element.tag, rendered_attrs),
                call.span,
            )
        } else {
            self.append_html(
                out,
                &format!(
                    "<{}{}>{}</{}>",
                    element.tag, rendered_attrs, body, element.tag
                ),
                call.span,
            )
        }
    }

    fn bind_native_args(
        &mut self,
        spec: &ComponentSpec,
        call: &ComponentCall,
    ) -> SlangResult<Args> {
        let mut values: BTreeMap<String, Value> = BTreeMap::new();
        let mut positional_index = 0usize;
        for arg in &call.args {
            let value = self.eval(&arg.value)?;
            if let Some(name) = &arg.name {
                if spec.prop_spec(name).is_none() {
                    return err(
                        "E3002",
                        format!("`{}` 不支持属性 `{name}`", spec.name),
                        arg.span,
                    );
                }
                if values.contains_key(name) {
                    return err(
                        "E3002",
                        format!("`{}` 的属性 `{name}` 重复", spec.name),
                        arg.span,
                    );
                }
                values.insert(name.clone(), value);
            } else {
                let Some(name) = spec.positional.get(positional_index) else {
                    return err(
                        "E3002",
                        format!(
                            "`{}` 最多接受 {} 个位置参数",
                            spec.name,
                            spec.positional.len()
                        ),
                        arg.span,
                    );
                };
                if values.contains_key(name) {
                    return err(
                        "E3002",
                        format!("`{}` 的属性 `{name}` 重复", spec.name),
                        arg.span,
                    );
                }
                values.insert(name.clone(), value);
                positional_index += 1;
            }
        }
        for prop in &spec.props {
            if !values.contains_key(&prop.name) {
                if let Some(default) = &prop.default {
                    values.insert(prop.name.clone(), default.clone());
                } else if prop.required {
                    return err(
                        "E3003",
                        format!("`{}` 缺少必填属性 `{}`", spec.name, prop.name),
                        call.span,
                    );
                }
            }
        }
        let mut args = Args::new(call.span);
        for (name, value) in values {
            args.insert(name, value);
        }
        Ok(args)
    }

    fn append_text(&mut self, out: &mut String, text: &str, span: Span) -> SlangResult<()> {
        let escaped = crate::ui::theme::escape(text);
        self.append_html(out, &escaped, span)
    }

    fn append_trusted_html(&mut self, out: &mut String, html: &str, span: Span) -> SlangResult<()> {
        if self.cx.trust == Trust::Sandboxed {
            let clean = ammonia::clean(html);
            self.append_html(out, &clean, span)
        } else {
            self.append_html(out, html, span)
        }
    }

    fn append_html(&mut self, out: &mut String, html: &str, span: Span) -> SlangResult<()> {
        self.budgets.output_bytes += html.len();
        if self.budgets.output_bytes > self.cx.budgets.max_output_bytes {
            return err(
                "E6001",
                format!("输出超过 {} 字节上限", self.cx.budgets.max_output_bytes),
                span,
            );
        }
        out.push_str(html);
        Ok(())
    }

    // ── 表达式 ───────────────────────────────────────────────────────────────

    fn eval(&mut self, expr: &Expr) -> SlangResult<Value> {
        match &expr.kind {
            ExprKind::Null => Ok(Value::Null),
            ExprKind::Bool(value) => Ok(Value::Bool(*value)),
            ExprKind::Int(value) => Ok(Value::Int(*value)),
            ExprKind::Float(value) => Ok(Value::Float(*value)),
            ExprKind::Str(value) => Ok(Value::Str(value.clone())),
            ExprKind::Template(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Text(text) => out.push_str(text),
                        TemplatePart::Expr(expr) => out.push_str(&self.eval(expr)?.to_text()),
                    }
                }
                Ok(Value::Str(out))
            }
            ExprKind::Ident(name) => self.env.get(name).ok_or_else(|| {
                Diagnostic::error("E4001", format!("未知标识符 `{name}`"), expr.span)
                    .with_hint("数据请走 `data.*`；局部变量先用 `@let` 定义")
            }),
            ExprKind::Field(base, name) => Ok(self.eval(base)?.get_field(name)),
            ExprKind::Index(base, index) => {
                let base = self.eval(base)?;
                let index = self.eval(index)?;
                Ok(base.get_index(&index))
            }
            ExprKind::Unary(op, value) => {
                let value = self.eval(value)?;
                match op {
                    UnOp::Not => Ok(Value::Bool(!value.is_truthy())),
                    UnOp::Neg => match value {
                        Value::Int(value) => Ok(Value::Int(-value)),
                        Value::Float(value) => Ok(Value::Float(-value)),
                        other => err(
                            "E4003",
                            format!("不能对 {} 取负", other.type_name()),
                            expr.span,
                        ),
                    },
                }
            }
            ExprKind::Binary(op, left, right) => match op {
                BinOp::And => {
                    let left = self.eval(left)?;
                    if !left.is_truthy() {
                        Ok(Value::Bool(false))
                    } else {
                        Ok(Value::Bool(self.eval(right)?.is_truthy()))
                    }
                }
                BinOp::Or => {
                    let left = self.eval(left)?;
                    if left.is_truthy() {
                        Ok(Value::Bool(true))
                    } else {
                        Ok(Value::Bool(self.eval(right)?.is_truthy()))
                    }
                }
                BinOp::Coalesce => {
                    let left = self.eval(left)?;
                    if left.is_null() {
                        self.eval(right)
                    } else {
                        Ok(left)
                    }
                }
                _ => {
                    let left = self.eval(left)?;
                    let right = self.eval(right)?;
                    self.eval_binary(*op, left, right, expr.span)
                }
            },
            ExprKind::Ternary(condition, then_branch, else_branch) => {
                if self.eval(condition)?.is_truthy() {
                    self.eval(then_branch)
                } else {
                    self.eval(else_branch)
                }
            }
            ExprKind::Call(name, args) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg)?);
                }
                if self.template.module.functions.contains_key(name) {
                    self.call_function(name, values, expr.span)
                } else {
                    self.call_builtin(name, &values, expr.span)
                }
            }
            ExprKind::Filter(name, value, args) => {
                let mut values = vec![self.eval(value)?];
                for arg in args {
                    values.push(self.eval(arg)?);
                }
                self.call_builtin(name, &values, expr.span)
            }
            ExprKind::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item)?);
                }
                Ok(Value::List(values))
            }
            ExprKind::Object(fields) => {
                let mut map = BTreeMap::new();
                for (key, value) in fields {
                    map.insert(key.clone(), self.eval(value)?);
                }
                Ok(Value::Map(map))
            }
        }
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        left: Value,
        right: Value,
        span: Span,
    ) -> SlangResult<Value> {
        match op {
            BinOp::Add => left
                .add(&right)
                .map_err(|reason| Diagnostic::error("E4003", reason, span)),
            BinOp::Sub => left
                .sub(&right)
                .map_err(|reason| Diagnostic::error("E4003", reason, span)),
            BinOp::Mul => left
                .mul(&right)
                .map_err(|reason| Diagnostic::error("E4003", reason, span)),
            BinOp::Div => left
                .div(&right)
                .map_err(|reason| Diagnostic::error("E4004", reason, span)),
            BinOp::Mod => left
                .rem(&right)
                .map_err(|reason| Diagnostic::error("E4004", reason, span)),
            BinOp::Eq => Ok(Value::Bool(left.equals(&right))),
            BinOp::Ne => Ok(Value::Bool(!left.equals(&right))),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let ordering = left.compare(&right).ok_or_else(|| {
                    Diagnostic::error(
                        "E4003",
                        format!("不能比较 {} 和 {}", left.type_name(), right.type_name()),
                        span,
                    )
                })?;
                Ok(Value::Bool(match op {
                    BinOp::Lt => ordering.is_lt(),
                    BinOp::Le => ordering.is_le(),
                    BinOp::Gt => ordering.is_gt(),
                    BinOp::Ge => ordering.is_ge(),
                    _ => false,
                }))
            }
            BinOp::And | BinOp::Or | BinOp::Coalesce => unreachable!(),
        }
    }

    // ── 内置函数 ─────────────────────────────────────────────────────────────

    fn call_builtin(&mut self, name: &str, args: &[Value], span: Span) -> SlangResult<Value> {
        match name {
            "len" => {
                expect_arity(name, args, 1, span)?;
                let value = match &args[0] {
                    Value::Str(value) => value.chars().count() as i64,
                    Value::List(items) => items.len() as i64,
                    Value::Map(map) => map.len() as i64,
                    Value::Null => 0,
                    other => {
                        return err(
                            "E4003",
                            format!("`len` 不支持 {}", other.type_name()),
                            span,
                        )
                    }
                };
                Ok(Value::Int(value))
            }
            "upper" | "lower" | "trim" => {
                expect_arity(name, args, 1, span)?;
                let value = as_string(name, &args[0], span)?;
                Ok(Value::Str(match name {
                    "upper" => value.to_uppercase(),
                    "lower" => value.to_lowercase(),
                    _ => value.trim().to_string(),
                }))
            }
            "truncate" => {
                expect_arity(name, args, 2, span)?;
                let value = as_string(name, &args[0], span)?;
                let max = as_int(name, &args[1], span)?.max(0) as usize;
                let mut out: String = value.chars().take(max).collect();
                if value.chars().count() > max {
                    out.push('…');
                }
                Ok(Value::Str(out))
            }
            "default" => {
                expect_arity(name, args, 2, span)?;
                if args[0].is_null() {
                    Ok(args[1].clone())
                } else {
                    Ok(args[0].clone())
                }
            }
            "coalesce" => {
                for value in args {
                    if !value.is_null() {
                        return Ok(value.clone());
                    }
                }
                Ok(Value::Null)
            }
            "join" => {
                expect_arity(name, args, 2, span)?;
                let list = as_list(name, &args[0], span)?;
                let separator = as_string(name, &args[1], span)?;
                Ok(Value::Str(
                    list.iter()
                        .map(Value::to_text)
                        .collect::<Vec<_>>()
                        .join(&separator),
                ))
            }
            "split" => {
                expect_arity(name, args, 2, span)?;
                let value = as_string(name, &args[0], span)?;
                let separator = as_string(name, &args[1], span)?;
                Ok(Value::List(if separator.is_empty() {
                    value
                        .chars()
                        .map(|ch| Value::Str(ch.to_string()))
                        .collect()
                } else {
                    value
                        .split(&separator)
                        .map(|part| Value::Str(part.to_string()))
                        .collect()
                }))
            }
            "replace" => {
                expect_arity(name, args, 3, span)?;
                let value = as_string(name, &args[0], span)?;
                let from = as_string(name, &args[1], span)?;
                let to = as_string(name, &args[2], span)?;
                Ok(Value::Str(value.replace(&from, &to)))
            }
            "contains" => {
                expect_arity(name, args, 2, span)?;
                let result = match &args[0] {
                    Value::Str(value) => value.contains(&args[1].to_text()),
                    Value::List(items) => items.iter().any(|item| item.equals(&args[1])),
                    Value::Map(map) => map.contains_key(&args[1].to_text()),
                    _ => false,
                };
                Ok(Value::Bool(result))
            }
            "starts_with" | "ends_with" => {
                expect_arity(name, args, 2, span)?;
                let value = as_string(name, &args[0], span)?;
                let needle = as_string(name, &args[1], span)?;
                Ok(Value::Bool(if name == "starts_with" {
                    value.starts_with(&needle)
                } else {
                    value.ends_with(&needle)
                }))
            }
            "concat" => Ok(Value::Str(
                args.iter().map(Value::to_text).collect::<Vec<_>>().join(""),
            )),
            "urlencode" => {
                expect_arity(name, args, 1, span)?;
                Ok(Value::Str(url_encode(&args[0].to_text())))
            }
            "json" => {
                expect_arity(name, args, 1, span)?;
                Ok(Value::Str(
                    serde_json::to_string(&args[0].to_json()).unwrap_or_default(),
                ))
            }
            "get" => {
                expect_arity(name, args, 2, span)?;
                Ok(args[0].get_index(&args[1]))
            }
            "keys" => {
                expect_arity(name, args, 1, span)?;
                match &args[0] {
                    Value::Map(map) => Ok(Value::List(
                        map.keys().cloned().map(Value::Str).collect(),
                    )),
                    Value::Null => Ok(Value::List(Vec::new())),
                    other => err(
                        "E4003",
                        format!("`keys` 需要对象，得到 {}", other.type_name()),
                        span,
                    ),
                }
            }
            "values" => {
                expect_arity(name, args, 1, span)?;
                match &args[0] {
                    Value::Map(map) => Ok(Value::List(map.values().cloned().collect())),
                    Value::Null => Ok(Value::List(Vec::new())),
                    other => err(
                        "E4003",
                        format!("`values` 需要对象，得到 {}", other.type_name()),
                        span,
                    ),
                }
            }
            "entries" => {
                expect_arity(name, args, 1, span)?;
                match &args[0] {
                    Value::Map(map) => Ok(Value::List(
                        map.iter()
                            .map(|(key, value)| {
                                let mut entry = BTreeMap::new();
                                entry.insert("key".to_string(), Value::Str(key.clone()));
                                entry.insert("value".to_string(), value.clone());
                                Value::Map(entry)
                            })
                            .collect(),
                    )),
                    Value::Null => Ok(Value::List(Vec::new())),
                    other => err(
                        "E4003",
                        format!("`entries` 需要对象，得到 {}", other.type_name()),
                        span,
                    ),
                }
            }
            "slice" => {
                if args.len() < 2 || args.len() > 3 {
                    return err("E4003", "`slice` 需要 `(list, start, end?)`", span);
                }
                let list = as_list(name, &args[0], span)?;
                let len = list.len() as i64;
                let normalize = |index: i64| -> usize {
                    if index < 0 {
                        (len + index).clamp(0, len) as usize
                    } else {
                        index.min(len) as usize
                    }
                };
                let start = normalize(as_int(name, &args[1], span)?);
                let end = match args.get(2) {
                    Some(value) => normalize(as_int(name, value, span)?),
                    None => list.len(),
                };
                if start >= end {
                    Ok(Value::List(Vec::new()))
                } else {
                    Ok(Value::List(list[start..end].to_vec()))
                }
            }
            "append" => {
                expect_arity(name, args, 2, span)?;
                let mut list = as_list(name, &args[0], span)?.to_vec();
                list.push(args[1].clone());
                Ok(Value::List(list))
            }
            "reverse" => {
                expect_arity(name, args, 1, span)?;
                let mut list = as_list(name, &args[0], span)?.to_vec();
                list.reverse();
                Ok(Value::List(list))
            }
            "unique" => {
                expect_arity(name, args, 1, span)?;
                let list = as_list(name, &args[0], span)?;
                let mut out: Vec<Value> = Vec::new();
                for item in list {
                    if !out.iter().any(|existing| existing.equals(item)) {
                        out.push(item.clone());
                    }
                }
                Ok(Value::List(out))
            }
            "sort" => {
                expect_arity(name, args, 1, span)?;
                let mut list = as_list(name, &args[0], span)?.to_vec();
                list.sort_by(|a, b| a.compare(b).unwrap_or(Ordering::Equal));
                Ok(Value::List(list))
            }
            "range" => {
                if args.is_empty() || args.len() > 3 {
                    return err(
                        "E4003",
                        "`range` 需要 `(n)` / `(start, end)` / `(start, end, step)`",
                        span,
                    );
                }
                let (start, end, step) = match args.len() {
                    1 => (0, as_int(name, &args[0], span)?, 1),
                    2 => (
                        as_int(name, &args[0], span)?,
                        as_int(name, &args[1], span)?,
                        1,
                    ),
                    _ => (
                        as_int(name, &args[0], span)?,
                        as_int(name, &args[1], span)?,
                        as_int(name, &args[2], span)?,
                    ),
                };
                if step == 0 {
                    return err("E4003", "`range` 的步长不能为 0", span);
                }
                let mut out = Vec::new();
                let mut cursor = start;
                let limit = self.cx.budgets.max_loop_iterations as i64;
                while (step > 0 && cursor < end) || (step < 0 && cursor > end) {
                    if out.len() as i64 >= limit {
                        return err("E4003", "`range` 生成的序列过长", span);
                    }
                    out.push(Value::Int(cursor));
                    cursor += step;
                }
                Ok(Value::List(out))
            }
            other => err("E4002", format!("未知函数 `{other}`"), span).map_err(
                |diagnostic: Diagnostic| {
                    diagnostic.with_hint(
                        "可用：len / upper / lower / trim / truncate / default / coalesce / join / split / replace / contains / starts_with / ends_with / concat / urlencode / json / get / keys / values / entries / slice / append / reverse / unique / sort / range",
                    )
                },
            ),
        }
    }
}

#[derive(Debug, Clone)]
enum PathSeg {
    Field(String),
    Index(Value),
}

fn set_path(value: &mut Value, segments: &[PathSeg], new: Value, span: Span) -> SlangResult<()> {
    if segments.is_empty() {
        *value = new;
        return Ok(());
    }
    match (&segments[0], value) {
        (PathSeg::Field(name), Value::Map(map)) => {
            let child = map.entry(name.clone()).or_insert(Value::Null);
            set_path(child, &segments[1..], new, span)
        }
        (PathSeg::Field(name), other) => err(
            "E7102",
            format!("`{name}` 赋值要求对象，得到 {}", other.type_name()),
            span,
        ),
        (PathSeg::Index(index), Value::List(items)) => {
            let position = index
                .as_i64()
                .ok_or_else(|| Diagnostic::error("E7102", "数组下标必须是整数", span))?;
            if position < 0 || position as usize >= items.len() {
                return err("E7102", format!("下标越界：{position}"), span);
            }
            let item = &mut items[position as usize];
            set_path(item, &segments[1..], new, span)
        }
        (PathSeg::Index(index), Value::Map(map)) => {
            let key = index
                .as_str()
                .ok_or_else(|| Diagnostic::error("E7102", "对象下标必须是字符串", span))?;
            let child = map.entry(key.to_string()).or_insert(Value::Null);
            set_path(child, &segments[1..], new, span)
        }
        (PathSeg::Index(_), other) => err(
            "E7102",
            format!("下标赋值要求数组或对象，得到 {}", other.type_name()),
            span,
        ),
    }
}

fn apply_assign_op(op: AssignOp, left: Value, right: Value, span: Span) -> SlangResult<Value> {
    match op {
        AssignOp::Set => Ok(right),
        AssignOp::Add => left
            .add(&right)
            .map_err(|reason| Diagnostic::error("E4003", reason, span)),
        AssignOp::Sub => left
            .sub(&right)
            .map_err(|reason| Diagnostic::error("E4003", reason, span)),
        AssignOp::Mul => left
            .mul(&right)
            .map_err(|reason| Diagnostic::error("E4003", reason, span)),
        AssignOp::Div => left
            .div(&right)
            .map_err(|reason| Diagnostic::error("E4004", reason, span)),
        AssignOp::Mod => left
            .rem(&right)
            .map_err(|reason| Diagnostic::error("E4004", reason, span)),
    }
}

fn statement_span(statement: &ScriptStmt) -> Span {
    match statement {
        ScriptStmt::Let { span, .. }
        | ScriptStmt::Assign { span, .. }
        | ScriptStmt::If { span, .. }
        | ScriptStmt::For { span, .. }
        | ScriptStmt::Return { span, .. } => *span,
        ScriptStmt::Break(span) | ScriptStmt::Continue(span) => *span,
        ScriptStmt::Function(function) => function.span,
    }
}

fn expect_arity(name: &str, args: &[Value], expected: usize, span: Span) -> SlangResult<()> {
    if args.len() == expected {
        Ok(())
    } else {
        err(
            "E4003",
            format!("`{name}` 需要 {expected} 个参数，得到 {}", args.len()),
            span,
        )
    }
}

fn as_string(name: &str, value: &Value, span: Span) -> SlangResult<String> {
    match value {
        Value::Str(value) => Ok(value.clone()),
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => Ok(value.to_text()),
        other => err(
            "E4003",
            format!("`{name}` 需要字符串，得到 {}", other.type_name()),
            span,
        ),
    }
}

fn as_int(name: &str, value: &Value, span: Span) -> SlangResult<i64> {
    value.as_i64().ok_or_else(|| {
        Diagnostic::error(
            "E4003",
            format!("`{name}` 需要整数，得到 {}", value.type_name()),
            span,
        )
    })
}

fn as_list<'v>(name: &str, value: &'v Value, span: Span) -> SlangResult<&'v [Value]> {
    value.as_list().ok_or_else(|| {
        Diagnostic::error(
            "E4003",
            format!("`{name}` 的第一个参数需要数组，得到 {}", value.type_name()),
            span,
        )
    })
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn iteration_entries(value: Value, span: Span) -> SlangResult<Vec<Value>> {
    match value {
        Value::List(items) => Ok(items),
        Value::Map(map) => Ok(map
            .into_iter()
            .map(|(key, value)| {
                let mut entry = BTreeMap::new();
                entry.insert("key".to_string(), Value::Str(key));
                entry.insert("value".to_string(), value);
                Value::Map(entry)
            })
            .collect()),
        Value::Str(value) => Ok(value.chars().map(|ch| Value::Str(ch.to_string())).collect()),
        Value::Null => Ok(Vec::new()),
        other => err(
            "E4003",
            format!("`@for` 不能遍历 {}", other.type_name()),
            span,
        ),
    }
}

fn render_element_attr(name: &str, value: &Value, span: Span) -> SlangResult<Option<String>> {
    if value.is_null() {
        return Ok(None);
    }
    if let Value::Bool(false) = value {
        return Ok(None);
    }
    if matches!(
        name,
        "hidden" | "open" | "required" | "disabled" | "reversed"
    ) {
        if value.is_truthy() {
            return Ok(Some(name.to_string()));
        }
        return Ok(None);
    }
    let rendered = match name {
        "class" => match value {
            Value::Str(value) => html::sanitize_class(value)
                .map_err(|reason| Diagnostic::error("E3006", reason, span))?,
            Value::List(items) => {
                let mut parts = Vec::new();
                for item in items {
                    if let Value::Str(value) = item {
                        parts.push(
                            html::sanitize_class(value)
                                .map_err(|reason| Diagnostic::error("E3006", reason, span))?,
                        );
                    }
                }
                parts.join(" ")
            }
            other => {
                return err(
                    "E3006",
                    format!("`class` 需要字符串或字符串数组，得到 {}", other.type_name()),
                    span,
                )
            }
        },
        "href" | "src" => html::sanitize_url(&value.to_text())
            .map_err(|reason| Diagnostic::error("E3007", reason, span))?,
        "tabindex" => {
            let index = value
                .as_i64()
                .ok_or_else(|| Diagnostic::error("E3006", "`tabindex` 需要整数", span))?;
            if index < -1 {
                return err("E3006", "`tabindex` 只能是 -1 或非负整数", span);
            }
            index.to_string()
        }
        _ => value.to_text(),
    };
    Ok(Some(format!(
        "{}=\"{}\"",
        name,
        html::escape_attr(&rendered)
    )))
}

// ─────────────────────────────────────────────────────────────────────────────
// 上下文值
// ─────────────────────────────────────────────────────────────────────────────

fn build_roots(cx: &RenderContext) -> Arc<Env> {
    let root = Env::root();
    root.define(
        "plugin",
        Binding::readonly(
            cx.plugin.as_ref().map(plugin_value).unwrap_or(Value::Null),
            BindingKind::Root,
        ),
    );
    root.define(
        "page",
        Binding::readonly(
            cx.page.as_ref().map(page_value).unwrap_or(Value::Null),
            BindingKind::Root,
        ),
    );
    root.define(
        "slot",
        Binding::readonly(
            cx.slot.as_ref().map(slot_value).unwrap_or(Value::Null),
            BindingKind::Root,
        ),
    );
    root.define(
        "app",
        Binding::readonly(app_value(&cx.app), BindingKind::Root),
    );
    root.define(
        "route",
        Binding::readonly(route_value(&cx.route), BindingKind::Root),
    );
    root.define(
        "data",
        Binding::readonly(cx.data.clone(), BindingKind::Root),
    );
    root
}

fn plugin_value(plugin: &PluginMeta) -> Value {
    let mut map = BTreeMap::new();
    map.insert("id".to_string(), Value::Str(plugin.id.clone()));
    map.insert("name".to_string(), Value::Str(plugin.name.clone()));
    map.insert("version".to_string(), Value::Str(plugin.version.clone()));
    map.insert(
        "description".to_string(),
        Value::Str(plugin.description.clone()),
    );
    Value::Map(map)
}

fn page_value(page: &PageMeta) -> Value {
    let mut map = BTreeMap::new();
    map.insert("slug".to_string(), Value::Str(page.slug.clone()));
    map.insert("title".to_string(), Value::Str(page.title.clone()));
    Value::Map(map)
}

fn slot_value(slot: &SlotMeta) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::Str(slot.name.clone()));
    map.insert("order".to_string(), Value::Int(slot.order as i64));
    map.insert("index".to_string(), Value::Int(slot.index as i64));
    Value::Map(map)
}

fn app_value(app: &AppMeta) -> Value {
    let mut map = BTreeMap::new();
    map.insert("workspace".to_string(), Value::Str(app.workspace.clone()));
    map.insert("gateway".to_string(), Value::Str(app.gateway.clone()));
    map.insert("product".to_string(), Value::Str(app.product.clone()));
    map.insert("version".to_string(), Value::Str(app.version.clone()));
    Value::Map(map)
}

fn route_value(route: &RouteMeta) -> Value {
    let mut map = BTreeMap::new();
    map.insert("path".to_string(), Value::Str(route.path.clone()));
    map.insert("query".to_string(), Value::Str(route.query.clone()));
    Value::Map(map)
}

/// 便捷入口：默认原生组件注册表。
pub fn default_engine() -> Engine {
    Engine::new(native::default_registry())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MemoryLoader {
        root: PathBuf,
        files: HashMap<PathBuf, String>,
    }

    impl Loader for MemoryLoader {
        fn load(&self, path: &Path) -> SlangResult<String> {
            self.files.get(path).cloned().ok_or_else(|| {
                Diagnostic::error(
                    "E5001",
                    format!("memory loader 没有文件 {}", path.display()),
                    Span::default(),
                )
            })
        }

        fn canonicalize(&self, path: &Path) -> SlangResult<PathBuf> {
            if path.starts_with(&self.root) {
                Ok(path.to_path_buf())
            } else {
                err(
                    "E5001",
                    format!("路径越界：{}", path.display()),
                    Span::default(),
                )
            }
        }
    }

    fn render(source: &str, data: Value) -> String {
        let engine = default_engine();
        let template = engine.compile_str(source).expect("compile");
        let cx = RenderContext {
            data,
            ..Default::default()
        };
        engine.render(&template, &cx).expect("render")
    }

    fn render_result(source: &str, data: Value) -> SlangResult<String> {
        let engine = default_engine();
        let template = engine.compile_str(source)?;
        let cx = RenderContext {
            data,
            ..Default::default()
        };
        engine.render(&template, &cx)
    }

    fn compile_error(source: &str) -> Diagnostic {
        default_engine().compile_str(source).unwrap_err()
    }

    #[test]
    fn renders_text_interpolation_and_escaping() {
        let html = render(
            "Hello, {{ data.name }}!",
            serde_json::json!({"name": "world"}).into(),
        );
        assert_eq!(html, "Hello, world!");

        let html = render(
            "{{ data.x }}",
            serde_json::json!({"x": "<script>alert(1)</script>"}).into(),
        );
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn renders_native_components_and_control_flow() {
        let html = render(
            r#"
            @let label = `共 {{ data.items | len }} 条`;
            @if data.flag { yes } @else { no }
            @for item in data.items { [{{ item }}] } @else { empty }
            {{ label }}
            ::Button("重试", action: "/threads/t1/retry", variant: "primary")
            "#,
            serde_json::json!({"flag": true, "items": [1, 2]}).into(),
        );
        assert!(html.contains("yes"), "{html}");
        assert!(!html.contains("no"), "{html}");
        assert!(html.contains("[1]"), "{html}");
        assert!(html.contains("[2]"), "{html}");
        assert!(html.contains("共 2 条"), "{html}");
        assert!(html.contains("ada-btn--primary"), "{html}");
    }

    #[test]
    fn module_functions_are_shared_and_pure() {
        let html = render(
            r#"
            @script {
              function toneFor(count) {
                if (count > 0) { return "success"; }
                return "warn";
              }
              function badge(count) {
                if (count <= 0) { return "空"; }
                return `{{ count }} 条`;
              }
            }
            ::Chip(text: badge(data.count), tone: toneFor(data.count))
            "#,
            serde_json::json!({"count": 3}).into(),
        );
        assert!(html.contains("3 条"), "{html}");
        assert!(html.contains("ada-chip--success"), "{html}");
    }

    #[test]
    fn module_functions_can_mutate_their_own_locals_and_module_script_can_init() {
        let html = render(
            r#"
            @script {
              let total = 0;
              for n in data.numbers { total = total + n; }
              function double(n) { return n * 2; }
            }
            {{ total }}/{{ double(data.numbers | len) }}
            "#,
            serde_json::json!({"numbers": [1, 2, 3]}).into(),
        );
        assert_eq!(html.trim(), "6/6");
    }

    #[test]
    fn functions_do_not_capture_component_locals() {
        let html = render(
            r#"
            @script { function label(n) { return `{{ n }} 条`; } }
            @def Badge {
              @props { count: { type: Int, default: 0 } }
              ::Chip(text: label(count), tone: "brand")
            }
            ::Badge(count: 2)
            "#,
            Value::Null,
        );
        assert!(html.contains("2 条"), "{html}");
    }

    #[test]
    fn props_contract_is_type_checked_and_defaulted() {
        let html = render(
            r#"
            @def Card {
              @props {
                title: String,
                count: { type: Int, required: true },
                tone: { type: String, default: "neutral" },
              }
              ::Chip(text: `{{ title }}:{{ count }}`, tone: tone)
            }
            ::Card(title: "x", count: 3)
            "#,
            Value::Null,
        );
        assert!(html.contains("x:3"), "{html}");
        assert!(html.contains("ada-chip--neutral"), "{html}");

        let error = render_result(
            r#"
            @def Card {
              @props { title: String }
              ::span { {{ title }} }
            }
            ::Card(title: 1)
            "#,
            Value::Null,
        )
        .unwrap_err();
        assert_eq!(error.code, "E3006");
    }

    #[test]
    fn missing_required_prop_is_compile_error() {
        let error = compile_error(
            r#"
            @def Card {
              @props { title: String }
              ::span { x }
            }
            ::Card()
            "#,
        );
        assert_eq!(error.code, "E3003");
    }

    #[test]
    fn def_parameter_lists_and_script_setup_are_rejected() {
        let error = compile_error(
            r#"
            @def Card(title) { ::span { {{ title }} } }
            ::Card(title: "x")
            "#,
        );
        assert_eq!(error.code, "E2001");
        assert!(error.message.contains("@props"), "{error:?}");

        let error = compile_error(
            r#"
            @def Card {
              @script setup { const x = 1; }
              ::span { x }
            }
            "#,
        );
        assert_eq!(error.code, "E2001");
        assert!(error.message.contains("@script setup"), "{error:?}");
    }

    #[test]
    fn props_block_must_be_inside_a_component() {
        let error = compile_error(
            r#"
            @props { label: String }
            ok
            "#,
        );
        assert_eq!(error.code, "E2001");
    }

    #[test]
    fn component_props_only_close_over_module_scope() {
        let html = render(
            r#"
            @script { let secret = "module"; }
            @def Card {
              @props { label: String }
              ::span { {{ label }}/{{ secret }} }
            }
            ::Card(label: "x")
            "#,
            Value::Null,
        );
        assert!(html.contains("x/module"), "{html}");
    }

    #[test]
    fn module_functions_cannot_mutate_module_lets_from_templates() {
        let error = render_result(
            r#"
            @script {
              let n = 1;
              function bump() { n += 1; return n; }
            }
            {{ bump() }}
            "#,
            Value::Null,
        )
        .unwrap_err();
        assert_eq!(error.code, "E7105");
    }

    #[test]
    fn scoped_slots_receive_child_context() {
        let html = render(
            r#"
            @def Rows {
              @props { items: { type: "list", default: [] } }
              ::ul(class: "rows") {
                @for (item, index) in items {
                  ::li { @slot row(item, index) { 默认 {{ item }} } }
                }
              }
            }
            ::Rows(items: ["a", "b"]) {
              @fill row(value, i) { [{{ i }}:{{ value }}] }
            }
            "#,
            Value::Null,
        );
        assert!(html.contains("[0:a]"), "{html}");
        assert!(html.contains("[1:b]"), "{html}");
        assert!(!html.contains("默认"), "{html}");
    }

    #[test]
    fn bare_children_fill_only_the_default_slot() {
        let html = render(
            r#"
            @def Card {
              ::div(class: "card") { @slot default { fallback } }
            }
            ::Card() { 正文 }
            "#,
            Value::Null,
        );
        assert!(html.contains("正文"), "{html}");
        assert!(!html.contains("fallback"), "{html}");

        let error = compile_error(
            r#"
            @def Card { ::div { @slot body { fallback } } }
            ::Card() { 正文 }
            "#,
        );
        assert_eq!(error.code, "E3004");
    }

    #[test]
    fn required_slot_is_checked_at_compile_time() {
        let error = compile_error(
            r#"
            @def Card { ::div { @slot body required { x } } }
            ::Card()
            "#,
        );
        assert_eq!(error.code, "E3005");
    }

    #[test]
    fn slots_object_exposes_provided_names() {
        let html = render(
            r#"
            @def Card {
              @slot header { 默认头 }
              @if slots.header { <has-header> }
            }
            ::Card() { @fill header { 自定义头 } }
            "#,
            Value::Null,
        );
        // `<has-header>` 被转义成文本，但条件分支确实被渲染了
        assert!(html.contains("自定义头"), "{html}");
        assert!(html.contains("&lt;has-header&gt;"), "{html}");
    }

    #[test]
    fn data_helpers_work_without_lambdas() {
        let html = render(
            r#"
            {{ data.items | sort | join(",") }}
            {{ keys(data.map) | sort | join("/") }}
            {{ range(0, 3) | join("-") }}
            {{ unique([1, 1, 2]) | join("+") }}
            "#,
            serde_json::json!({"items": [3, 1, 2], "map": {"b": 1, "a": 2}}).into(),
        );
        assert!(html.contains("1,2,3"), "{html}");
        assert!(html.contains("a/b"), "{html}");
        assert!(html.contains("0-1-2"), "{html}");
        assert!(html.contains("1+2"), "{html}");
    }

    #[test]
    fn lambdas_and_events_are_rejected() {
        let error = compile_error(
            r#"
            @def Card { @props { x: Int } ::span { {{ x }} } }
            ::Card(x: (v) => v)
            "#,
        );
        assert_eq!(error.code, "E2001");

        let error = compile_error(
            r#"
            @def Card { ::span { x } }
            ::Card(on_pick: "x")
            "#,
        );
        assert_eq!(error.code, "E3002");
    }

    #[test]
    fn unknown_component_reports_hint() {
        let error = compile_error(r#"::Buton("x")"#);
        assert_eq!(error.code, "E3001");
        assert!(error.hint.unwrap_or_default().contains("Button"));
    }

    #[test]
    fn component_cycles_are_rejected() {
        let error = compile_error(
            r#"
            @def A { ::B() }
            @def B { ::A() }
            ::A()
            "#,
        );
        assert_eq!(error.code, "E5003");
    }

    #[test]
    fn includes_render_in_current_scope_and_use_imports_functions() {
        let root = PathBuf::from("C:/virtual-include");
        let page = root.join("page.slang");
        let partial = root.join("partial.slang");
        let components = root.join("components.slang");
        let mut files = HashMap::new();
        files.insert(
            partial.clone(),
            r#"::span(class: "partial") { {{ label }} }"#.to_string(),
        );
        files.insert(
            components.clone(),
            r#"
            @script { function toneFor(n) { if (n > 0) { return "success"; } return "warn"; } }
            @def Badge {
              @props { n: Int }
              ::Chip(text: n, tone: toneFor(n))
            }
            "#
            .to_string(),
        );
        files.insert(
            page.clone(),
            r#"@use "components.slang"
               @let label = "共享作用域";
               @include "partial.slang"
               ::Badge(n: 2)"#
                .to_string(),
        );
        let loader = MemoryLoader {
            root: root.clone(),
            files,
        };
        let engine = default_engine();
        let source = loader.load(&page).unwrap();
        let template = engine
            .compile(&source, Some(page), Some(root), &loader)
            .expect("compile include/use");
        let html = engine
            .render(&template, &RenderContext::default())
            .expect("render");
        assert!(html.contains("共享作用域"), "{html}");
        assert!(html.contains("ada-chip--success"), "{html}");
    }

    #[test]
    fn budget_stops_runaway_loop_and_recursion() {
        let engine = default_engine();
        let template = engine
            .compile_str("@for item in data.items { x }")
            .expect("compile");
        let mut cx = RenderContext {
            data: serde_json::json!({"items": vec![1; 20]}).into(),
            ..Default::default()
        };
        cx.budgets.max_loop_iterations = 3;
        let error = engine.render(&template, &cx).unwrap_err();
        assert_eq!(error.code, "E6001");

        let template = engine
            .compile_str(
                r#"
                @script { function down(n) { return down(n - 1); } }
                {{ down(10) }}
                "#,
            )
            .expect("compile recursion");
        let mut cx = RenderContext::default();
        cx.budgets.max_call_depth = 4;
        let error = engine.render(&template, &cx).unwrap_err();
        assert_eq!(error.code, "E7103");
    }

    #[test]
    fn html_blocks_and_sandbox_sanitize_data() {
        let html = render(
            r#"@html { <b>{{ data.x }}</b> }"#,
            serde_json::json!({"x": "<script>"}).into(),
        );
        assert!(html.contains("<b>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");

        let engine = default_engine();
        let template = engine.compile_str("@html(data.html)").unwrap();
        let cx = RenderContext {
            data: serde_json::json!({"html": "<b>ok</b><script>alert(1)</script>"}).into(),
            trust: Trust::Sandboxed,
            ..Default::default()
        };
        let html = engine.render(&template, &cx).unwrap();
        assert!(html.contains("<b>ok</b>"), "{html}");
        assert!(!html.contains("<script"), "{html}");
    }

    #[test]
    fn only_version_two_is_accepted() {
        assert_eq!(render("@slang 2\nok", Value::Null), "ok");
        let error = compile_error("@slang 1\nok");
        assert_eq!(error.code, "E2001");
    }
}
