# ssr-h 功能现状与 TODO

| 项 | 值 |
|---|---|
| 日期 | 2026-10-24 |
| 范围 | `ssr-h/`（独立 SSR Agent 前端实验版） |
| 依据 | [README.md](../README.md)「当前实现边界」、[UED.md](UED.md) §11/§12 差距表、`src/main.rs` 路由实测 |
| 图例 | ✅ 已完成　◐ 部分实现　○ 未实现 |

> 维护约定：功能收口后把对应条目改状态并在 TODO 里勾掉；「有意不做」里的条目**不进 TODO**，
> 别当成 bug 或欠账去修。判断现状以代码为准，本文只是快照。

## 一、已完成

### 1. 对话主链路

- [x] 会话列表 / 详情（`/`、`/threads/{id}`）、统计概览（`/overview`）
- [x] 发送（Enter 发送 / Shift+Enter 换行 / 自动增高 / 空内容禁用 / 防重复提交）
- [x] **SSE 流式**：订阅网关 `evt.state.patch`，正文按 item 增量替换（约 25FPS 合帧）；
      托盘 / 任务规划 / 顶栏 / 输入框辅助区走 `parts` 事件（≥500ms）
- [x] 断流自愈：`EventSource` 自动重连 + 期间 2 秒轮询 `/threads/{id}/live` 兜底，重连后整段同步一次
- [x] 中止当前轮次（共享确认对话框）
- [x] 失败重试：`notice.level=error` 卡片带「重试这一轮」，动作失败回会话页 flash
- [x] 排队管理：可见 / 提升（中断当前回答，有确认）/ 删除 / 清空
- [x] 审批托盘：输入框上方 in-flow，带命令/参数预览与等待时长，批准/拒绝唯一入口，随事件流出现/消失
- [x] **提问作答**（2026-10-24）：待答复的提问在托盘展示题目/选项/文本补充并可提交
      （`POST /questions/{call_id}/answer` → `question.answer`）；核心托盘为默认实现，
      `thread.tray` 插件（示例 `ask-user`）输出非空时接管渲染，停用/失败自动回退；
      详见 [PLAN-question-panel.md](PLAN-question-panel.md)
- [x] Markdown 渲染（pulldown-cmark + ammonia 白名单，GFM）、代码块悬停复制
- [x] 思考 / 工具单行日志式（v0.5）：中文标签、目标对象、真实耗时、结果尾标、状态语义色、执行中活计时
- [x] **折叠状态与面板滚动位置在增量/整段同步后保持**；仅接近底部时自动跟随 + 「回到最新」胶囊
- [x] **打开会话默认置底**（2026-10-24）：去掉滚动容器全局 smooth（动画会被首帧同步/字体落定打断），
      boot / 首次整段同步 / load 落定三处强制瞬时置底；「回到最新」显式平滑
- [x] **模型选择器**（v0.10）：模型药丸弹出按供应商分组的清单 + 推理等级（`provider.setActive` / `config.setEffort`），无 JS 可开合
- [x] **审批档位页内切换**（2026-10-24）：工具行药丸点开四档弹层（当前项打勾禁用、完全访问带确认）→
      `POST /approval/mode` → `config.setApproval`；未连接保持只读药丸，换帧由 `replaceKeepingOpen` 保开合
- [x] 网关断开时聊天页保留壳并显示「未连接」+ 重新连接
- [x] **空会话页内选模式 + 工作区**（2026-10-24，取代原「新建会话」弹窗）：侧栏「新会话」
      直接建空会话；模式四档分段挂 `POST /threads/{id}/mode` → `thread.setMode`（开始后不可切），
      工作区药丸下拉挂 `POST /threads/{id}/workspace` → `thread.setWorkspace`（只绑定这条会话），
      「选择工作区」打开**弹窗目录选择器**（`partials/workspace_picker.html`，htmx 懒加载
      `/workspaces/picker`：盘符/常用位置标签、路径与上一级、进入子目录、新建文件夹、
      选定此工作区；无 JS 回退整页 `/workspaces?bind=<thread_id>`，表单按钮变「绑定到该会话」）。
      有第一条消息后控件消失；SSE 首帧/兜底轮询的空状态复用同一份富空状态，不会被换掉。
      视觉对齐 desktop-ui：模式四档带线性图标、收窄居中；工作区下拉与「选择工作区」合成
      一个胶囊控件（中间一条分隔线），菜单贴着药丸左侧展开
- [x] **原新建弹窗已移除**：侧栏新会话表单直接创建（当前工作区），`app.js` 的
      `data-new-thread` 拦截与弹窗视口 portal 一并删除
- [x] **图片输入**（2026-10-24）：输入框左侧「添加图片」/ 粘贴 / 拖入输入卡 → 预览与逐张移除 →
      随消息提交（同名 `images` 多值隐藏字段）。前端只做提前反馈，服务端 `collect_images`
      再校验：PNG/JPEG/WebP/GIF、单张 ≤8 MB（base64 后 12 MB 上限）、最多 4 张；
      发送路由的 body 上限放宽到 32 MB。对话流按固定缩略框渲染图片，点击在**当前页面浮层**
      查看（Esc/遮罩/关闭按钮关闭，锁滚动并归还焦点，不开新标签）；非图片 data URI 只报数量不渲染；
      纯图片消息允许不写文字。
      实机回归：`verify-ssr-streaming.mjs` 断言表单 → 模型请求 → 会话渲染全链路 + 非图片拒收

### 2. 工作区

- [x] 侧栏工作区树：按 `workspace` 分组（与 desktop-ui 同口径）、当前标记、组内新建（`+`）/ 切换（✓）、空工作区占位
- [x] **侧栏随事件流增量刷新**（2026-10-24）：`#thread-list` 在 SSE `parts` / 兜底轮询里整块换新，
      其它客户端的新会话/改名/运行状态点不再要手动刷新
- [x] **会话行内管理**（2026-10-24）：悬浮出重命名（行内表单，`thread.update`）与删除（确认弹窗，`thread.delete`）；
      删当前会话回首页，删其它会话留在当前页
- [x] 组折叠状态按 `data-ws-key` 记忆（`localStorage`，跨刷新保持）；搜索匹配工作区名称与完整路径
- [x] 搜索同时过滤工作区名与会话标题，工作区名命中整组展开
- [x] **二级页品牌位改为「返回会话」**（2026-10-24）：对话页保持 a_da 品牌，概览/插件页同一位置
      渲染「← 返回会话」（新增 `i-arrow-left` 图标），窄轨 title/aria 同步
- [x] `/workspaces` 目录浏览器：磁盘 / 主目录 / 最近工作区，浏览与新建目录（走网关 `fs.roots` / `fs.list` / `fs.mkdir`，不自己读盘）
- [x] `workspaces/select` 设为当前工作区并打开最近会话（没有则 `thread.create`）

### 3. 供应商管理（前端插件 `provider-admin`）

- [x] Master-Detail + 双页签（供应商信息 / 模型列表），页签是真实链接（`?provider=…&tab=…`）
- [x] 新建 / 保存连接（含代理、自定义请求头）/ 删除（至少保留一个）/ 设为当前（可带模型）
- [x] 模型增删改（按下标寻址，模型 id 可含 `/`），编辑与新增走弹窗
- [x] 获取远程模型：真打端点 `/models`（htmx 片段），弹窗展示、按厂商分组、就地搜索
- [x] 测试连接：`config.checkProvider`（仅参数校验，见「未实现」注记）
- [x] API Key 不回显（`已配置 + sk-…7f3a`，留空＝不修改）——与 desktop-ui 唯一的有意差异
- [x] 旧入口 `/providers` 303 跳插件页，动作端点保留回退
- [x] **界面优化**（2026-10-24）：页头「当前模型」可点药丸（取代两张信息大卡）、布局去写死高度、
      左列定宽 236px、≤900px 断点修正
- [x] **窄屏模型表溢出修复**（2026-10-24）：`.pa-page` 的 auto 轨道被表格 620px 的 min-width 撑开，
      窄屏下整张卡片溢出视口、表格横向滚动失效（首列文字与后列叠字）。改
      `grid-template-columns: minmax(0,1fr)` + 打通 `pa-body/layout/pane/table-scroll` 的
      `min-width:0` 链——卡片收在视口内，横向滚动回到 `.pa-table-scroll`，固定首列/操作列恢复生效

### 4. 动态前端插件体系

- [x] `frontend-plugins/<id>/plugin.json` 运行时扫描 + 热重载（`/extensions` 点重载，无需重启）
- [x] **只接受 TSX**：OXC 转译 + Boa 运行时，支持相对 import / export（ESM loader），不需要 Node/Bun
- [x] 独立页面（`/ext/{id}/{slug}`）、仪表盘卡片、静态资源（路径规范化 + 越界拒绝）
- [x] 插槽：`chat.corner` / `composer.left|right|footer|above` / `sidebar.top|bottom` / `topbar.right` / `transcript.after` / `extensions.list` / `dashboard.widgets`
- [x] 作用域隔离 CSS（`@scope ([data-plugin-scope])` + 指纹缓存）；自定义 JS 注入（defer）
- [x] 插件加载失败只影响自身，界面退回默认实现；可逐插件停用/启用；`GET /api/plugins`
- [x] **停用状态落盘**（2026-10-24）：`.ssr-h-state.json`（插件目录旁，只记停用 id），
      重启后保持；坏文件按全启用处理
- [x] **插件导航选中态**（2026-10-24）：插件页的 active 是 `ext:{id}`，侧栏「前端插件」链接按它点亮
- [x] 内置 TSX 组件库（Button / Chip / Notice / Panel / Modal / Form / Todo / 消息类等）
- [x] 安全校验：标签 / 属性名 / URL 三重校验、`hx-*` 显式白名单（拒 `hx-on*` 与 `js:` 值）、Boa 无 fs/net/process、循环/递归/输出/嵌套预算
- [x] 示例插件：`hello-tsx`（最小三件套 + JSON API）、`token-weather`（SSE `parts` 刷新 + 浮层；
      2026-10-24 品牌一致性对齐：emoji 换线性 SVG 图标（新增 i-cloud/i-cloud-fog/i-cloud-rain）、
      accent 走 `--ph-*` 语义对、去渐变/发光装饰）、`task-plan`（任务规划卡，`chat.corner` / `composer.above` 可挂）
- [x] **插件自带 JSON API**（2026-10-24）：manifest `api` 声明 → `/ext/{id}/api/{path}`，
      TSX 入口返回值直接序列化（不走 JSX）；一条静态分发路由 + 注册表热查找实现动态扩展，
      重载即生效；请求面在 `props.api`，方法白名单 GET/POST、体 ≤256KB
- [x] 任务规划数据契约：核心按 `set` / `update` / `get` 动作推演计划，插件只消费 `props.data.todo` 投影
- [x] 声明式核心行为契约：`data-ada-scroll-bottom`、`data-confirm` 等由 `chat.js` 事件委托承接

### 5. 安全

- [x] 默认仅回环监听；非回环必须显式 `--allow-remote` + `--ui-token`（≥12 位 ASCII），否则拒绝启动
- [x] token 换发 `SameSite=Strict` HttpOnly cookie 并 303 去除 URL token；`/__auth` 手动输入页
- [x] Host 白名单（防 DNS rebinding）、Origin / Sec-Fetch-Site 校验（防 CSRF，含 `Origin: null` 导航特例）
- [x] 安全响应头：CSP（`frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff`、`Referrer-Policy: same-origin`

### 6. UI 与工程化

- [x] 服务端组件库（`src/ui/`）+ 插件 TSX 组件库双层；图标唯一来源 `src/ui/icons.rs`（14–16px 线性 SVG）
- [x] 深浅主题（`localStorage` 优先，跟随系统兜底）；焦点态统一（品牌描边 + 3px 光环）
- [x] 响应式：≤760px 侧栏抽屉（汉堡 + 遮罩 + Esc）；宽屏侧栏收起/展开（存 `localStorage`）
- [x] 共享确认对话框（中止 / 删除供应商与模型等危险操作）
- [x] 窄屏与无 JS 基线：禁用 JS 仍可读主要内容、可开合关键交互
- [x] 实机验收脚本：`tools/verify-ssr-streaming.mjs`（流式）、`tools/verify-ssr-workspaces.mjs`（工作区）
- [x] **二级页离线壳**（2026-10-24）：概览/扩展断网时保留外壳 + 「未连接」+ 重新连接（`OfflinePage`），
      不渲染 0 值假统计
- [x] **焦点态口径统一**（2026-10-24）：文本框从 `:focus-visible` outline 组移出——浏览器对文本输入
      在鼠标点击时也匹配 `:focus-visible`，会叠出「硬描边 + 柔光环」两套焦点，与对话输入框观感不一致；
      现在统一走 `.ada-input:focus` 的品牌描边 + 3px 光环（与输入卡 / 下拉 / 搜索同源）
- [x] **运行状态指示**（2026-10-24）：输入框左上方（与 `composer.above` 插件同一带）的旋转弧 +
      「运行中」，随 SSE `parts` 的 `runStatus` 出现/消失（兜底轮询同源）；空闲时 `:empty` 不占位，
      `prefers-reduced-motion` 下停转。实机回归：`verify-ssr-streaming.mjs` 断言页面容器与 parts 下发
- [x] ~~窄断点弹窗被侧栏裁剪修复~~（2026-10-24 的弹窗 portal）——**已随「新建会话」弹窗移除而删除**：
      现在侧栏里没有任何视口级弹窗；选择挪到空会话页后不再有 transform 包含块的问题
- [x] **Black Swan 主题**（2026-10-24）：主调取自 `assets/logo.png`（宣纸暖白 `#F6F3EC` +
      墨黑 `#16171A`），蓝色主色退场。共享令牌 `design/tokens.css` 深浅两套替换、新增
      `--ph-on-brand`（暗色主按钮反转为米白底）、组件/插件里的 `#fff` 与蓝色硬编码清零、
      Tailwind 产物重建。设计说明与对比度见 [design/COLOR-SYSTEM.md](../../design/COLOR-SYSTEM.md)；
      desktop-ui 仍待 Phase 2（29 个文件的 `blue-*` 需语义迁移）
- [x] **产品 LOGO 统一为 `assets/logo.png`**（2026-10-24）：ssr-h 内嵌提供 `/static/logo.png`，
      侧栏品牌位 / 窄轨 / 空状态 / favicon 全部换用；desktop-ui 标题栏与空状态用 `LOGO_URL`
      （Vite `publicDir: '../assets'`，不复制第二份），favicon 同源；Electron 窗口/exe 图标沿用
      `assets/logo.ico`（electron-builder 已配置）

### 7. 交互改造 M0（止血批，2026-10-24）

按 [PLAN-interaction.md](PLAN-interaction.md) 的 M0 批次落地（对应
[INTERACTION-REDESIGN.md](INTERACTION-REDESIGN.md) 建议 04/05/06/08/09/11）：

- [x] **flash 文案单一真源**：`NOTICE_TEXTS` 表 + 源码扫描守门（`notice_literals_are_translated`），
      补上 `thread_renamed` / `thread_deleted` / `answered` 三条缺失文案（不再把英文 key 显示给用户）
- [x] **等待时长统一 `startedAt`**：审批与提问托盘都不再拿排序序号 `at` 当时间戳，与行内
      `elapsed_label` 同口径；拿不到 `startedAt` 就不显示（宁缺毋错）
- [x] **排队操作文本指纹**：提升/删除表单带 `expect`（渲染帧的文本），服务端比对当前队列，
      陈旧帧回 flash「排队已变化」而不是误伤相邻消息
- [x] **读屏可感知**：错误 flash `role="alert"`；托盘容器 `aria-live="polite"`
- [x] **触屏可达**：代码块复制、会话行重命名/删除在 `hover: none` 下常驻（对齐工作区组头的既有补丁）

### 8. 交互改造 M1（局部提交与状态保持，2026-10-24）

继续按 [PLAN-interaction.md](PLAN-interaction.md) 落地（T06/T07/T08/T11）：

- [x] **局部提交协议**（建议 21）：`form[data-partial]` → fetch 带 `Accept: application/json`，
      服务端按头分流返回 JSON 回执（不 303）；覆盖切模型 / 推理等级 / 审批档位 / 批准拒绝 /
      排队提升删除清空。页面不重建＝阅读位置、展开态、草稿都不动；成功后就地反馈并立即拉一帧
      `/live`，失败就地报错；**无 JS 或任何异常自动退回原生 303**（与改造前行为一致）
- [x] **发送草稿回填**（建议 03）：composer 草稿按会话存 sessionStorage（输入防抖 + 提交瞬间各写一次），
      发送成功（`?notice=sent`）清除；发送失败/刷新后回填，辅助区提示「已恢复未发送的草稿」
- [x] **阅读位置与展开态跨刷新**（建议 01/02）：`chatTop` 与展开的 `data-key` 按会话存 sessionStorage，
      动作回跳（URL 带 notice/error）或手动刷新时恢复；直接打开会话仍默认置底
- [x] **多审批汇总带**（建议 15）：同屏等待批准 ≥2 时显示「等待批准 ×N」；排队区限高此前已有（32vh 内滚）

## 二、部分实现（◐）

| 项 | 已有 | 缺口 |
|---|---|---|
| 连接状态 | ~~部分~~ → 二级页也已保壳（2026-10-24） | 插件页依赖数据缺失时走各自的降级提示 |
| Composer 辅助区 | 图片 / 工作区 / 审批档位 / 模型 / 发送 | 无通用附件（非图片）、无 @ 提及、无斜杠指令 |
| Toast/Flash | flash 横幅（可手动关闭、按级别自动消失） | 无动作按钮、不堆叠，高频操作反馈弱 |
| 侧栏会话列表 | ~~页面加载时快照~~ → 已随 parts 增量刷新（2026-10-24）；切换器与系统导航仍是整页快照 | 其它客户端改 agent 当前项目时侧栏切换器不跟 |
| 侧栏形态 | 固定宽度、可收起 | 无拖动改宽、无子智能体树、无悬停删除/导出 |
| TSX 沙箱预算 | 循环 / 递归 / 输出 / 嵌套预算 | 无内存上限、无 wall-clock 超时 |

## 三、未实现（○）

| 项 | 说明 | 优先级 |
|---|---|---|
| TabStrip 多标签 | 浏览器标签替代 | P2 |
| 右侧面板 | 上下文占用 / 变更(Diff) / 进程与后台任务 | P2 |
| 命令面板 | Ctrl/Cmd+K | P2 |
| 插件市场 | 接入 `plugin-hub` 发布/安装 | P2 |

**依赖后端/能力的注记**：

- 供应商「测试连接」：后端 `config.checkProvider` 目前只做参数校验、不发起真实请求；验可达性用「获取远程模型」。要做真测试需后端先行。
- 移除工作区：后端 `workspace.remove` 会连同该工作区会话文件一起删除，Web 端刻意不提供入口。

## 四、有意不做（不进 TODO）

- **不做移除工作区**：破坏性操作（连带删会话文件）。
- **不做登录鉴权体系**：非回环监听仅靠 `--ui-token` 一道门，且只提供明文 HTTP（TLS 交给反代）。
- **不做窗口控制/拖拽区**：浏览器标签即窗口。
- **插件不支持 npm 依赖**：只加载插件目录内相对模块。
- **插件内容按本地受信任内容处理**：不在不受信任目录放插件。

## 五、TODO

### P1（主流程状态面）

- [x] 提问浮窗（2026-10-24 完成）：核心提问托盘 + `thread.tray` 插槽 + `data.questions`
      投影 + `POST /questions/{call_id}/answer`；见 [PLAN-question-panel.md](PLAN-question-panel.md)
- [x] 侧栏会话列表增量刷新（2026-10-24 完成）：`#thread-list` 并入 parts/OOB，
      见 [README](../README.md)「工作区」一节
- [x] 新建会话时可选模式（2026-10-24 完成）：弹窗四档分段（编码/规划/创造/PM），
      随 `POST /threads` 的 mode → `thread.create`；开始后不可切
- [x] 审批档位页内切换（2026-10-24 完成）：工具行药丸点开四档弹层（当前项打勾禁用、
      完全访问带确认）→ `POST /approval/mode` → `config.setApproval`（后端落盘广播，parts
      随快照刷新）；未连接保持只读药丸
- [x] 二级页离线壳（2026-10-24 完成）：概览/扩展断网时保留外壳 + 「未连接」+ 重新连接
      （`OfflinePage`），不渲染假统计；插件页/工作区页本就按 null 数据降级
- [x] 图片输入与渲染（2026-10-24 完成）：选图/粘贴/拖入、预览移除、随消息提交、对话流缩略图；
      服务端格式与大小白名单，见「已完成 §1」
- [ ] 交互改造剩余项（见 [PLAN-interaction.md](PLAN-interaction.md)）：无 JS 服务端确认（T09）、
      AppError 统一 ErrorPage（T10）、provider-admin 脏检查与「测试连接」文案纠偏（T12）

### P2（能力面）

- [ ] 附件（拖拽/粘贴上传）、@ 提及、斜杠指令
- [ ] TabStrip 多会话标签
- [ ] 右侧上下文 / 变更(Diff) / 进程面板
- [ ] 命令面板（Ctrl/Cmd+K）
- [ ] 插件市场：接入 `plugin-hub` 的发布与安装
- [ ] 侧栏拖动改宽（180–560px，对齐 desktop-ui）、子智能体树（缩进 12–16px、父节点可折叠）
- [ ] Toast 升级：动作按钮 + 堆叠，flash 保留为首屏反馈

### 技术债

- [x] TSX 沙箱 wall-clock 超时（2026-10-24 完成）：渲染跑独立大栈线程，超时（`A_DA_TSX_BUDGET_MS`，
      缺省 500ms）报错返回、线程被放弃（循环预算收紧到 20k 保证终止）；输出上限已有。
      内存上限在 Boa 0.22 不可行（GC 配置私有、无中断 API），真隔离需子进程——见 [TSX.md](TSX.md) §5/§8
- [ ] Boa Context 池化 / 组件库预编译（当前每次渲染新建 Context，含组件库页约 3.4ms）
- [ ] 「测试连接」发起真实请求（需后端 `config.checkProvider` 增强，Web 端跟进表单与结果展示）
