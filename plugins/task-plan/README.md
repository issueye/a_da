# 任务卡片（task-plan）

`todo` 工具的当前计划 → 会话区右上角的悬浮卡。**这是一个统一插件包**：
后端订阅 `afterTodoUpdate` 维护按会话的计划状态，前端读后端 `plan` action；
核心 `props.data.todo` 只作为后端停用/未就绪时的降级。

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

## 数据契约（后端优先，核心降级）

后端插件 `index.ts` 订阅 `afterTodoUpdate`，把这次**已生效**的全量 `todos`
按 `ctx.threadId` 写入工作区 `.ada/task-plan-state.json`，并注册一个只读动作 `plan`：

```ts
ada.registerAction({
  name: "plan",
  mode: "read",
  async execute(input) {
    // input.threadId ← SSR 预取时自动带上的当前会话
    return { ok: true, data: { done, total, allDone, active, markdown, steps } };
  },
});
```

`plugin.json` 的 `frontend.actions.plan` 声明 `read:true`，SSR 在首屏和每次
`parts` 刷新时调用，注入：

```jsonc
"data": {
  "backend": {
    "task-plan": {
      "plan": {
        "ok": true,
        "data": { "done": 3, "total": 4, "steps": [] },
        "error": null,
        "fetchedAt": 1761000000000
      }
    }
  }
}
```

前端优先用 `data.backend["task-plan"].plan.data`；后端停用/未就绪时回落到
`props.data.todo`（核心 `todo_view` 的投影）。`notes` 仍从核心投影补，因为
`afterTodoUpdate` 的载荷只有 `todos`，没有备注。

- 没有规划时是 `null` → 卡片不渲染，而不是画 `0/0`；
- 后端状态文件只存计划，不存会话消息；换会话按 `threadId` 分开；
- 核心 `derive_todo_plan` 仍是降级路径的真源，后端 action 不重演 `set/update/get`
  的卡片解析——它只接收 `afterTodoUpdate` 已经应用后的全量清单。

## 文件

```text
plugin.json          # 清单：frontend.slots/actions + backend
index.ts             # 后端：afterTodoUpdate → 工作区状态文件；plan action
slot.tsx             # 卡片模板（TSX，服务端渲染；后端优先 + 核心降级）
static/task-plan.css # 卡片外观（服务端包进 @scope，不污染应用壳）
static/task-plan.js  # 偏好记忆 + 复制按钮不误触开合
```
