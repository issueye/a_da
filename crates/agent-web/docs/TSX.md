# TSX 运行时（服务端 SSR）

> 状态：**生产化原型**。`.tsx` 插件页面 / 插槽 / 仪表盘卡片已接入插件注册表；
> 支持 import/export（Boa 原生 ESM loader），不支持事件函数、hooks、hydration。交互仍走 HTMX / Alpine。

## 1. 数据流

```text
plugins/<id>/*.tsx
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
- 大写标签是**内置 TSX 组件**（`components.tsx`，也可以 `import { Button } from "ssr-h:components"`）：
  组件本身就是普通 TSX 函数，直接返回 VNode；Rust 侧只保留 `Icon` 这个 SVG 原语。
  写了没定义的大写标签会得到 `ReferenceError`，不是"被静默忽略"。
- 小写标签是内置 HTML 元素：**标签 + 属性名 + 属性值**三重校验（标签白名单、
  `^[A-Za-z_:][-A-Za-z0-9_:.]*$` 属性名、URL scheme）。
- `key` 对 HTML 标签是列表身份（丢弃不渲染），对组件是可读 prop：
  `<Disclosure key={item.id}>` 会落成 `data-key`，供 live 轮询还原展开态。
- 服务端不允许函数 prop、`onX`、`dangerouslySetInnerHTML`。
- 不支持 hooks、ref、事件、async、hydration；需要浏览器交互时用 `hx-*` / Alpine。

`props` 形状：

```json
{
  "plugin": { "id": "...", "name": "...", "version": "...", "description": "..." },
  "page":   { "slug": "...", "title": "..." },
  "slot":   { "name": "composer.left", "order": 10, "index": 0 },
  "app":    { "workspace": "...", "gateway": "...", "product": "...", "version": "..." },
  "route":  { "path": "...", "query": "..." },
  "data":   {
    "plugin_count": 3,
    "render": { "at": 1761000000000 },
    "tokens": {
      "available": true,
      "used": 12000,
      "window": 128000,
      "percent": 9,
      "cached": 9000,
      "cachePercent": 75,
      "cacheRate": 75,
      "cacheStale": false,
      "completion": 400,
      "thinking": 1200,
      "total": 12400,
      "tokensPerSec": 42,
      "durationMs": 9500,
      "threadId": "…",
      "threadTitle": "…",
      "model": "deepseek-chat"
    },
    "thread": { "id": "…", "title": "…", "workspace": "…", "mode": "code",
                "agentId": "ada-coding", "createdAt": 0,
                "running": true, "waiting": false, "isSubagent": false },
    "model": { "id": "…", "name": "…", "providerId": "…", "providerName": "…", "protocol": "openai_chat" },
    "config": { "approval": "ask", "effort": "high", "mode": "code", "contextWindow": 128000,
                "maxOutputTokens": 64000, "supportsImages": true, "retryCount": 5 },
    "workspace": { "project": "…", "current": "…", "publicWorkspace": "…" },
    "counts": { "threads": 6, "running": 1, "waiting": 0, "queue": 0, "approvals": 0 },
    "todo": {
      "done": 2,
      "total": 3,
      "allDone": false,
      "active": "写条目",
      "notes": "可选备注",
      "markdown": "### 任务规划步骤 (2/3)\n- [x] …",
      "steps": [{ "id": "…", "title": "…", "status": "pending|in_progress|completed" }]
    }
  }
}
```

`data` 的**完整字段表、类型与各渲染点的可用性**见 [PLUGIN-DATA.md](PLUGIN-DATA.md)。

`data.tokens` 是**当前会话的上下文用量与缓存**：最近一次模型调用如实报回的
`usage.promptTokens ÷ config.contextWindow`（与引擎的上下文治理同源，不做估算），
缓存命中是 `usage.cachedTokens ÷ promptTokens`。`percent` / `cachePercent` 为 `null`
表示算不出来（没有会话 / 窗口未配置 / 模型没报缓存字段）——插件要自己决定显示什么，
别当成 0%。**只有拿得到快照的渲染点才有这些字段**：聊天页（含每 2 秒的 live 轮询）、
插件独立页、概览卡片有；供应商/扩展等二级页的插槽只有 `plugin_count`。

`data.thread` / `data.model` / `data.config` / `data.workspace` / `data.counts` 是
为插槽准备的**会话与运行环境投影**（不含任何密钥）；`data.render.at` 是本次渲染时刻，
做"几秒前"这类相对时间用它，别信浏览器时钟。

`data.todo` 是**当前会话的任务规划**（`todo` 工具）推演后的结果：`set` 替换 /
`update` 按 id 增量合并 / `get` 只读，由 `ui/todo.rs::derive_todo_plan` 算好再投影。
没有规划过时是 `null`——插件据此不渲染卡片（不要画 `0/0` 的空面板）。

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

- `plugins/hello-tsx/`：页面 / 仪表盘 / JSON API（不占用输入框插槽）；
- `plugins/token-weather/`：**按上下文用量显示天气**（0-30% 晴 / 31-60% 多云 /
  61-85% 阴 / >85% 暴雨）。三处形态：独立页面、输入框上方胶囊（`composer.above`，随
  live 轮询每 2 秒刷新）、概览卡片。顺带演示相对 import（`components/weather.tsx` 三处共用）、
  `props.data.tokens` 数据契约（含 `cachedTokens` 缓存命中与 `M`/`K` 缩写），
  以及 `hx-get` + `hx-select` 做页面自刷新；
- `plugins/task-plan/`：**统一插件示例**。后端 `afterTodoUpdate` 维护计划状态，
  前端 `plan` action 读取并渲染任务卡；演示 `chat.corner`（会话区右上角）与
  `composer.above`（输入框上方）两处挂载点可切换，以及 `scripts` 注入的浏览器增强；
- `plugin.json` 里所有 `template` / `dashboard` 都必须是 `.tsx`；

### 自定义 CSS / JS

在插件目录放 CSS/JS（通常在 assets 目录下），在 manifest 里声明：

```json
{
  "assets": "static",
  "styles": ["hello.css"],
  "scripts": ["hello.js"]
}
```

```text
plugin.json
static/hello.css
static/hello.js
```

- `styles` / `scripts` 是相对 **assets 目录**的路径，分别只接受 `.css` / `.js`，
  不能是绝对路径或包含 `..`；
- 服务端把 `<link rel="stylesheet" href="/ext/{id}/assets/hello.css?v={fingerprint}">`
  与 `<script src="/ext/{id}/assets/hello.js?v={fingerprint}" defer>` 注入所有页面 `<head>`；
  CSS 会被包装为：
  ```css
  @scope ([data-plugin-scope="hello-tsx"]) to ([data-plugin-scope]) {
    /* 插件 CSS */
  }
  ```
  因此选择器只能命中该插件的包裹层内部，不会污染应用壳或其他插件；
- 插件页面/插槽/仪表盘内容会自动带上 `data-plugin-scope="<id>"` 包裹层
  （`display: contents`，不改变布局）；CSS/JS 改动后重载插件，指纹变化会刷新浏览器缓存；
- 浏览器不支持 `@scope` 时整段插件样式被忽略（安全降级，不会污染）；
- 请用插件内容 class 选择器，不要依赖 `:root`/`body`；`@import`/`@charset` 在作用域内无效，
  `@keyframes` 名称仍建议加插件前缀；
- 🔴 **HTML 白名单不开放 `<script>` 标签**：浏览器端行为只能走 manifest `scripts`（或 `hx-*` / Alpine），
  不要在模板里写内联 `<script>`；
- CSS/JS 是本地受信任内容，不做 sanitizer；不要从不可信目录安装插件。

## 5. 安全与预算

- Boa 环境**不注入** fs/net/process；插件脚本只能算数据；
- **wall-clock 预算**（2026-10-24）：每次渲染跑在独立大栈线程上，超时（环境变量
  `A_DA_TSX_BUDGET_MS`，缺省 500ms）即报错返回、渲染线程被放弃——循环/递归预算
  保证被放弃的线程最终自行结束，不会永久泄漏。正常渲染在个位数毫秒级；
- 循环 20,000 次、递归 64 层、输出 1 MB、VNode 嵌套 64 层；
- 文本默认转义；HTML 标签白名单；URL scheme 校验；
- **属性名必须过字符校验**（`^[A-Za-z_:][-A-Za-z0-9_:.]*$`）：值会被转义，但名字里出现
  `"` 或空格就能把 `data-x` 撑成 `data-x" onmouseover="…`，凭空造出一个事件属性。
  展开属性（`{...obj}`）是唯一能带任意属性名的入口，这里是它唯一的闸门。
- **HTMX 属性走显式白名单**（`hx-get/post/put/patch/delete/target/swap/trigger/include/…`）：
  `hx-on*` 等价于内联脚本（浏览器会把属性名小写化，所以写 `hx-ON:click` 一样能执行），
  `hx-vals` / `hx-headers` / `hx-trigger` 里的 `js:` 值会被 HTMX 求值——这些都拒。
- **没有内存上限**（Boa 0.22 的 GC 配置是私有的，作者留了 TODO）：`"x".repeat(1e9)`
  这类一次性大分配仍是 `abort`（`catch_unwind` 拦不住），输出 1 MB 的上限是**构造完
  之后**才检查的。wall-clock 能挡「慢模板拖住请求」，挡不住单次渲染内的瞬时大分配。
  真内存隔离需要 worker/子进程（可超时、可 kill），或等上游暴露堆约束。
- 插件仍是**本地受信任内容**；不要从不可信来源安装 `.tsx`。

## 5.1 调试：console.log 往哪里打

TSX 在**服务端**执行，所以：

- 输出进 **ssr-h 进程的日志**（tracing → stdout），**不是浏览器 DevTools**；
  `.\ssr-h.exe > log.txt` 或直接看终端即可；
- Boa 引擎**不自带 `console`**，这里注入了一个最小实现（只有 log/info/warn/error/debug，
  全部落到 `target=ssr_h::plugin` 的 tracing 日志），前缀是 `插件id/页面` 或 `插件id@插槽`：
  `INFO ssr_h::plugin: [token-weather/weather] props {...}`；
- 单次渲染最多 20 条、每条最多 2000 字符（插槽每 2 秒渲染一次，不设限会刷屏）；
  换行/制表符会被压成空格，避免把终端输出冲散；
- 想看 `debug` 级别：`RUST_LOG=ssr_h=debug`（默认 `ssr_h=info`）；
- 改完 `.tsx` 必须在 `/extensions` 点**重载前端插件**（模板是加载时编译并缓存的），
  否则跑的仍是旧代码——这也是"改了没反应"的常见原因。

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
# 本机（Windows）默认 target 目录编译 `ring` 会撞 MSVC D8050，复用缓存目录即可：
CARGO_TARGET_DIR=../cargo_target_ada CARGO_BUILD_JOBS=2 \
  cargo test --manifest-path ssr-h/Cargo.toml -- --test-threads=1
```

覆盖：TSX 转译/执行、组合组件、循环、fragment、转义、URL 校验、**属性名注入**、
**HTMX 白名单**、事件 prop 拒绝、`key → data-key`、上下文隔离、循环预算、
插件页面/插槽/仪表盘、热重载、相对模块 import。

## 8. 已知边界

- **没有 TypeScript 类型检查**：类型注解被 OXC 剥掉；原生组件属性是手工白名单，
  复合组件 props 没有运行时校验。后续从 `ComponentSpec` 生成 `.d.ts` + schema。
- **wall-clock 已有（2026-10-24），内存上限仍没有**：渲染跑在独立线程上，超时报错返回
  （线程被放弃，循环预算保证终止）；但 Boa 0.22 没有堆约束 API，一次性大分配
  （`repeat`、`Array.join`）在放弃判定到达前仍能逼近进程内存上限。真隔离需要
  worker/子进程（可超时、可 kill），或等上游暴露堆约束。
- **Context 每次新建**：约 1.7ms/页；复用可到约 0.6ms，但要先解决 Realm/全局状态隔离。
- **npm 依赖不支持**：内置 loader 只解析插件目录内的相对模块；需要 npm 包时必须在插件
  发布阶段预打包/内联，或后续增加显式的依赖安装流程。
- **事件/hydration 不支持**：TSX 只做服务端模板 + JS 逻辑；交互仍走 HTMX/Alpine。
- **部分组件是简化实现**：`Modal` 的 actions 尚未覆盖；`Form` 内请用 `<Button kind="submit">`，
  不要再用 `<Button action>`（会嵌套 form）。
- **路由上下文**：页面 props 的 `route.path` 已按 `/ext/{id}/{slug}` 覆盖；插槽/卡片沿用
  当前页面的 `render_context`（首屏之外的 live 轮询同样传真实上下文，不会再刷成空）。
- **`dashboard` 字段 vs `dashboard.widgets` 插槽**：前者是"这个插件的一张卡片"（清单字段），
  后者是"往概览的卡片区里再塞一段 HTML"（插槽）。两者都声明就会出两张卡——通常只用一个。
