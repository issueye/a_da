# 任务管理（task-manager）

当前会话的后台命令任务面板。数据来自 `run_background` 族共享的
`BackgroundTasks` 实例，经 `task.list` 投影到 `props.data.tasks`。

## 结构

```text
plugin.json               # frontend.capabilities = tasks.read；sidebar.bottom 插槽
slot.tsx                  # 任务列表：命令 / PID / 状态 / 输出预览 / 终止
static/task-manager.css
```

## 数据流

```text
run_background（agent 工具）
  → BackgroundTasks（实例态，按 threadId 记录）
  → task.list（RPC，按当前 threadId 过滤）
  → props.data.tasks
  → task-manager 插槽
  → 用户点「终止」
      → POST /tasks/{id}/kill
      → task.kill（RPC）
      → BackgroundTasks::kill（整棵进程树）
```

- 任务表是**实例态**，进程重启后为空；不写磁盘。
- 界面始终按当前会话（`threadId`）取任务；`task.list` 不带 `threadId` 时返回整个
  任务表（管理面用），插件不会走那条路。
- 同一 agent 进程里按工作区动态装配的引擎**共用一张任务表**；`run_background`
  仍按各自引擎的工作区做 `cwd` 沙箱校验。
- 输出预览截断到 4000 字符；完整输出用 `check_task` 工具查。
- 插件只负责显示与提交终止；进程操作仍由 `agent-toolkit` 的后台任务表执行。
- 插件停用/读取失败时面板不显示（或显示错误），不影响核心工具。

## 验收

```bash
bun tools/verify-task-manager-e2e.mjs
```

真网关 + 真 agent + 本地假模型：模型调用一次 `run_background`，脚本断言会话页出现
任务面板、PID 与终止表单，再走 `POST /tasks/{id}/kill` 确认状态变为“已结束”。

## 相关

- [unified-package.md](../../docs/plugins/unified-package.md)
- [PLUGIN-DATA.md](../../crates/agent-web/docs/PLUGIN-DATA.md) §2.14
- `crates/agent-toolkit/src/background.rs`（任务表与 `task.list`/`task.kill` 的数据源）
