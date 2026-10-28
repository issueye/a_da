# 任务卡片（task-plan）

`todo` 工具的当前计划 → 会话区右上角的悬浮卡。这是一个**纯前端插件**：
计划状态直接来自核心 `props.data.todo`，不再维护工作区状态文件。

> 2026-10-27：`beforeTodoUpdate` / `afterTodoUpdate` 已从核心移除。后续需要
> 拦截/旁注时，由统一事件系统（`event_*`）提供；本插件不再有后端入口。

## 形态

| 状态 | 表现 |
|---|---|
| 收起（默认） | 胶囊药丸：列表图标 + 「任务规划」+ `3/12` + 当前步骤 |
| 展开 | 320px 卡片：头部（标题 + `3 / 12 已完成` + 复制 + 收起）+ 备注 + 步骤清单（内滚） |

- 步骤状态：完成=绿勾 + 删除线；进行中=蓝箭头 + 脉冲 + 品牌浅底；待办=灰色空心圆；
- 「复制」输出 Markdown 清单（`- [x] 标题`）；
- 展开/收起用 `<details>/<summary>`：**没有 JS 也能开合**；JS 只记住偏好
  （`localStorage` 键 `a_da_todo_panel_collapsed`，与 原桌面端 同口径）。

## 挂载点：右上角 / 输入框上方

`plugin.json` 里只改 `frontend.slots`：

```json
"frontend": {
  "slots": [{ "slot": "chat.corner", "template": "slot.tsx", "order": 10 }]
}
```

| `slot` | 位置 |
|---|---|
| `chat.corner`（默认） | **会话区右上角**：贴在顶栏下方、浮在对话流之上，不占正文宽度 |
| `composer.above` | **输入框上方**：右下角悬浮区（`.ada-float-dock`），与 Token 天气同一条 |

改完在 `/extensions` 点「重载前端插件」即可，不需要重启 `ssr-h`。卡片样式对两个位置
都适用（宽度上限按视口收窄，见 `static/task-plan.css`）。

> 两个挂载点**不要同时声明**：同一份计划会渲染成两张卡。

## 数据契约

`props.data.todo` 是核心 `todo_view` 对最近一次 `todo` 工具调用的投影：

```jsonc
{
  "done": 3,
  "total": 4,
  "allDone": false,
  "active": "写测试",
  "notes": "",
  "markdown": "- [x] 读代码\n- [ ] 写测试",
  "steps": [
    { "id": "t1", "title": "读代码", "status": "completed" },
    { "id": "t2", "title": "写测试", "status": "in_progress" }
  ]
}
```

- 没有规划时是 `null` → 卡片不渲染，而不是画 `0/0`；
- 核心 `derive_todo_plan` 是唯一状态机，插件只负责渲染，不重演 `set/update/get`。

## 文件

```text
plugin.json          # 清单：frontend.slots
slot.tsx             # 卡片模板（TSX，服务端渲染；只读 props.data.todo）
static/task-plan.css # 卡片外观（服务端包进 @scope，不污染应用壳）
static/task-plan.js  # 偏好记忆 + 复制按钮不误触开合
```
