# Slang 2（精简版）：SSR 模板 + props + 模块纯函数

> **一句话**：Slang 只做**服务端渲染**——把数据算好、把组件和插槽摆好；
> **浏览器交互全部交给 Alpine.js**（CSP 构建）。服务端不实现 setup、闭包、ref/computed、
> 事件回调或 lambda。
>
> 状态：**已实现**（`slang/2`）。脚本语法见 [SLANG-SCRIPT.md](SLANG-SCRIPT.md)。

| 项 | 值 |
|---|---|
| 入口 | `ssr-h/src/slang/` |
| 语言版本 | `slang/2`（唯一支持的版本） |
| 示例 | `ssr-h/frontend-plugins/hello-slang/` |
| 测试 | `cargo test --manifest-path ssr-h/Cargo.toml` |
| 客户端交互 | `static/alpine.min.js`（Alpine 3 CSP 构建，无需 `unsafe-eval`） |
| 关联 | [SLANG-SCRIPT.md](SLANG-SCRIPT.md)、[COMPONENTS.md](COMPONENTS.md)、[UED.md](UED.md) |

---

## 0. 结论摘要

Slang 的定位是**组件编排层**，不是通用编程语言，也不是前端框架：

1. **Rust 提供原子能力**：`Button` / `Chip` / `Disclosure` / `Panel` …、数据获取、权限、
   转义、URL 校验、CSP；
2. **Slang 负责服务端编排**：页面、插槽、业务组件、条件、循环、props、作用域插槽；
3. **Alpine 负责浏览器交互**：下拉、弹窗、开关、局部状态、客户端筛选；
4. **默认安全**：文本转义、属性白名单、URL scheme 校验、循环/输出预算、组件契约校验。

刻意**不做**的东西（因为 SSR 单次渲染 + Alpine 已覆盖）：

| 不做 | 原因 |
|---|---|
| `@script setup` / 组件实例脚本 | 组件只需要模板 + props；逻辑放模块函数或 Alpine |
| 闭包 / 函数值 / lambda | 服务端没有回调场景；事件在浏览器里 |
| `ref` / `computed` / `watch` | 每次渲染只跑一遍，`@let` 已足够 |
| `provide` / `inject` | 层级浅，显式 props 更清楚 |
| `emit` / `on_<event>` | 服务端同步回调没有消费者；交互走 Alpine |
| `v-` 指令 / 任意 JS | 无 JS 运行时，保持可静态校验、可预算 |

一句话：**服务端算一次，浏览器动起来。**

---

## 1. 设计目标与非目标

### 1.1 目标

- 运行时新增/修改页面、插槽、业务组件，**不重新编译 Rust**；
- 复用现有 `ui::components` 的 DOM/class/`data-*` 契约，视觉不漂；
- 编译期发现未知组件、未知属性、缺必填 props、未知插槽；
- props 有类型/默认值/必填校验；插槽支持作用域上下文；
- 默认转义、显式信任、有输出/循环/嵌套预算；
- 与 Askama、HTMX、Alpine、`guard.rs`、`markdown.rs` 共存；
- 失败隔离：一个插件/片段坏掉，不拖垮主应用。

### 1.2 非目标

- 不执行任意 JS/TS：没有 `eval` / `Function` / `import`；
- 不做客户端运行时：没有 DOM patch、hydration、事件绑定；
- 不做响应式：没有 ref/computed/watch/生命周期；
- 不做通用语言：没有 `class`、`while`、`try/catch`、`async/await`、函数值；
- 不做文件/网络/时钟/随机数：需要时间和数据就让宿主放进 `data`；
- 不替代 Askama 页面壳：Slang 负责插件页面/插槽/仪表盘卡片等“运行时内容”。

---

## 2. 文件模型

```slang
@slang 2

@script {
  // 模块作用域：每次渲染执行一次；被 @use 合并；组件只能读，不能改。
  function toneFor(count) {
    if (count > 0) { return "success"; }
    return "warn";
  }
}

@def StatCard {
  @props {
    label: String,
    value: Any,
    hint: { type: String, default: "" },
  }

  ::div(class: "card-inset") {
    ::p(class: "text-faint") { {{ label }} }
    ::p(class: "text-ink") { {{ value }} }
    @if hint != "" {
      ::p(class: "text-faint") { {{ hint }} }
    }
  }
}

@let plugin_count = data.plugin_count ?? 0;
::StatCard(label: "前端插件", value: plugin_count, hint: "运行时扫描")
```

| 块 | 位置 | 作用 |
|---|---|---|
| `@script { ... }` | 文件顶层 | 模块级纯函数/常量；所有组件共享、只读 |
| `@props { ... }` | `@def` 内 | props 契约（类型 / 默认值 / required） |
| `@def Name { ... }` | 文件顶层 | 组合组件：props + 模板 |
| `@use "x.slang"` | 顶层 | 导入 `@def` 与模块脚本，不输出内容 |
| `@include "x.slang"` | 顶层/块内 | 内联片段，共享当前作用域 |

- 文件顶层内容就是“文件级组件”（页面/插槽/仪表盘模板），没有 props；
- 被 `@use` / `@include` 导入的文件只贡献 `@def` 与模块脚本；
- `@def Name` **不接受参数列表**；props 只能写在 `@props` 里。

---

## 3. 模板语言

### 3.1 节点

| 类别 | 写法 | 输出 |
|---|---|---|
| 文本 | 普通文本 | 转义文本 |
| 插值 | `{{ expr }}` | 转义文本 |
| 组件 | `::Name(args) { children }` | 组件渲染结果 |
| 控制 | `@if` / `@for` / `@let` / `@include` / `@use` | 不直接输出 |
| 富文本 | `@html { ... }` / `@html(expr)` / `@md(expr)` | 受信任或清洗后的 HTML |
| 插槽声明 | `@slot name(exprs) [required] { fallback }` | 子组件渲染点 |
| 插槽填充 | `@fill name(vars) { ... }` | 只在组件调用块里合法 |

- 小写名 = 内置 HTML 元素（属性白名单）；大写名 = 注册组件；
- 找不到组件在编译期 `E3001`，并给最近候选；
- 元素属性有白名单；`script`/`style`/`iframe`/`on*` 不开放。

### 3.2 数据与作用域

| 根 | 内容 |
|---|---|
| `plugin` | 插件元数据（id/name/version/description） |
| `page` | 页面元数据（slug/title） |
| `slot` | 插槽元数据（name/order/index） |
| `app` | 应用信息（workspace/gateway/product/version） |
| `route` | 当前路由（path/query） |
| `data` | 宿主提供的 JSON |

- 根命名空间只读，所有组件可见；
- `@let name = expr;` 是**块作用域**；`@if`/`@for`/`@include` 各自一个块；
- `@for item in expr` / `@for (item, index) in expr`，支持数组/对象/字符串/null；
- 模板里读 props 用裸名字（`{{ title }}`）或 `props.title`；
- `slots.header` 是布尔值，表示调用点是否填充了该插槽。

### 3.3 表达式

```slang
{{ user.name }}
{{ user.name ?? "匿名" }}
{{ items[0].title }}
{{ data.plugins | len }}
{{ title | upper | truncate(20) }}
{{ data.connected ? "已连接" : "未连接" }}
{{ `共 {{ data.count }} 条` }}
```

- 支持：`+ - * / %`、`== != < <= > >=`、`&& || !`、`??`、三元、管道 `|`；
- 字符串 `"..."` / `'...'` / `` `...{{ expr }}...` `` / `"""..."""`；
- 数组 `[...]`、对象 `{ key: expr }`；
- **没有** lambda、函数值、方法调用（`items.map(f)` 不支持）；
- **没有**赋值/循环表达式/IO 函数。

**真值**：`false` / `null` / `0` / `""` / `[]` / `{}` 为假。
**相等**：标量按类型比较；数组/对象深比较。
**缺失路径**：返回 `null`，不报错。

### 3.4 内置函数

| 类别 | 函数 |
|---|---|
| 字符串 | `upper` `lower` `trim` `truncate` `split` `replace` `contains` `starts_with` `ends_with` `concat` `urlencode` |
| 集合 | `len` `join` `get` `keys` `values` `entries` `slice` `append` `reverse` `unique` `sort` |
| 默认值 | `default` `coalesce` |
| 序列 | `range(n)` / `range(start, end[, step])` |
| 序列化 | `json` |

全部是纯数据函数，不接受回调。

---

## 4. props 契约

```slang
@def Card {
  @props {
    title: String,                                   // 必填 string
    tone: { type: String, default: "neutral" },       // 可选 + 默认值
    count: { type: Int, required: true },             // 必填 int
    items: { type: "list", default: [] },
    level: { type: "enum", values: ["info", "warn"] },
  }
  ...
}
```

- 类型：`String` / `Int` / `Float` / `Bool` / `List` / `Object` / `Any` / `Url` / `Icon`
  / `Class` / `enum`（也接受小写/字符串形式）；
- 默认值必须是**字面量**；不参与作用域求值；
- props 既可通过 `props.title` 访问，也自动以 `title` 绑定；
- 位置参数按 `@props` 声明顺序绑定：`::Card("标题")`；
- 调用点：未知/重复 props → `E3002`；缺必填 → `E3003`；类型不符（运行期）→ `E3006`；
  显式传 `null` 视为空值，不做类型检查；
- `@def Name(...)` 参数列表已移除（`E2001`）。

---

## 5. 模块脚本

```slang
@script {
  const limit = 99;

  function clamp(n) {
    if (n > limit) { return limit; }
    return n;
  }

  function summary(items) {
    let done = 0;
    for item in items {
      if item.done { done = done + 1; }
    }
    return { done: done, total: len(items) };
  }
}
```

- 每个 `.slang` 文件一份（多个 `@script` 块按顺序拼接），每次渲染执行一次；
- 函数是**具名函数**，只在模块顶层声明；没有闭包、没有函数值、没有 lambda；
- 函数能读模块常量/`let` 与自己的参数/局部变量；**看不到组件 props 或调用点局部变量**，
  需要数据就通过参数传；
- 模块级 `let` 只能由模块脚本自身写；组件/模板调用函数时改它 → `E7105`；
- 没有 `while`；`for` + 预算保证终止；计数用 `range(n)`；
- 不允许表达式语句（没有 `emit` 这类副作用调用）。

---

## 6. 作用域插槽

```slang
@def Rows {
  @props { items: { type: "list", default: [] } }
  ::ul {
    @for (item, index) in items {
      ::li {
        @slot row(item, index) {
          {{ item.label }}
        }
      }
    }
  }
}

::Rows(items: rows) {
  @fill row(value, i) {
    {{ i + 1 }}. {{ value.label }}
  }
}
```

- `@slot name(expr, ...)` 里的表达式在**子组件作用域**求值；
- `@fill name(a, b)` 里的名字在**调用点作用域**绑定；
- `@fill` 声明的参数可以少于 slot 提供的参数（多余的忽略，缺的为 `null`），不能多；
- 默认插槽名固定为 `default`；裸子节点填它；必需插槽（`required`）在编译期检查；
- 原生组件的插槽仍是渲染好的字符串，不支持作用域参数。

---

## 7. 运行期架构

```text
Engine::compile
  TextParser ──► Program { nodes, defs, module_scripts }
      │
      ├─ Resolver：展开 @use / @include（合并 @def 与模块脚本）
      ├─ compile_script(@script) ──► functions + 顶层语句
      ├─ parse_props_decl(@props) ──► PropsDecl
      └─ Validator：组件 / props / 插槽 / 循环
                          │
                          ▼
                    CompiledTemplate

Engine::render
  build_roots(cx) ──► Env(root)
  Env(module) ──► 执行模块脚本
  Env(instance) ──► 绑定 props / slots
                 ──► render_nodes（块作用域逐个压栈）
                      ├─ ::Composite ──► 新实例 env + 插槽 thunk
                      ├─ ::Native    ──► 求值实参 + 立即渲染插槽
                      └─ @slot       ──► 调用 SlotThunk（绑定上下文变量）
```

### 7.1 关键类型

```rust
pub struct Env {
    vars: Mutex<BTreeMap<String, Binding>>,
    parent: Option<Arc<Env>>,
}

pub enum BindingKind { Root, Module, Prop, System, Local, Param }

pub struct SlotThunk {
    params: Vec<String>, // 调用点声明的变量
    body: Vec<Node>,
    env: Arc<Env>,       // 调用点作用域
}

pub enum Value { Null, Bool, Int, Float, Str, List, Map }
```

- 作用域链：root → module → instance → block → call；
- 函数调用创建 `Env::child(module_env)`，参数只读；函数局部 `let` 在更内层块；
- 没有跨请求状态；每次渲染新建整条作用域链。

### 7.2 预算

| 预算 | 默认 | 超限 |
|---|---|---|
| 脚本步数 | 100,000 | `E7103` |
| 函数调用深度 | 64 | `E7103` |
| 循环总迭代 | 10,000 | `E6001`（脚本 `E7103`） |
| 组件展开深度 | 32 | `E6001` |
| 渲染嵌套深度 | 64 | `E6001` |
| 输出字节 | 1 MiB | `E6001` |

---

## 8. 客户端交互：Alpine（CSP 构建）

服务端只输出 HTML；浏览器行为统一交给 Alpine：

```html
<div x-data="{ open: false }">
  <button type="button" x-on:click="open = !open">详情</button>
  <div x-show="open" x-cloak>…</div>
</div>
```

- 仓库内置的是 **Alpine 3 CSP 构建**（`static/alpine.min.js`），不依赖 `'unsafe-eval'`；
  当前 CSP `script-src 'self' 'unsafe-inline'` 因此可以直接工作；
- CSP 构建的表达式子集有限：复杂状态建议用 `Alpine.data('name', () => ({ ... }))`
  注册在 `/static/*.js` 或插件资产里，再用 `x-data="name"`；
- `::div` 的属性白名单**不包含** `x-*`（只放行 `data-*` / `aria-*` / `hx-*` 与普通属性）。
  在 `.slang` 里需要 Alpine 容器时，用 `@html { ... }` 显式写受信任 HTML：

  ```slang
  @html {
    <div class="widget" data-count="{{ data.plugin_count }}" x-data="pluginWidget">
      <button type="button" x-on:click="toggle()">详情</button>
      <div x-show="open">{{ data.hint }}</div>
    </div>
  }

  @* 在 /static/plugin-widget.js 里注册： *@
  @* Alpine.data('pluginWidget', () => ({ open: false, toggle() { this.open = !this.open; } })) *@
  ```

  - `@html` 里的 `{{ }}` 仍然转义；容器标签与 Alpine 属性原样输出；
  - **别在 `@html` 块里写内联对象字面量**（`x-data="{ open: false }"` 的 `}` 会被当成块结束）；
    用 `Alpine.data` 命名注册，或改在 `.html` 插件片段里写、用 `\}` 转义；
  - 更推荐把 Alpine 密集的交互放在 `.html` 插件片段（片段按本地受信任内容插入）。
- 服务端数据通过 `data-*` 属性交给 Alpine，或由 `.html` 片段自行 `fetch`；
- 插件页面/插槽是服务端内容，Alpine 在 HTMX 替换后会自动初始化新节点；
- 原则：**服务端只放“值和结构”，交互状态留在浏览器。**

---

## 9. 安全模型

| 能力 | Slang 2 |
|---|---|
| 读 `data` / `plugin` / `page` / `slot` / `app` / `route` | ✅ 只读 |
| 改根命名空间 / props / 模块 `let`（组件侧） | ❌ |
| 改局部变量 / 局部对象字段 | ✅ 只影响本次渲染 |
| 文件 / 网络 / 进程 / 环境变量 | ❌ 没有对应函数 |
| `eval` / `Function` / `import` | ❌ 语法里没有 |
| 时钟 / 随机数 | ❌ 保证确定性 |
| 产出 HTML | 只有 `@html` / `@md` 显式决定 |

- `{{ }}` 永远转义；文本节点默认转义；
- `@html(expr)` 在 `Sandboxed` 信任层走 `ammonia::clean`；`Local/Core` 视为作者显式信任；
- `@md(expr)` 始终经 `pulldown-cmark → ammonia`；
- 元素属性白名单；`href`/`src` 过滤 `javascript:` 等 scheme；
- 逐项预算，超限返回诊断，不 panic、不挂死。

---

## 10. 与宿主集成

- `Engine::compile_str` / `compile_file` / `compile` / `render` 签名不变；
- `RenderContext` 字段不变（`data` / `plugin` / `page` / `slot` / `app` / `route` / `trust` / `budgets`）；
- 原生组件 API 不变：`NativeRenderFn(&mut RenderCx, &Args, &RenderedSlots)`；
- `plugin.json` 的 `slang` 只接受 `2`；
- `.html` 插件片段完全不受影响；Alpine 可用于这些片段。

---

## 11. 已移除的旧语法

| 旧写法 | 现在 |
|---|---|
| `@slang 1` | `E2001`：只支持 `slang/2` |
| `@def Card(title, tone: "neutral")` | `E2001`：用 `@props { ... }` |
| `@script setup { ... }` | `E2001`：已移除；逻辑放模块 `@script` 或 Alpine |
| `defineProps` / `defineEmits` / `emit` / `ref` / `computed` / `provide` / `inject` | 已移除 |
| lambda `(x) => ...` | 语法错误 |
| 组合组件裸子节点填 `body` 插槽 | `E3004`；默认插槽固定为 `default` |
| `plugin.json` 的 `"slang": 1` | 插件加载失败 |

---

## 12. 错误码

| 码 | 含义 |
|---|---|
| `E1xxx` | 词法 |
| `E2xxx` | 模板语法（指令、块、参数、插槽、表达式） |
| `E3xxx` | 组件契约（未知组件/属性/插槽、缺必填、URL/属性非法、类型不符） |
| `E4xxx` | 表达式求值（未知标识符/函数、算术、比较、除零） |
| `E5xxx` | 导入与循环（路径越界、循环 include、组件循环） |
| `E6xxx` | 预算（输出、循环、嵌套、AST 节点） |
| `E71xx` | 脚本语法/校验（保留名、return/break 位置、未知函数、步数/深度） |
| `E72xx` | `@props` 契约（位置、schema、重复） |

---

## 13. 测试与验收

```bash
cargo test --manifest-path ssr-h/Cargo.toml -- --test-threads=1
cargo run --manifest-path ssr-h/Cargo.toml     # 打开 /ext/hello-slang/hello
```

示例页面应展示：`Slang / 2` 徽章、`MetricList` 的作用域插槽行、
`StatusChip` 通过模块函数算出的文案与色调。

---

## 14. 已知边界与后续

1. 没有响应式/更新循环：需要交互就用 Alpine；
2. 没有函数值/lambda：列表变换用内置纯函数（`sort`/`slice`/…）或模块函数；
3. `@props` 默认值只接受字面量；
4. 原生组件的插槽仍是字符串（不支持作用域参数）；
5. 组件层级需要共享上下文时，显式传 props 或放进 `data`（没有 provide/inject）。
