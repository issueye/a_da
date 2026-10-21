//! Slang 2 的运行期基元：词法作用域、插槽 thunk、预算。
//!
//! Slang 2（精简版）没有闭包、没有函数值、没有 ref/computed：
//! 函数只是模块脚本里的具名函数，调用时新建一个参数作用域；
//! 作用域链只用于“模块 → 组件实例 → 块 → 函数调用”的变量查找。
//!
//! 用 `Arc` + `Mutex` 而不是 `Rc` + `RefCell`：`Value` 会穿过 `RenderContext`
//! （它要能作为 axum 处理器状态的一部分跨线程传递）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use super::ast::{Node, Span};
use super::value::Value;

/// 统一取锁：中毒时取回内部值（渲染期不允许因为一个 panic 的锁把整页拖死）。
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `plugin` / `page` / `slot` / `app` / `route` / `data`。
    Root,
    /// 模块脚本（`@script`）里的绑定。
    Module,
    /// 组件 props（只读）。
    Prop,
    /// 组件级自动绑定：`props` / `slots`。
    System,
    /// 组件/模板/函数里的局部绑定。
    Local,
    /// 函数参数（只读）。
    Param,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub value: Value,
    pub mutable: bool,
    pub kind: BindingKind,
}

impl Binding {
    pub fn new(value: Value, mutable: bool, kind: BindingKind) -> Self {
        Self {
            value,
            mutable,
            kind,
        }
    }

    pub fn readonly(value: Value, kind: BindingKind) -> Self {
        Self {
            value,
            mutable: false,
            kind,
        }
    }
}

/// 一次渲染内的词法作用域链。
#[derive(Debug)]
pub struct Env {
    vars: Mutex<BTreeMap<String, Binding>>,
    parent: Option<Arc<Env>>,
}

impl Env {
    pub fn root() -> Arc<Env> {
        Arc::new(Self {
            vars: Mutex::new(BTreeMap::new()),
            parent: None,
        })
    }

    pub fn child(parent: &Arc<Env>) -> Arc<Env> {
        Arc::new(Self {
            vars: Mutex::new(BTreeMap::new()),
            parent: Some(parent.clone()),
        })
    }

    pub fn define(&self, name: impl Into<String>, binding: Binding) {
        lock(&self.vars).insert(name.into(), binding);
    }

    pub fn define_value(&self, name: impl Into<String>, value: Value, kind: BindingKind) {
        self.define(name, Binding::readonly(value, kind));
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(binding) = lock(&self.vars).get(name) {
            return Some(binding.value.clone());
        }
        self.parent.as_ref().and_then(|parent| parent.get(name))
    }

    pub fn get_binding(&self, name: &str) -> Option<Binding> {
        if let Some(binding) = lock(&self.vars).get(name) {
            return Some(binding.clone());
        }
        self.parent
            .as_ref()
            .and_then(|parent| parent.get_binding(name))
    }

    /// 沿作用域链赋值。
    ///
    /// `allow_module` 只在执行模块脚本（`@script`）时为真：模块级 `let` 是**同一次渲染
    /// 内所有组件共享**的，允许组件/模板改它会让渲染结果依赖遍历顺序；所以默认拒绝。
    pub fn assign(&self, name: &str, value: Value, allow_module: bool) -> Result<(), AssignError> {
        {
            let mut vars = lock(&self.vars);
            if let Some(binding) = vars.get_mut(name) {
                match binding.kind {
                    BindingKind::Root | BindingKind::Prop | BindingKind::System => {
                        return Err(AssignError::Immutable);
                    }
                    BindingKind::Module if !allow_module => {
                        return Err(AssignError::ModuleLocked);
                    }
                    _ => {}
                }
                if !binding.mutable {
                    return Err(AssignError::Immutable);
                }
                binding.value = value;
                return Ok(());
            }
        }
        match &self.parent {
            Some(parent) => parent.assign(name, value, allow_module),
            None => Err(AssignError::NotFound),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignError {
    NotFound,
    Immutable,
    ModuleLocked,
}

impl AssignError {
    pub fn message(&self, name: &str) -> String {
        match self {
            AssignError::NotFound => format!("未知变量 `{name}`"),
            AssignError::Immutable => {
                format!("不能给只读绑定 `{name}` 赋值（根命名空间 / props / 参数 / const）")
            }
            AssignError::ModuleLocked => {
                format!("不能从组件里改模块脚本的 `let {name}`；模块变量在一次渲染内由所有组件共享")
            }
        }
    }

    pub fn code(&self) -> &'static str {
        "E7105"
    }
}

/// 调用点提供的插槽：一份模板节点 + 捕获的调用点作用域。
///
/// 与“提前渲染成字符串”不同，thunk 让子组件可以用
/// `@slot body(item) { ... }` 把 `item` 传给父组件写的填充内容（作用域插槽）。
#[derive(Debug, Clone)]
pub struct SlotThunk {
    pub params: Vec<String>,
    pub body: Vec<Node>,
    pub env: Arc<Env>,
    pub span: Span,
}

/// 一次渲染的预算与计数器（放在 Renderer 里，不跨请求共享）。
#[derive(Debug, Default)]
pub struct BudgetState {
    pub output_bytes: usize,
    pub loop_iterations: usize,
    pub script_steps: usize,
    pub call_depth: usize,
    pub component_depth: usize,
}

impl BudgetState {
    pub fn new() -> Self {
        Self::default()
    }
}
