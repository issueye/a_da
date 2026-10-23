# 任务卡片（task-plan）

`todo` 工具的当前计划 → 会话区右上角的悬浮卡。**计划状态由核心推演，插件只负责呈现。**

## 形态

| 状态 | 表现 |
|---|---|
| 收起（默认） | 胶囊药丸：列表图标 + 「任务规划」+ `3/12` + 当前步骤 |
| 展开 | 320px 卡片：头部（标题 + `3 / 12 已完成` + 复制 + 收起）+ 备注 + 步骤清单（内滚） |

- 步骤状态：完成=绿勾 + 删除线；进行中=蓝箭头 + 脉冲 + 品牌浅底；待办=灰色空心圆；
- 「复制」输出 Markdown 清单（`- [x] 标题`）；
- 展开/收起用 `<details>/<summary>`：**没有 JS 也能开合**；JS 只记住偏好
  （`localStorage` 键 `a_da_todo_panel_collapsed`，与 desktop-ui 同口径）。

## 挂载点：右上角 / 输入框上方

`plugin.json` 里只改一行：

```json
"slots": [{ "slot": "chat.corner", "template": "slot.tsx", "order": 10 }]
```

| `slot` | 位置 |
|---|---|
| `chat.corner`（默认） | **会话区右上角**：贴在顶栏下方、浮在对话流之上，不占正文宽度 |
| `composer.above` | **输入框上方**：右下角悬浮区（`.ada-float-dock`），与 Token 天气同一条 |

改完在 `/extensions` 点「重载前端插件」即可，不需要重启 `ssr-h`。卡片样式对两个位置
都适用（宽度上限按视口收窄，见 `static/task-plan.css`）。

> 两个挂载点**不要同时声明**：同一份计划会渲染成两张卡。

## 数据契约（`props.data.todo`）

核心（`src/handlers/mod.rs::todo_view`）把推演后的计划投影成：

```json
{
  "done": 4,
  "total": 4,
  "allDone": true,
  "active": "复核提交结果",
  "notes": "可选备注",
  "markdown": "### 任务规划步骤 (4/4)\n- [x] …",
  "steps": [{ "id": "…", "title": "…", "status": "pending|in_progress|completed" }]
}
```

- 没有规划过（或当前会话拿不到 items）时是 `null` → 卡片不渲染，而不是画 `0/0`；
- `set` / `update` / `get` 的语义只在 `src/ui/todo.rs::derive_todo_plan` 实现一份，
  插件不要、也拿不到原始卡片去自行重演——那正是"面板停在旧计划上（7/12 vs 13/13）"的来源。

## 文件

```text
plugin.json          # 清单：插槽 / CSS / JS
slot.tsx             # 卡片模板（TSX，服务端渲染）
static/task-plan.css # 卡片外观（服务端包进 @scope，不污染应用壳）
static/task-plan.js  # 偏好记忆 + 复制按钮不误触开合
```
