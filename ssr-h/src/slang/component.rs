//! 组件契约与注册表。
//!
//! - [`ComponentSpec`]：原生组件的名字、属性、插槽、位置参数、primary slot；
//! - [`PropsDecl`] / [`PropDecl`]：组合组件的 props 契约（来自 `@props`），
//!   编译期校验、渲染期绑定；
//! - [`CompositeDef`]：一个 `@def` 组件（props + 模板，没有 setup）；
//! - [`Args`] / [`RenderedSlots`]：原生组件收到的求值后实参与已渲染插槽。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use super::ast::{ConstValue, Node, Span};
use super::error::{err, Diagnostic, SlangResult};
use super::value::Value;

/// 属性类型。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PropType {
    String,
    Int,
    Float,
    Bool,
    Enum(Vec<String>),
    Url,
    Icon,
    Class,
    List(Box<PropType>),
    Object,
    #[default]
    Any,
}

impl PropType {
    pub fn from_name(name: &str) -> Option<PropType> {
        match name {
            "String" | "string" | "str" => Some(PropType::String),
            "Int" | "int" | "integer" => Some(PropType::Int),
            "Float" | "float" | "number" | "Number" => Some(PropType::Float),
            "Bool" | "bool" | "boolean" => Some(PropType::Bool),
            "List" | "list" | "Array" | "array" => Some(PropType::List(Box::new(PropType::Any))),
            "Object" | "object" | "Map" | "map" => Some(PropType::Object),
            "Any" | "any" | "Unknown" | "unknown" => Some(PropType::Any),
            "Url" | "url" => Some(PropType::Url),
            "Icon" | "icon" => Some(PropType::Icon),
            "Class" | "class" => Some(PropType::Class),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            PropType::String => "string".to_string(),
            PropType::Int => "int".to_string(),
            PropType::Float => "number".to_string(),
            PropType::Bool => "bool".to_string(),
            PropType::Enum(values) => format!("enum({})", values.join(" | ")),
            PropType::Url => "url".to_string(),
            PropType::Icon => "icon".to_string(),
            PropType::Class => "class".to_string(),
            PropType::List(inner) => format!("list<{}>", inner.describe()),
            PropType::Object => "object".to_string(),
            PropType::Any => "any".to_string(),
        }
    }

    /// 运行期类型检查。`Null` 只会走到“未提供”分支，不会进这里。
    pub fn accepts(&self, value: &Value) -> bool {
        match self {
            PropType::Any => true,
            PropType::String | PropType::Url | PropType::Icon | PropType::Class => {
                matches!(value, Value::Str(_))
            }
            PropType::Int => value.as_i64().is_some(),
            PropType::Float => value.as_f64().is_some(),
            PropType::Bool => matches!(value, Value::Bool(_)),
            PropType::Enum(values) => value
                .as_str()
                .is_some_and(|value| values.iter().any(|allowed| allowed == value)),
            PropType::List(inner) => value
                .as_list()
                .is_some_and(|items| items.iter().all(|item| inner.accepts(item))),
            PropType::Object => matches!(value, Value::Map(_)),
        }
    }
}

/// 一个 props 声明。
#[derive(Debug, Clone)]
pub struct PropDecl {
    pub name: String,
    pub ty: PropType,
    pub required: bool,
    /// 编译期字面量默认值。
    pub default: Option<ConstValue>,
    pub span: Span,
}

impl PropDecl {
    pub fn new(name: &str, ty: PropType, required: bool) -> Self {
        Self {
            name: name.to_string(),
            ty,
            required,
            default: None,
            span: Span::default(),
        }
    }
}

/// 一组 props 声明（顺序即位置参数顺序）。
#[derive(Debug, Clone, Default)]
pub struct PropsDecl {
    pub props: Vec<PropDecl>,
}

impl PropsDecl {
    pub fn get(&self, name: &str) -> Option<&PropDecl> {
        self.props.iter().find(|prop| prop.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.props.iter().map(|prop| prop.name.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }
}

/// 一个 `@def` 组件。
#[derive(Debug, Clone)]
pub struct CompositeDef {
    pub name: String,
    pub props: PropsDecl,
    pub body: Vec<Node>,
    pub span: Span,
    pub source: Option<PathBuf>,
}

impl CompositeDef {
    pub fn slot_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        collect_slot_names(&self.body, &mut names);
        names
    }
}

fn collect_slot_names(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match &node.kind {
            super::ast::NodeKind::Slot { name, .. } => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            super::ast::NodeKind::If { arms, otherwise } => {
                for (_, body) in arms {
                    collect_slot_names(body, out);
                }
                collect_slot_names(otherwise, out);
            }
            super::ast::NodeKind::For {
                body, otherwise, ..
            } => {
                collect_slot_names(body, out);
                collect_slot_names(otherwise, out);
            }
            super::ast::NodeKind::Component(call) => collect_slot_names(&call.children, out),
            super::ast::NodeKind::Html { body, .. } => {
                if let Some(body) = body {
                    collect_slot_names(body, out);
                }
            }
            super::ast::NodeKind::Fill { body, .. } => collect_slot_names(body, out),
            _ => {}
        }
    }
}

/// 原生组件渲染函数。
pub type NativeRenderFn =
    fn(&mut super::eval::RenderCx<'_>, &Args, &RenderedSlots) -> SlangResult<String>;

/// 原生组件定义。
#[derive(Clone)]
pub struct NativeDef {
    pub spec: ComponentSpec,
    pub render: NativeRenderFn,
}

impl std::fmt::Debug for NativeDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDef")
            .field("spec", &self.spec.name)
            .finish()
    }
}

/// 内置 HTML 元素定义。
#[derive(Debug, Clone)]
pub struct ElementDef {
    pub tag: &'static str,
    pub allowed_attrs: &'static [&'static str],
    pub required_attrs: &'static [&'static str],
    pub void: bool,
    pub primary_slot: &'static str,
}

/// 组件定义。
#[derive(Debug, Clone)]
pub enum ComponentDef {
    Native(NativeDef),
    Composite(CompositeDef),
    Element(ElementDef),
}

impl ComponentDef {
    pub fn name(&self) -> &str {
        match self {
            ComponentDef::Native(def) => &def.spec.name,
            ComponentDef::Composite(def) => &def.name,
            ComponentDef::Element(def) => def.tag,
        }
    }
}

/// 组件注册表：原生组件 + 内置元素 + 全局组合组件。
#[derive(Debug, Clone, Default)]
pub struct ComponentRegistry {
    defs: HashMap<String, Arc<ComponentDef>>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: &str, def: ComponentDef) {
        self.defs.insert(name.to_string(), Arc::new(def));
    }

    pub fn register_arc(&mut self, name: &str, def: Arc<ComponentDef>) {
        self.defs.insert(name.to_string(), def);
    }

    pub fn get(&self, name: &str) -> Option<Arc<ComponentDef>> {
        self.defs.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.defs.keys().map(String::as_str).collect()
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

/// 一个插槽声明（供原生组件与校验使用）。
#[derive(Debug, Clone)]
pub struct SlotSpec {
    pub name: String,
    pub required: bool,
}

impl SlotSpec {
    pub fn new(name: &str, required: bool) -> Self {
        Self {
            name: name.to_string(),
            required,
        }
    }
}

/// 一个原生组件的静态契约。
#[derive(Debug, Clone)]
pub struct ComponentSpec {
    pub name: String,
    pub aliases: Vec<String>,
    pub props: Vec<PropSpec>,
    pub slots: Vec<SlotSpec>,
    /// 位置参数按顺序映射到这些属性名。
    pub positional: Vec<String>,
    /// 裸子节点填到哪个插槽。
    pub primary_slot: String,
}

impl ComponentSpec {
    pub fn new(name: &str, primary_slot: &str) -> Self {
        Self {
            name: name.to_string(),
            aliases: Vec::new(),
            props: Vec::new(),
            slots: Vec::new(),
            positional: Vec::new(),
            primary_slot: primary_slot.to_string(),
        }
    }

    pub fn alias(mut self, alias: &str) -> Self {
        self.aliases.push(alias.to_string());
        self
    }

    pub fn prop(mut self, prop: PropSpec) -> Self {
        self.props.push(prop);
        self
    }

    pub fn slot(mut self, slot: SlotSpec) -> Self {
        self.slots.push(slot);
        self
    }

    pub fn positional(mut self, names: &[&str]) -> Self {
        self.positional = names.iter().map(|name| name.to_string()).collect();
        self
    }

    pub fn prop_spec(&self, name: &str) -> Option<&PropSpec> {
        self.props.iter().find(|prop| prop.name == name)
    }

    pub fn slot_spec(&self, name: &str) -> Option<&SlotSpec> {
        self.slots.iter().find(|slot| slot.name == name)
    }
}

/// 一个原生属性声明。
#[derive(Debug, Clone)]
pub struct PropSpec {
    pub name: String,
    pub ty: PropType,
    pub required: bool,
    pub default: Option<Value>,
}

impl PropSpec {
    pub fn required(name: &str, ty: PropType) -> Self {
        Self {
            name: name.to_string(),
            ty,
            required: true,
            default: None,
        }
    }

    pub fn optional(name: &str, ty: PropType) -> Self {
        Self {
            name: name.to_string(),
            ty,
            required: false,
            default: None,
        }
    }

    pub fn default_value(name: &str, ty: PropType, value: Value) -> Self {
        Self {
            name: name.to_string(),
            ty,
            required: false,
            default: Some(value),
        }
    }
}

/// 求值后的组件实参。
#[derive(Debug, Clone, Default)]
pub struct Args {
    values: BTreeMap<String, Value>,
    span: Span,
}

impl Args {
    pub fn new(span: Span) -> Self {
        Self {
            values: BTreeMap::new(),
            span,
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, value: Value) {
        self.values.insert(name.into(), value);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn string(&self, name: &str) -> SlangResult<Option<String>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Str(value) => Ok(Some(value.clone())),
            Value::Int(_) | Value::Float(_) | Value::Bool(_) => Ok(Some(value.to_text())),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要字符串，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn required_string(&self, name: &str) -> SlangResult<String> {
        self.string(name)?
            .ok_or_else(|| Diagnostic::error("E3003", format!("属性 `{name}` 必填"), self.span))
    }

    pub fn bool(&self, name: &str) -> SlangResult<Option<bool>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Bool(value) => Ok(Some(*value)),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要布尔值，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn int(&self, name: &str) -> SlangResult<Option<i64>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Int(value) => Ok(Some(*value)),
            Value::Float(value) if value.fract() == 0.0 => Ok(Some(*value as i64)),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要整数，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn float(&self, name: &str) -> SlangResult<Option<f64>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Int(value) => Ok(Some(*value as f64)),
            Value::Float(value) => Ok(Some(*value)),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要数字，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn enum_(&self, name: &str, allowed: &[&'static str]) -> SlangResult<Option<String>> {
        let Some(value) = self.string(name)? else {
            return Ok(None);
        };
        if allowed.contains(&value.as_str()) {
            Ok(Some(value))
        } else {
            err(
                "E3006",
                format!(
                    "属性 `{name}` 的值 `{value}` 不在允许范围：{}",
                    allowed.join(" / ")
                ),
                self.span,
            )
        }
    }

    pub fn url(&self, name: &str) -> SlangResult<Option<String>> {
        let Some(value) = self.string(name)? else {
            return Ok(None);
        };
        super::html::sanitize_url(&value)
            .map(Some)
            .map_err(|reason| Diagnostic::error("E3007", reason, self.span))
    }

    pub fn icon(&self, name: &str) -> SlangResult<Option<String>> {
        let Some(value) = self.string(name)? else {
            return Ok(None);
        };
        if crate::ui::icons::has(&value) {
            Ok(Some(value))
        } else {
            err("E3006", format!("未知图标 `{value}`"), self.span)
        }
    }

    pub fn class(&self, name: &str) -> SlangResult<Option<String>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Str(value) => Ok(Some(
                super::html::sanitize_class(value)
                    .map_err(|reason| Diagnostic::error("E3006", reason, self.span))?,
            )),
            Value::List(items) => {
                let mut parts = Vec::new();
                for item in items {
                    match item {
                        Value::Str(value) => parts.push(
                            super::html::sanitize_class(value)
                                .map_err(|reason| Diagnostic::error("E3006", reason, self.span))?,
                        ),
                        Value::Null => {}
                        other => {
                            return err(
                                "E3006",
                                format!("属性 `{name}` 的 class 列表里出现 {}", other.type_name()),
                                self.span,
                            )
                        }
                    }
                }
                Ok(Some(parts.join(" ")))
            }
            other => err(
                "E3006",
                format!(
                    "属性 `{name}` 需要字符串或字符串数组，得到 {}",
                    other.type_name()
                ),
                self.span,
            ),
        }
    }

    pub fn list(&self, name: &str) -> SlangResult<Option<&[Value]>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::List(items) => Ok(Some(items)),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要数组，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn object(&self, name: &str) -> SlangResult<Option<&BTreeMap<String, Value>>> {
        let Some(value) = self.values.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Map(map) => Ok(Some(map)),
            other => err(
                "E3006",
                format!("属性 `{name}` 需要对象，得到 {}", other.type_name()),
                self.span,
            ),
        }
    }

    pub fn raw(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }
}

/// 调用点已渲染好的插槽内容（原生组件专用）。
#[derive(Debug, Clone, Default)]
pub struct RenderedSlots {
    pub map: HashMap<String, String>,
}

impl RenderedSlots {
    pub fn insert(&mut self, name: impl Into<String>, html: String) {
        self.map.insert(name.into(), html);
    }

    pub fn get(&self, name: &str) -> &str {
        self.map.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn has(&self, name: &str) -> bool {
        self.map.get(name).is_some_and(|value| !value.is_empty())
    }
}
