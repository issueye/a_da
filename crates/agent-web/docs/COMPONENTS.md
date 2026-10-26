# ssr-h 前端组件库与插件扩展点设计（v0.6）

> 组件库（`ui/` + `static/components.css` + `static/components.js`）与插件插槽体系（`src/plugins/`）。
> 目标：**界面契约只有一处**、**插件异常不拖垮主应用**、**SSR 与客户端看到同一份事实**。
>
> 插件能拿到的数据字段与各点位可用性见 [PLUGIN-DATA.md](PLUGIN-DATA.md)。

---

## 1. 为什么这样做

改造前的状态（v0.5 及更早）：

- 同一段 UI（按钮、药丸、折叠行、输入框）在不同模板里各写一遍，改一处要改三处；
- 图标内联在模板里，主题色靠 `currentColor` 侥幸统一，但形状与描边没人管；
- 焦点态每个控件各写各的，视觉不��齐；
- 插件只能贡献"导航 / 页面 / 仪表盘卡片"三种东西，想在输入框旁边加个控件就得改主应用代码。

改造后的目标：

| 目标 | 做法 |
|---|---|
| UI 契约单一 | 组件在 **Rust 侧渲染**成 HTML，模板只摆放、不再逐个元素拼 class |
| 样式统一 | 一套 `ada-*` class + `--ph-*` 变量（`static/components.css`），不再有第二份页面级 CSS |
| 焦点态一致 | 组件库统一 `--ada-focus-ring`，所有可聚焦件共用一套外环 |
| 插件可扩展 | **插槽（slot）**机制：界面预留固定挂载点，插件往里放内容 |
| 插件不拖垮主应用 | 清单坏 → 记录为 `Failed` 并显示原因；片段坏 → 该插槽降级；不假装能兜住 panic |
| 热插拔 | 改插件目录 → 重载即生效；可单独停用/启用某个插件 |

---

## 2. 目录结构

```text
ssr-h/
├── src/
│   ├── main.rs                 # 路由、静态资源注册（新增 components.css/js、/api/plugins、启停路由）
│   ├── handlers.rs             # 处理器：数据投影 + 组件组装 + 插槽注入
│   ├── templates.rs            # Askama 视图模型（多为"已渲染好的片段"）
│   ├── agent.rs                # 网关客户端（未改动）
│   ├── markdown.rs             # Markdown → 安全 HTML（未改动协议）
│   ├── ui/                     # ★ 组件库
│   │   ├── mod.rs              #   对外 API（re-export）
│   │   ├── theme.rs            #   语义色 / 尺寸 / 类名 / 焦点态规范
│   │   ├── icons.rs            #   SVG 图标集（全局唯一来源）
│   │   ├── components.rs       #   组件实现（按钮/药丸/输入/折叠/消息/浮层/插槽…）
│   │   └── rows.rs             #   对话流的行视图与渲染
│   └── plugins/                # ★ 插件系统
│       ├── mod.rs              #   公共类型
│       ├── manifest.rs         #   plugin.json 解析与校验（新增 slots 字段）
│       └── registry.rs         #   注册表：生命周期、插槽、失败隔离、热插拔
├── templates/
│   ├── layout.html             # 二级页面外壳 + 插槽挂载点
│   ├── chat.html               # 对话页外壳 + 插槽挂载点
│   ├── partials/transcript.html# 只输出组件渲染好的行
│   └── ...
├── static/
│   ├── app.css                 # Tailwind 产物（构建自 plugin-hub/static/input.css）
│   ├── components.css          # 组件库·基础层（token/焦点/按钮/输入/折叠/消息/输入框）
│   ├── layout.css              # 组件库·布局层（外壳、左右两栏、供应商清单、模型表格）
│   ├── widgets.css             # 组件库·控件层（下拉选择、弹窗）
│   ├── shell.css               # 组件库·外壳层（复制按钮、侧栏、主区、托盘）
│   ├── pages.css               # 组件库·页面层（任务规划、插件悬浮区、响应式）
│   ├── ada-select.js           # ★ `<ada-select>` Web Component
│   ├── ada-modal.js            # ★ `<ada-modal>` Web Component
│   ├── components.js           # 组件库其余行为（折叠/复制/插槽注册表/输入框）
│   ├── app.js / chat.js / theme.js
└── frontend-plugins/<id>/
    ├── plugin.json             # 可声明 slots
    ├── slot.tsx                # 插槽示例
    └── ...
```

> 样式按层拆成 5 个文件，模板里的引用顺序必须是 **components → layout → widgets → shell → pages**，
> 这与拆分前单文件的串联顺序一致，层叠行为不变。新增样式请落进对应层，别再往基础层堆。
> `static/ssr.css` 已被组件库吸收并删除——**只保留一份页面级样式体系**是这次改造的硬约束。

---

## 3. 组件库

### 3.1 分层

```text
领域数据（agent 快照 / 插件清单）
        │  归一化
        ▼
   RowView / 视图参数            ← ui/rows.rs
        │  渲染
        ▼
   组件（Button / Chip / …）      ← ui/components.rs
        │  输出 HTML
        ▼
   模板 {{ x|safe }}              ← templates/*.html（应用壳）
        │  套上样式
        ▼
   components.css（ada-* + --ph-*）
```

> **运行时编排**：插件页面/插槽/仪表盘写 `.tsx`，由 OXC + Boa 在服务端执行；
> 组件库本身也在 `src/tsx/components.tsx`（TSX 函数，输出 `ada-*` class）。
> Rust 只保留 `Icon` SVG 原语与 HTML 白名单；应用壳的 Rust 组件不参与插件渲染。
> 见 [TSX.md](TSX.md)。

### 3.2 组件清单

| 组件 | 用途 | 关键 API |
|---|---|---|
| `Button` | 文本按钮 / 链接按钮 | `variant` `size` `kind` `icon` `href` `action` |
| `IconButton` | 纯图标按钮（强制 `aria-label`） | `icon` `label` `kind` |
| `Chip` | 状态药丸（模式/审批档位/模型） | `tone` `icon` `caret` |
| `TextField` | 单行输入 | `label` `hint` `icon` |
| `Select` | 下拉选择（**非**原生 `<select>`） | `option` `options` `value` `title` |
| `Tabs` / `TabItem` | 页签导航条（同一实体的多块内容切换） | `id`（DOM id 前缀） `label` `item` `active` |
| `ModelTable` | 供应商模型表：固定列宽、省略号 + `title` tooltip、行级操作菜单 | `ModelTable::new(provider_id, provider_name, rows)` |
| `ModelPicker` | 输入框的模型选择器：药丸 + 向上弹层（模型分组 / 推理等级） | `ModelPicker::from_providers(values, active_provider_id, model, effort, thread_id)` |\n| `ToolCall` | 工具调用卡片：折叠行 + 参数 + 长输出 12 行预览 / 展开全文 | `ToolCall::new(...)` |
| `ConfirmDialog` | 共享确认弹窗（**非** `<dialog>` / `window.confirm`） | `ConfirmDialog::markup()` |
| `Modal` | 通用弹窗（编辑表单等；同一套 `[data-modal]` 控制器） | `id` `title` `body` `actions` `wide` |
| `Disclosure` | 折叠行（工具/思考/压缩） | `icon` `label` `detail` `sub` `meta` `badge` `body` |
| `Panel` | 展开面板（标题栏 + 复制按钮 + 内容） | `title` `copy` `preformatted` |
| `UserMessage` / `AssistantMessage` | 消息行 | `text` / `html` |
| `Notice` | 提示卡（可带动作） | `tone` `action` |
| `EmptyState` | 空状态 | `icon` `title` `description` `action` |
| `Composer` + `ComposerAux` | 输入框与辅助区 | `left` `right` `aux` `focus_hint` |
| `ApprovalTray` / `QueueTray` | 审批 / 排队浮层 | — |
| `SendButton` | 发送主操作 | `disabled` |
| `Slot` | 插件插槽容器 | `name` `html` `fallback` `contributors` |

### 3.3 规范

- 🔴 **弹层类 UI 一律用组件，不用原生**：下拉用 `Select`（不写 `<select>`）、确认用 `ConfirmDialog`
  （不写 `<dialog>`，更不用 `window.confirm` / `alert`）、其它弹窗用 `Modal`。原生弹层样式不可控、
  会阻塞脚本、各浏览器长相不一，同一个"删除"在两处会是两种观感（2026-10-21 实拍过）。
  `Select` 用隐藏 input 承载表单值；`Modal` / `ConfirmDialog` 是 `<ada-modal>` 元素，
  开合、焦点圈、Esc、遮罩、滚动锁定都在**元素内部**实现。

### 3.4 Web Components 层（`static/ada-select.js` / `static/ada-modal.js`）

弹层类组件的**行为**已经元素化，服务端直接渲染自定义标签，浏览器解析后自动 upgrade：

| 元素 | 对应组件 | 对外 API |
|---|---|---|
| `<ada-select>` | `Select` | `openMenu()` `closeMenu()` `value`（读写）`isOpen`，事件 `ada:select` |
| `<ada-modal>` | `Modal` / `ConfirmDialog` | `open()` `close()` `isOpen`，事件 `ada:modal-open` / `ada:modal-close`；声明式入口 `data-modal-open="<id>"` / `data-modal-close` |

- **刻意不挂 shadow root**：页面是 SSR + htmx 的，样式、插件插槽、服务端渲染的 class 都在同一棵树；
  shadow DOM 会把它们整片隔离（还得把 `components.css` 复制进每个 root）。这里要的是**行为封装**，
  不是样式隔离——每个组件的监听器只挂在自己的元素上，不再靠 document 级委托 + `data-*` 猜。
- **模态栈**：`<ada-modal>` 自己维护打开顺序，只有栈顶响应 Esc / Tab 焦点圈，`body.ada-modal-open` 按栈深加/减。
- 没有 JS 时两个元素退化成普通行内标签，服务端渲染的内容照样可读，只是交互不可用。
- 兼容层：`window.AdaComponents.openModal/closeModal/modalOpen` 转发到 `window.AdaElements`，
  旧调用点（`app.js` 的确认框）不用改。

### 3.5 页签（Tabs）的契约

页签刻意拆成两半：**导航条归组件库，面板归页面模板**（面板里是页面自己的表单/表格，
组件不该知道它长什么样）。

- 导航条：`Tabs::new(id, label).active(key).item(TabItem::new(key, label, href).count(n))`，
  渲染 `role="tablist"` / `role="tab"`、`aria-selected`、`aria-controls` 与 roving `tabindex`；
- 面板：模板摆放
  `<section class="ada-tabs__panel" id="{id}-panel-{key}" role="tabpanel" aria-labelledby="{id}-tab-{key}" data-ada-tab-panel="{key}">`。
  当前页签**不渲染** `hidden`，其余渲染 `hidden`——由服务端按 `?tab=` 决定，所以首屏就是对的；
- 无 JS：页签是**真实链接**，点一下整页刷新到该页签（地址带 `?tab=`，刷新/收藏/分享都在同一页签）；
- 有 JS：`components.js` 的 `bindTabs` 就地切换面板，并 `history.replaceState` 成该页签自己的
  `href`（不新增历史记录，浏览器「后退」仍回到上一页）；方向键 / Home / End 在页签之间移动；
- 🔴 **弹窗必须放在页签容器之外**：面板带 `hidden` 时，里面的 `<ada-modal>` 会跟着不可见，
  「编辑 / 新增」会像点了没反应（供应商页的模型弹窗就是这样挪出去的）。

### 3.6 规范

- 🔴 **不要用 Tailwind 的任意值类写布局**（`lg:grid-cols-[264px_minmax(0,1fr)]`、
  `sm:grid-cols-[...]`）：`static/app.css` 是**预编译产物**，里面没有的类写了等于没写——
  供应商页就是这样一直静默地上下堆叠，而不是左右两栏。页面结构类一律落在 `components.css`
  （`.ada-master-detail`、`.ada-table-wrap` 等），模板只引用这些确定的类名。
- **类名**：`ada-` 前缀；状态一律用 `data-*`（`data-state="open"`），不拼 class；
- **颜色**：组件不写死颜色，只引用 `--ph-*`；**令牌只在 `design/tokens.css` 定义**
  （唯一真源，两个前端共用），组件库自己只定义 `--ada-*` 尺寸标尺；
- **标尺**：尺寸一律从 `components.css` 的 `:root` 取，不许写字面值。
  **所有令牌都是 2 的倍数**——两个令牌相加、相减仍落在同一网格，居中与栅格
  不会出现半像素，125% 缩放时也不会冒出 0.5px 的字形边缘：

  | 维度 | 档位 | 用途 |
  |---|---|---|
  | 字号 | 10 / 12 / 14 / 16 / 20 / 24 | 辅助 / 次要 / 正文 / 卡片标题 / 小节 / 页面 |
  | 圆角 | 6 / 10 / 14 / 20 + 胶囊 | 内联 / 控件与卡片 / 面板与弹窗 / 输入卡 |
  | 图标 | 12 / 14 / 18 | 行内 / 操作 / 强调（线宽恒 1.75） |
  | 间距 | 4 / 8 / 12 / 16 / 20 / 24 | 4px 网格，用于 padding / gap / margin |

  这条由 `tests/ui_scale.rs` 守住（12 个测试）：参数值是奇数 → 失败；
  CSS 里出现 `border-radius: 9px` / `font-size: 11.5px` → 失败；模板写
  `text-[11px]` 或内联 `style="font-size:..."` → 失败。报错会给出文件名与行号；
- **滚动条**：槽位 6px、1px 透明描边 → 可见 4px 圆条；三态色走
  `--ph-scrollbar / -hover / -active`（半透明混合，明暗自动适配）。
  所有滚动容器加 `overscroll-behavior: contain`（滚到底不把整页带走）；
  对话流与会话列表额外加 `scrollbar-gutter: stable`（长度跨过滚动阈值时不横移）。
  🔴 `thumb` 必须用 `background-color`——用 `background` 简写会把
  `background-clip` 重置成 `border-box`，4px 细条瞬间涨满整个槽位；
- **焦点态**：`.ada-focusable` 统一 2px 品牌描边 + 3px 半透明光环；组合式（输入卡）用 `:focus-within`；
- **转义**：所有动态文本经 `theme::escape`，组件只输出可信 HTML；
- **图标**：组件图标在 `icons.rs` 定义；聊天页组件已全走它，侧栏/顶栏/插件页模板仍有少量内联 SVG（待收口）。

---

## 4. 焦点态

| 场景 | 表现 |
|---|---|
| 输入框聚焦 | 品牌色描边 + 3px 光环；底部不再常驻快捷键提示（快捷键行为保留） |
| 组件可聚焦 | 统一 2px 描边外环（`:focus-visible`） |
| 折叠行 | 整行可点，键盘可达（原生 `<summary>`） |
| 打开状态 | 左侧品牌色条 + 品牌浅底 + 品牌色文字（不再依赖展开箭头） |

---

## 5. 交互调整：去掉展开箭头

需求"去掉工具/思考调用行前方的展开图标，点击整行即可展开/收起"已落地：

- 行内**不再渲染任何展开/收起图标**；
- 整行 `<summary>` 天然可点（鼠标、触摸、键盘 Enter/Space 均可）；
- 开合状态用三重信号表达：**左侧品牌色条** + **浅底背景** + **文字变深**，并同步 `data-state="open|closed"` 供样式与插件读取；
- SSE 增量替换（或结构变化时的整段同步）后，展开状态由 `components.js` 按 **`data-key`（item id）** 恢复；
- **面板内部滚动位置同样要还原**（`chat.js` 的 `restoreScroll`）：替换后的面板是新 DOM，
  `.ada-panel__body` 有 `max-height: 300px`，不还原就是"展开的工具每次更新自己滚回顶部"。
  位置按 `item id#面板序号` 记录，还原顺序必须是"先恢复 `details.open` 再设 `scrollTop`"——
  折叠状态下面板没有高度，先设会被夹成 0。

---

## 6. 输入框下方辅助功能区

`ComposerAux` 提供三段信息：

| 分组 | 内容 | 来源 |
|---|---|---|
| 状态提示 | 网关未连接、等待批准 N 项、排队 N 条（运行中只用旋转弧表达） | 真实快照 |
| 遥测 | 速度 / 缓存命中 / 上下文占用：**图标 + 数字**，完整口径在 `title` | 真实用量 |
| 快捷操作 | 中止（运行中）、清空队列（有队列时）、重试上一轮 | 真实路由（POST） |
| 插件插槽 | `composer.footer` | 插件 |

> 辅助区里的每个按钮都对应**真实可用的后端动作**，不放"看着能点其实没反应"的假控件。
>
> 布局：**一整行连续排列并居中**（状态 → 遥测 → 快捷操作 → 插件插槽），
> 不做左右分栏——分栏会把状态与遥测推成两段话。不再有常驻的快捷键提示行。

输入框工具行（左组 / 右组）：

| 位置 | 元素 | 规格 |
|---|---|---|
| 左 | `＋` | 纯图标，无底色（附件与提及尚未落地，标题里注明） |
| 左 | 工作区药丸 | 分支图标 + 路径末段（`.a-da`） |
| 左 | 审批档位药丸 | 盾牌图标 + 档位名 + 下拉箭头；完全访问=红 / 每次询问=琥珀 / 自动批准=中性 |
| 右 | 模型药丸 | 火花图标 + **完整模型名**（不截断）+ 下拉箭头 |
| 右 | 发送键 | 32px 圆形；空内容降级为浅灰禁用态 |

---

## 7. 插件扩展点

### 7.1 插槽清单

| 插槽 | 位置 |
|---|---|
| `sidebar.top` | 侧栏顶部（品牌区下方） |
| `sidebar.bottom` | 侧栏底部（插件导航之后） |
| `topbar.right` | 顶栏右侧操作区 |
| `chat.corner` | 会话区**右上角**的悬浮点位（贴右上角浮在对话流之上；只在聊天页） |
| `composer.left` | 输入框工具行左侧 |
| `composer.right` | 输入框工具行右侧 |
| `composer.footer` | 输入框下方辅助区 |
| `composer.above` | 输入框**上方**的悬浮点位（贴底浮在对话流之上） |
| `transcript.after` | 对话流末尾 |
| `extensions.list` | 扩展页列表 |
| `dashboard.widgets` | 概览页卡片区 |

插槽是**稳定契约**：界面保证这些位置一直存在；插件只负责往里放东西。

`composer.above` 的特别之处：它**浮在对话流之上**——用 `position: sticky; bottom: 10px` 吸在滚动容器底部，
滚动到底时自然落到内容末尾，不会一直挡着正文；容器 `pointer-events: none`，只有插件自己的卡片可交互。
适合放"当前任务状态""快捷统计"这类小卡片（`.ada-float` 外观已内置，插件可直接用）。

`chat.corner` 是同一类点位的"上方版本"：绝对定位在会话区右上角（顶栏下方），
给**常驻状态卡**（任务规划）用——它不该插进正文流，也不该占顶栏高度。
两个点位都随 SSE `parts` / 2 秒 live 轮询刷新（`parts.corner` / `parts.above`）。

### 7.2 清单格式

```json
{
  "id": "hello",
  "name": "Hello Frontend Plugin",
  "version": "0.2.0",
  "frontend": {
    "slots": [
      { "slot": "composer.left", "template": "slot.tsx", "order": 20 }
    ]
  }
}
```

- 顶层只放 `id/name/version/description`；页面/插槽/资源都在 `frontend` 段。
  旧版把 `slots` 等直接放顶层的清单仍兼容，但仓库自带插件已统一到新形状；
- `slot`：必须是清单里的合法插槽名，**写错在加载阶段就报错**（而不是"挂上去却没生效"）；
- `template`：相对插件根目录的 `.tsx`（页面/插槽/仪表盘都只接受 TSX）；
- `order`：同一插槽内排序，小的在前；
- `assets` / `styles` / `scripts`：静态资源目录（默认 `static/`）、要注入 `<head>` 的 CSS 与 JS
  （相对 assets，只接受 `.css` / `.js`；CSS 会被包进 `@scope`，JS 以 `defer` 加载）。

### 7.3 生命周期

```text
register → load → mount → render → unmount → destroy
   │        │       │        │
   └────────┴───────┴────────┴── 任一步失败：标记 Failed，降级为"不贡献内容"
```

| 阶段 | 做什么 | 失败处理 |
|---|---|---|
| register | 读清单、校验 id 与插槽名 | 整个插件跳过（其它插件不受影响） |
| load | 读插槽/页面/卡片片段进内存 | 该插槽降级为空，插件其余能力保留 |
| mount | 进入插槽表，对页面可见 | — |
| render | SSR 期拼进插槽 | **逐插件 `catch_unwind` 隔离**，panic 也只跳过它自己 |
| unmount | 停用/重载时摘掉贡献 | 界面回到默认实现 |
| destroy | 释放缓存（插件目录被删） | — |

### 7.4 热插拔

- `POST /extensions/reload`：重新扫描目录并做差分——新增→注册、有变更→重载、消失→销毁；**手动停用的插件不会被重载悄悄恢复**；
- `POST /extensions/{id}/enable` / `disable`：单个插件的装载/卸载，界面立刻生效；
- `GET /api/plugins`：插件与插槽的状态快照（客户端与 SSR 看到同一份事实）。

### 7.5 悬浮点位示例

`plugins/task-plan/` 是现成的例子：同一张任务卡片可以挂 `chat.corner`（会话区右上角，默认）
或 `composer.above`（输入框上方），改清单里的一行即可切换；计划状态由**同包后端插件**
通过 `afterTodoUpdate` 维护，前端优先读 `plan` action，核心 `props.data.todo` 作为降级；
浏览器增强（展开偏好、复制）由 `scripts` 注入的 `static/task-plan.js` 承担。

### 7.6 SSR / 客户端一致性

- 插件内容**在服务端渲染进 HTML**，首屏即可见，不依赖 JS；
- 客户端增强由插件自带的 `static/*.js` 承担（在清单 `scripts` 里声明，服务端注入 `<head>`，`defer`）；
- `components.js` 拉取 `/api/plugins`，把插槽贡献数写回 `data-contributors`，插件与调试面板据此工作；
- 插件片段是**本地受信任内容**（同目录约定），渲染前做体积上限与路径越界校验。

### 7.7 失败回退矩阵（已实测）

| 故障 | 现象 | 主应用 |
|---|---|---|
| `plugin.json` 语法错误 | 该插件被跳过并记录警告 | ✅ 正常 |
| 插槽名非法 | 加载阶段拒绝该插件 | ✅ 正常 |
| 片段文件缺失 | 插件仍可用，该插槽不贡献内容 | ✅ 正常 |
| 插槽渲染 panic | `catch_unwind` 兜住，跳过该插件贡献 | ✅ 正常 |
| 插件被停用 | 贡献消失，插槽回到默认实现 | ✅ 正常 |

---

---

## 10. 任务规划（todo 工具）卡片

任务清单**不插在正文流里**，也**不再由应用壳渲染**：它是 `plugins/task-plan/` 统一插件，
默认挂 `chat.corner`（会话区右上角）；把清单里的 `slot` 改成 `composer.above` 就换到输入框上方。
同包后端插件负责按会话维护计划状态（`afterTodoUpdate`），核心投影只作为降级。

### 10.1 计划状态：后端插件优先，核心降级

`plugins/task-plan/index.ts` 订阅 `afterTodoUpdate`：该点位拿到的是**已生效**的全量
`todos`（已经过 `beforeTodoUpdate` 的 deny/replace），后端按 `ctx.threadId` 写入
工作区 `.ada/task-plan-state.json`，再通过只读 action `plan` 返回给卡片。

后端停用/未就绪时，回落到 `ui/todo.rs` 的 `derive_todo_plan`，与 原桌面端 的
`utils/todo-plan.ts` 同一套语义：

| `action` | 含义 |
|---|---|
| `set` | **替换**整份计划（基线） |
| `update` | **增量合并**：按 `id` 命中就改，没命中就追加（无 id 时按标题匹配） |
| `get` | **只读**，不改计划状态 |
| 缺省 | 当作 `set` |

> 为什么不"倒序找最后一张能解析出 items 的卡片"：`update` 只带改动项（子集）、`get` 回的是旧快照，
> 那样面板会停在旧计划上（原桌面端 实测 7/12，正文却写着 13/13）。
> 这一点由 `ui/todo.rs` 里的 7 个单元测试锁住。

推演结果由 `src/handlers/mod.rs::todo_view` 投影成 `props.data.todo`：

```json
{ "done": 4, "total": 4, "allDone": true, "active": "…", "notes": null,
  "markdown": "### 任务规划步骤 (4/4)\n- [x] …",
  "steps": [{ "id": "…", "title": "…", "status": "pending|in_progress|completed" }] }
```

没有规划过时是 `null`，插件据此不渲染卡片（不画 `0/0`）。

### 10.2 界面（插件实现）

| 状态 | 形态 |
|---|---|
| 收起（默认） | 一枚胶囊药丸：列表图标 + 「任务规划」+ `3/12` 进度 + 当前步骤标题 |
| 展开 | 320px 卡片：头部（标题 + `3 / 12 已完成` + 复制 + 收起）+ 备注 + 步骤清单（最大 256px 内滚） |

- 步骤状态：完成=绿勾 + 删除线；进行中=蓝箭头 + 脉冲 + 品牌浅底；待办=灰色空心圆；
- 一键复制 Markdown 清单（`- [x] 标题` 形式；走组件库的 `[data-copy]` 委托）；
- 展开/收起用 `<details>/<summary>`：**没有 JS 也能开合**；插件 JS 只负责把偏好记进
  `localStorage`（与 原桌面端 同键 `a_da_todo_panel_collapsed`），并在 `parts` 替换后重新套用；
- 插件 CSS/JS 分别由清单的 `styles` / `scripts` 注入（CSS 包进 `@scope`，JS `defer`）；
- 工具行里展开 `todo` 工具时，同一份清单也会排版显示（核心 `TodoList`，与插件共用
  `.ada-todo__list*` 样式）。

## 8. 新增一个组件的做法

1. 在 `ui/components.rs` 加结构体 + `impl Render`（输出 `ada-*` class 与 `data-*` 状态）；
2. 在 `ui/theme.rs` 选语义色（`Tone`）与尺寸（`Size`），**不要写死颜色**；
3. 在 `static/components.css` 加对应 class（复用 `--ph-*` 变量与 `--ada-focus-ring`）；
4. 需要交互就在 `static/components.js` 加一段**渐进增强**（无 JS 时仍可读）；
5. 在 `ui/mod.rs` 导出。

## 9. 新增一个插槽的做法

1. 在 `src/plugins/manifest.rs` 的 `Slot` 枚举加一项（`as_str` + `parse`）；
2. 在 `src/handlers.rs` 的 `slot_content()` 里取一次，填进 `SlotContent`；
3. 在对应模板的挂载点输出 `{{ base.slots.<name>|safe }}`；
4. 插槽名写错会在插件加载时报错——**这是有意的**：契约变更必须显式。

