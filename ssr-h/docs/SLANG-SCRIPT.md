# Slang Script：模块纯函数 + `@props`

> **定位**：`.slang` 的服务端计算层。Slang 只保留**模块级具名函数/常量**与
> **`@props` 契约**；没有 setup、闭包、lambda、ref/computed、事件回调。
> 浏览器交互由 Alpine.js 负责（见 [SLANG-V2.md](SLANG-V2.md) §8）。
>
> 状态：**已实现**（`slang/2`）。

---

## 0. 一句话

**模块脚本放共用纯函数，`@props` 放组件契约，`@let` 放模板局部值；交互交给 Alpine。**

---

## 1. 两类声明

```slang
@slang 2

@script {
  const limit = 99;

  function clamp(n) {
    if (n > limit) { return limit; }
    return n;
  }
}

@def Card {
  @props {
    title: String,
    tone: { type: String, default: "neutral" },
  }

  ::Chip(text: title, tone: tone)
}
```

| | `@script` | `@props` |
|---|---|---|
| 位置 | 文件顶层 | `@def` 内 |
| 执行 | 每次渲染一次 | 不执行；编译期转成契约 |
| 内容 | 函数 / `const` / `let` | 类型 / 默认值 / required |
| 共享 | 所有组件 | 仅当前组件的调用点 |

- 一个文件可以有多个 `@script` 块，按出现顺序拼接；
- `@def` 不接受参数列表；props 只能写在 `@props` 里；
- `@script setup`、`defineProps`、`defineEmits` 已移除。

---

## 2. 函数语义

```slang
@script {
  function summary(items) {
    let done = 0;
    for item in items {
      if item.done { done = done + 1; }
    }
    return { done: done, total: len(items) };
  }
}
```

- 函数是**具名函数**，只在 `@script` 顶层声明；不能嵌套；
- 没有闭包、没有函数值、没有 lambda；调用只按名字；
- 函数可见：模块常量/`let`、自己的参数与局部变量；
- 函数**看不到**组件的 props、`@let`、循环变量或调用点作用域——需要什么就通过参数传；
- 参数只读；局部 `let` 可写；
- 递归受调用深度预算约束（默认 64）。

```slang
@def Badge {
  @props { count: Int }
  ::Chip(text: summary(data.items).done, tone: count > 0 ? "success" : "warn")
}
```

> 注意：`summary` 不能直接读 `count`；写成 `summary(data.items, count)` 才能拿到。

---

## 3. 语句

| 语句 | 语法 | 说明 |
|---|---|---|
| 变量 | `let name = expr;` | 可变 |
| 常量 | `const name = expr;` | 绑定不可重新赋值 |
| 赋值 | `name = expr;` / `a.b = expr;` / `a[i] = expr;` | 支持 `+= -= *= /= %=` |
| 条件 | `if cond { ... } else if ... else { ... }` | 条件可带括号 |
| 循环 | `for item in expr { ... }` / `for (item, i) in expr { ... }` | 数组/对象/字符串/null |
| 跳出 | `break;` / `continue;` | 仅循环内 |
| 返回 | `return expr;` / `return;` | 仅函数内 |
| 函数 | `function name(a, b) { ... }` | 仅 `@script` 顶层 |

**不允许**：`while`、`class`、`try/catch`、`async/await`、`import`、`eval`、
表达式语句（没有 `emit` 这类副作用调用）、给根命名空间赋值。

---

## 4. `@props`

```slang
@props {
  title: String,                                   // 必填 string
  tone: { type: String, default: "neutral" },       // 可选 + 默认值
  count: { type: Int, required: true },             // 必填 int
  items: { type: "list", default: [] },
  level: { type: "enum", values: ["info", "warn"] },
}
```

- 块体就是对象字面量；编译器转成 props 契约；
- 类型：`String` / `Int` / `Float` / `Bool` / `List` / `Object` / `Any` / `Url` / `Icon`
  / `Class` / `enum`（也接受小写/字符串形式）；
- 默认值必须是字面量；`required: true` 与 `default` 互斥；
- 一个组件只能有一个 `@props`，props 名不能与根命名空间/内置函数/关键字冲突；
- 绑定：每个 prop 以同名只读变量进入模板，同时可用 `props.title`；
- 位置参数按声明顺序绑定；
- 显式传 `null` 视为空值，不做类型检查。

调用点错误：

| 情况 | 码 |
|---|---|
| 未知/重复 prop、位置参数过多 | `E3002` |
| 缺必填 prop | `E3003` |
| 类型不符（运行期） | `E3006` |
| `@props` schema 非法/重复 | `E7202` / `E7204` |

---

## 5. `@let`：模板局部值

```slang
@let plugin_count = data.plugin_count ?? 0;
@let rows = [
  { label: "插件", value: plugin_count },
  { label: "产品", value: app.product },
];

::MetricList(items: rows)
```

- `@let` 是块作用域；`@if`/`@for`/`@include` 内声明的值不泄漏到外层；
- 用来缓存一次求值（比如 `len(items)`）或拼装传给子组件的数据；
- 不需要 `computed`：SSR 只渲染一次。

---

## 6. 表达式与内置函数

表达式复用 Slang 语法：

- 字面量、路径、`[]`、对象 `{ key: expr }`；
- `+ - * / %`、`== != < <= > >=`、`&& || !`、`??`、三元；
- 模板字符串 `` `{{ }}` ``；
- 管道 `value | filter(args)`；
- **没有** lambda / 函数值 / 方法调用。

内置函数（纯数据，不接受回调）：

| 类别 | 函数 |
|---|---|
| 字符串 | `upper` `lower` `trim` `truncate` `split` `replace` `contains` `starts_with` `ends_with` `concat` `urlencode` |
| 集合 | `len` `join` `get` `keys` `values` `entries` `slice` `append` `reverse` `unique` `sort` |
| 默认值 | `default` `coalesce` |
| 序列 | `range(n)` / `range(start, end[, step])` |
| 序列化 | `json` |

需要“按字段排序/过滤对象列表”时：数据量小就在模块函数里用 `for` 手写；
交互式筛选/排序交给 Alpine（在浏览器里做）。

---

## 7. 赋值与作用域

- 赋值沿作用域链找**最近**绑定；
- `const` / props / 参数不可重新赋值；
- 根命名空间（`data` / `plugin` / …）永不可写；
- 模块级 `let` 只能由模块脚本自身写；组件/模板调用函数时改它 → `E7105`；
- 函数局部 `let` 只影响本次调用。

---

## 8. 预算与错误

| 预算 | 默认 |
|---|---|
| 脚本步数 | 100,000 |
| 函数调用深度 | 64 |
| 循环总迭代 | 10,000 |
| 输出 | 1 MiB |

| 码 | 含义 |
|---|---|
| `E7101` | 脚本语法/位置错误（保留名、`return`/`break` 位置、嵌套函数、表达式语句） |
| `E7102` | 调用/类型错误（未知函数、参数个数） |
| `E7103` | 预算超限（步数/递归/循环） |
| `E7105` | 给只读绑定赋值（根命名空间、props、const、模块 `let`） |
| `E7202` | `@props` schema 非法 |
| `E7204` | props 重复声明/名字冲突 |

---

## 9. 示例：hello-slang

`scripts.slang`：

```slang
@slang 2
@script {
  function toneFor(count) {
    if (count > 0) { return "success"; }
    return "warn";
  }
  function labelFor(count) {
    if (count <= 0) { return "还没有插件"; }
    return `共 {{ count }} 个插件`;
  }
}
```

`components.slang`：

```slang
@def StatusChip {
  @props { count: { type: Int, default: 0 } }
  ::Chip(text: labelFor(count), tone: toneFor(count), icon: "i-plug")
}
```

`page.slang`：

```slang
@use "components.slang"

@let plugin_count = data.plugin_count ?? 0;
@let rows = [
  { label: "前端插件", value: plugin_count },
  { label: "产品", value: app.product },
];

::StatusChip(count: plugin_count)
::MetricList(items: rows) {
  @fill row(item, index) {
    {{ index + 1 }}. {{ item.label }} — {{ item.value }}
  }
}
```

---

## 10. 开放问题

| 问题 | 现状 | 后续 |
|---|---|---|
| 需要过滤/排序对象列表？ | 模块函数手写 `for`，或 Alpine 客户端处理 | 可加不带回调的 `sort_by(key)` |
| 需要响应式？ | 不做 | 交互交给 Alpine |
| `@props` 表达式默认值？ | 只允许字面量 | 可讨论编译期常量表达式 |
| 异步数据？ | 由宿主放进 `data` | 若引入 IO，另设异步边界 |
