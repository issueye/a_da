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
- [x] 折叠状态与面板滚动位置在增量/整段同步后保持；仅接近底部时自动跟随 + 「回到最新」胶囊
- [x] **模型选择器**（v0.10）：模型药丸弹出按供应商分组的清单 + 推理等级（`provider.setActive` / `config.setEffort`），无 JS 可开合
- [x] 网关断开时聊天页保留壳并显示「未连接」+ 重新连接

### 2. 工作区

- [x] 侧栏工作区树：按 `workspace` 分组（与 desktop-ui 同口径）、当前标记、组内新建（`+`）/ 切换（✓）、空工作区占位
- [x] 搜索同时过滤工作区名与会话标题，工作区名命中整组展开
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

### 4. 动态前端插件体系

- [x] `frontend-plugins/<id>/plugin.json` 运行时扫描 + 热重载（`/extensions` 点重载，无需重启）
- [x] **只接受 TSX**：OXC 转译 + Boa 运行时，支持相对 import / export（ESM loader），不需要 Node/Bun
- [x] 独立页面（`/ext/{id}/{slug}`）、仪表盘卡片、静态资源（路径规范化 + 越界拒绝）
- [x] 插槽：`chat.corner` / `composer.left|right|footer|above` / `sidebar.top|bottom` / `topbar.right` / `transcript.after` / `extensions.list` / `dashboard.widgets`
- [x] 作用域隔离 CSS（`@scope ([data-plugin-scope])` + 指纹缓存）；自定义 JS 注入（defer）
- [x] 插件加载失败只影响自身，界面退回默认实现；可逐插件停用/启用；`GET /api/plugins`
- [x] 内置 TSX 组件库（Button / Chip / Notice / Panel / Modal / Form / Todo / 消息类等）
- [x] 安全校验：标签 / 属性名 / URL 三重校验、`hx-*` 显式白名单（拒 `hx-on*` 与 `js:` 值）、Boa 无 fs/net/process、循环/递归/输出/嵌套预算
- [x] 示例插件：`hello-tsx`（最小三件套）、`token-weather`（SSE `parts` 刷新 + 浮层）、`task-plan`（任务规划卡，`chat.corner` / `composer.above` 可挂）
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

## 二、部分实现（◐）

| 项 | 已有 | 缺口 |
|---|---|---|
| 连接状态 | 聊天页离线壳 + 重连 + 兜底轮询 | `/overview`、插件页等二级页仍直接显示错误页 |
| 审批档位 | 输入框只读药丸（完全访问=红 / 每次询问=琥珀） | 页内不可切换，需去桌面端设置 |
| 模式 | 顶栏/工具行只读展示 | 新建会话时不能选模式 |
| Composer 辅助区 | 工作区 / 审批档位 / 模型 / 发送 | 无附件、无 @ 提及、无斜杠指令 |
| Toast/Flash | flash 横幅（可手动关闭、按级别自动消失） | 无动作按钮、不堆叠，高频操作反馈弱 |
| 侧栏会话列表 | 页面加载时快照；顶栏运行状态是活的 | 其它客户端新建/变更的会话需手动刷新 |
| 图片输入 | 显示「含 N 张图片」标记 | 不渲染图片内容 |
| 侧栏形态 | 固定宽度、可收起 | 无拖动改宽、无子智能体树、无悬停删除/导出 |
| TSX 沙箱预算 | 循环 / 递归 / 输出 / 嵌套预算 | 无内存上限、无 wall-clock 超时 |

## 三、未实现（○）

| 项 | 说明 | 优先级 |
|---|---|---|
| 模式滑块 | 新建会话时选 coding / pm 等 | P1 |
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
- [ ] 新建会话时可选模式（coding / pm），开始后保持只读
- [ ] 审批档位页内切换（走网关已有的档位设置方法；切换有确认）
- [ ] 二级页（`/overview`、插件页）对齐聊天页的离线壳：保留布局 + 「未连接」+ 重连，不再显示裸错误页
- [ ] 侧栏会话列表增量刷新：新会话/其它客户端变更不再要求手动刷新
- [ ] 图片输入渲染：data URI 直接预览（安全上限：尺寸/格式白名单）

### P2（能力面）

- [ ] 附件（拖拽/粘贴上传）、@ 提及、斜杠指令
- [ ] TabStrip 多会话标签
- [ ] 右侧上下文 / 变更(Diff) / 进程面板
- [ ] 命令面板（Ctrl/Cmd+K）
- [ ] 插件市场：接入 `plugin-hub` 的发布与安装
- [ ] 侧栏拖动改宽（180–560px，对齐 desktop-ui）、子智能体树（缩进 12–16px、父节点可折叠）
- [ ] Toast 升级：动作按钮 + 堆叠，flash 保留为首屏反馈

### 技术债

- [ ] TSX 沙箱补内存上限与 wall-clock 超时（现只有循环/递归/输出/嵌套预算，见 [TSX.md](TSX.md) §5）
- [ ] Boa Context 池化 / 组件库预编译（当前每次渲染新建 Context，含组件库页约 3.4ms）
- [ ] 「测试连接」发起真实请求（需后端 `config.checkProvider` 增强，Web 端跟进表单与结果展示）
