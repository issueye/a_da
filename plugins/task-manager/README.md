# 任务管理（task-manager）

当前会话的命令活动，一处看全：**输入框上方的胶囊 + 浮窗**（`composer.above` 插槽，
与 Token 天气同构）。浮窗里有两段，数据来源**刻意不同**：

| 分区 | 数据 | 来源 | 可终止 |
|---|---|---|---|
| 后台任务 | `props.data.tasks`（`task.list`） | `run_background` 族的活体任务表 | 是（`POST /tasks/{id}/kill`） |
| 最近命令 | `props.data.runs`（`runs.read`） | 会话快照里的 `run_command` tool item | **否**（只读记录） |

为什么不是一个数据源：`run_command` 是**阻塞调用**，结束后不留任何活体（没有 pid、
没有可杀进程），所以它进不了任务表；但它留下的 tool item 就是权威记录，于是
「最近命令」直接从会话快照投影（不新增 RPC、不落盘、不缓存第二份）。

后台任务的输出变化由宿主**以事件方式推进来**（SSE `parts`），插件不轮询、不重演状态；
最近命令随会话事件一起刷新（有新 item 就有新一帧）。

## 结构

```text
plugin.json               # frontend.capabilities = tasks.read + runs.read；composer.above 插槽
slot.tsx                  # 胶囊 + 浮窗：后台任务（命令/PID/状态/输出/终止）+ 最近命令（命令/耗时/退出码/输出）
static/task-manager.css   # 作用域内样式（@scope 由宿主注入）
static/task-manager.js    # 浏览器增强：浮窗被整块替换后还原输出面板的滚动位置
```

## 形态

| 状态 | 形态 |
|---|---|
| 收起（默认） | 一枚胶囊：终端图标 + 「任务」+ 徽标（有后台任务时 `1/2` = 运行中/总数；只有命令记录时是记录条数） |
| 展开 | 浮窗：头部（标题 + 计数 + 口径说明）+ 分区清单 + 底部口径说明 |

- 浮窗是插件自己的 `<details class="ada-popover" id="task-manager-popover">`：
  SSE `parts` 每次整块替换后，`chat.js` 按 `id` 还原展开状态，点外面 / `Esc` 收起；
- **后台任务**每条：状态点（运行中带呼吸光环）、命令（等宽、超长省略）、PID、
  状态行（`运行中 · 12s` / `已结束 · 退出码 0`），输出预览收在行内 `<details>` 里
  （`id="tm-output-{taskId}"`，默认折叠）；只有运行中的才有「终止」按钮
  （`data-confirm` 走共享确认弹窗）；
- **最近命令**每条：命令 + 耗时（`5.1s`）+ 状态（`完成 · 退出码 0` / `已超时` / `已中止`）
  + 可选 cwd，输出预览在 `<details id="tm-run-output-{itemId}">` 里；
  **没有终止按钮、没有 PID**——前台命令结束后无进程可杀，画一个按钮只会误导；
- **两段都空时整个插件不渲染**：空闲会话里不该常驻一枚 0/0 的胶囊。

## 数据流

```text
后台任务
  run_background（agent 工具）
    → BackgroundTasks（实例态，按 threadId 记录）
    → task.list（RPC，按当前 threadId 过滤）
    → props.data.tasks → 插槽（SSE `parts` 每拍重渲染）
    → 用户点「终止」→ POST /tasks/{id}/kill → task.kill → BackgroundTasks::kill（整棵进程树）

最近命令
  run_command（agent 工具，阻塞执行）
    → 会话快照 tool item（kind=tool, name=run_command, args/output/details/status/durationMs）
    → agent-web `runs_view`（纯函数投影，按 threadId 的当前会话取最近 20 条）
    → props.data.runs → 同一插槽（无新 RPC、无第二份状态）
```

- 任务表是**实例态**，进程重启后为空；不写磁盘。
- `runs` 是快照的**派生视图**：老 item 被上下文压缩掉之后自然消失——这正是"最近"的语义。
- 界面上始终按当前会话（`threadId`）取数据；`task.list` 不带 `threadId` 时返回整个
  任务表（管理面用），插件不会走那条路。
- 同一 agent 进程里按工作区动态装配的引擎**共用一张任务表**；`run_background`
  仍按各自引擎的工作区做 `cwd` 沙箱校验。
- 后台任务输出预览截断到 4000 字符（完整输出用 `check_task`）；最近命令的
  `outputPreview` 截断到 1200 字符（一屏要挤 20 条），行数由 `outputLines` 给。


## 输出是怎么"推"过来的

任务表在 **agent 进程**里，网关不广播它，所以宿主侧（`agent-web` 的 SSE 事件循环）
按 1 秒问一次 `task.list`，用**指纹**（`id` / 运行态 / 退出码 / 输出长度）判断有没有变；
变了就补一帧 `parts`，浏览器端 `chat.js` 把 `parts.above` 写回 `#composer-above`。

- 只在**有任务在跑**时轮询；没有任务 / 任务都结束时不空转 RPC（表不会自己变），
  结构变化（新卡片、条目重取）后会强制重拉一次，新任务的胶囊第一拍就出现；
- 同一帧里已经取到的任务表会传给插槽渲染（`attach_tasks(..., Some(value))`），
  不会为了一帧打两次 `task.list`；
- 因此浮窗里的输出是"事件推过来的"，不用等 2 秒兜底轮询或整页刷新。

## 验收

```bash
# 渲染层（真插件 + 真 TSX）：后台任务与"最近命令"两段，且后者不许出现终止表单
cargo test -p agent-web task_manager

# 端到端：真网关 + 真 agent + 本地假模型
bun tools/verify-task-manager-e2e.mjs
```

- `cargo test` 那两条覆盖：后台任务有胶囊/浮窗/PID/终止表单；最近命令有命令、耗时
  （`durationMs` → `5.1s`）、退出码，且**没有** `/kill` 与 `PID`；
- e2e 脚本：模型调用一次 `run_background`，断言会话页的 `#composer-above` 里出现胶囊与浮窗、
  PID 与终止表单，再走 `POST /tasks/{id}/kill` 确认状态变为"已结束"。

## 相关

- [unified-package.md](../../docs/plugins/unified-package.md)
- [PLUGIN-DATA.md](../../crates/agent-web/docs/PLUGIN-DATA.md) §2.14（tasks）/ §2.15（runs）
- `crates/agent-toolkit/src/background.rs`（任务表与 `task.list`/`task.kill` 的数据源）
- `crates/agent-web/src/handlers/mod.rs` 的 `runs_view`（最近命令的投影，取自会话快照）
- `crates/agent-web/src/handlers/chat/stream.rs`（任务指纹 → `parts` 事件）
