# TSX 运行时（服务端 SSR）

> 状态：**生产化原型**。`.tsx` 插件页面 / 插槽 / 仪表盘卡片已接入插件注册表；
> 支持 import/export（Boa 原生 ESM loader），不支持事件函数、hooks、hydration。交互仍走 HTMX / Alpine。

## 1. 数据流

```text
frontend-plugins/<id>/*.tsx
  └─ OXC：去 TypeScript 类型 + JSX classic pragma（h / Fragment）
       └─ Boa：无宿主环境（无 fs/net/process），循环/递归/输出/深度预算
            └─ h(...) -> VNode JSON
                 └─ 内置 TSX 组件库：Button/Chip/… 就是普通 TSX 函数
                      └─ Rust：只保留 Icon SVG 原语 + 小写 HTML 白名单
                      └─ HTML
```

- 编译在插件**加载/重载**时发生，产物缓存在 `PluginRecord::tsx_templates`；
- 每次渲染新建 Boa Context：模块级状态不会跨请求泄漏（有测试）。
- release 单页基准（含 OXC/Boa/内置组件库）：单文件约 **5.2 ms**；
  import 模块页约 **3.4 ms**（OXC 预转译 + Boa ESM loader）。
  组件库每次新建 Context 都要初始化；复用 Context 可显著降低，但当前不做池化（隔离优先）。

## 2. 作者约定

文件必须声明一个全局函数 `Page(props)`：

```tsx
function Page(props: any) {
  return (
    <div class="card">
      <h2>{props.plugin.name}</h2>
      <p>工作区：{props.app.workspace}</p>
      <Chip text={"插件 " + props.data.plugin_count} tone="brand" />
      <Button label="打开对话" href="/" />
    </div>
  );
}
```

- **可以写 `import` / `export`**：加载/重载时 OXC 递归转译相对模块，Boa 的 ESM loader
  在服务端执行；**不需要 Bun/Node**。只支持插件目录内的相对模块，npm 依赖不支持。
- 大写标签是 Rust 组件；小写标签是内置 HTML 元素（属性白名单 + URL scheme 校验）。
- 服务端不允许函数 prop、`onX`、`dangerouslySetInnerHTML`；`key` 会被丢弃。
- 不支持 hooks、ref、事件、async、hydration；需要浏览器交互时用 `hx-*` / Alpine。

`props` 形状：

```json
{
  "plugin": { "id": "...", "name": "...", "version": "...", "description": "..." },
  "page":   { "slug": "...", "title": "..." },
  "slot":   { "name": "composer.left", "order": 10, "index": 0 },
  "app":    { "workspace": "...", "gateway": "...", "product": "...", "version": "..." },
  "route":  { "path": "...", "query": "..." },
  "data":   { "plugin_count": 3 }
}
```

## 3. 内置 TSX 组件库

`Button` / `IconButton` / `Chip` / `Notice` / `Panel` / `Disclosure` /
`EmptyState` / `Icon` / `TextField` / `Select` / `Modal` / `Form` / `Todo` /
`UserMessage` / `AssistantMessage`。

- 组件由 `src/tsx/components.tsx` 实现，通过 `import { Button } from "ssr-h:components"`
  或全局同名函数使用；组件属性有运行时白名单；
- 原生 HTML 标签的 class / href / src / form.action 走内置白名单的规范化与校验；
- Rust 只保留 `Icon`（复用 `ui/icons.rs` 的 SVG 路径）与 HTML 白名单，不再实现业务组件；
- `EmptyState` / `Notice` 的 children 会作为 action 区；`Modal` 暂不支持 actions 区；
  `Form` 内的提交按钮请用 `<Button kind="submit">`。

## 4. 插件示例

- `frontend-plugins/hello-tsx/`：页面 / 仪表盘 / `composer.left` 插槽；
- `plugin.json` 里所有 `template` / `dashboard` 都必须是 `.tsx`；

## 5. 安全与预算

- Boa 环境**不注入** fs/net/process；插件脚本只能算数据；
- 循环 100,000 次、递归 64 层、输出 1 MB、VNode 嵌套 64 层；
- 文本默认转义；HTML 标签/属性白名单；URL scheme 校验；
- 没有 wall-clock deadline：Boa 0.22 不提供中断 API，异常长计算仍可能占住线程。
  生产化需要 worker/子进程隔离或接受预算兜底。
- 插件仍是**本地受信任内容**；不要从不可信来源安装 `.tsx`。

## 6. 模块加载（import/export，原生实现，不需要 Bun/Node）

含 `import` / `export` 的 `.tsx` 在**插件加载/重载**时由 OXC 递归转译，运行时交给
Boa 的 ESM loader 执行：

```text
page.tsx + 相对依赖
  └─ OXC：逐个模块去类型 + JSX classic（h / Fragment）
       └─ 预转译表：相对路径 → JS
            └─ Boa ModuleLoader：按相对说明符取模块、parse/link/evaluate
                 └─ 模块命名空间 Page / default → h(Page, props)
```

- 入口模块必须 `export function Page(props)` 或 `export default`；
- 支持 `import { x } from "./x"`、`import "./x"`、`export { x } from "./x"`、
  `export * from "./x"`；扩展名可省略（会尝试 `.tsx/.ts/.js/.jsx` 与 `index.*`）；
- **只支持插件目录内的相对模块**；裸模块（npm 包）会在加载时报明确错误；
- 没有 Bun/Node 运行时依赖，也不需要构建期外部命令；加载/重载时转译一次并缓存；
- 不支持顶层 await、动态 `import()`、JSON import assert；
- 测试 `tsx_plugin_with_imports_loads_and_renders` 覆盖了 import + 组件 + 渲染。

如果以后要支持 npm 依赖，需要另做**显式的依赖包安装/发布流程**（或引入 Rust 侧打包器），
不应在请求路径里调用外部打包命令。

## 7. 测试

```bash
cargo test --manifest-path ssr-h/Cargo.toml -- --test-threads=1
```

覆盖：TSX 转译/执行、组合组件、循环、fragment、转义、URL 校验、事件 prop 拒绝、
上下文隔离、循环预算、组件属性校验、插件页面/插槽/仪表盘、热重载、相对模块 import。

## 8. 已知边界

- **没有 TypeScript 类型检查**：类型注解被 OXC 剥掉；原生组件属性是手工白名单，
  复合组件 props 没有运行时校验。后续从 `ComponentSpec` 生成 `.d.ts` + schema。
- **没有 wall-clock deadline**：Boa 0.22 没有 interrupt API；目前只有循环/递归/输出/嵌套预算。
  生产化需要 worker/子进程隔离，或接受预算兜底。
- **Context 每次新建**：约 1.7ms/页；复用可到约 0.6ms，但要先解决 Realm/全局状态隔离。
- **npm 依赖不支持**：内置 loader 只解析插件目录内的相对模块；需要 npm 包时必须在插件
  发布阶段预打包/内联，或后续增加显式的依赖安装流程。
- **事件/hydration 不支持**：TSX 只做服务端模板 + JS 逻辑；交互仍走 HTMX/Alpine。
- **部分组件是简化实现**：`Modal` 的 actions 尚未覆盖；`Form` 内请用 `<Button kind="submit">`，
  不要再用 `<Button action>`（会嵌套 form）。
- **路由上下文**：页面 props 的 `route.path` 已按 `/ext/{id}/{slug}` 覆盖；插槽/卡片仍沿用
  当前页面的 `render_context`。
