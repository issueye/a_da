# ssr-h 提问浮窗（question.answer）开发计划

| 项 | 值 |
|---|---|
| 日期 | 2026-10-24 |
| 目标 | 在 ssr-h 落地「待答复」提问的操作面：托盘展示、作答回执（`question.answer`）、可由前端插件接管渲染 |
| 状态 | ✅ 已完成（2026-10-24）：阶段 1–4 全部落地；e2e 脚本（假模型发 `ask_user`）列为后续 |
| 备注 | 对应 [STATUS.md](STATUS.md) TODO P1 第 1 条，已划掉 |

## 1. 背景与现状

- 协议与后端**全部现成，不改**：`question.answer` 已在 agent-rpc dispatch（支持单选
  `choice` / 多选 `choices[]` / 文本 `text` / 多题 `answers[{id,choice,choices,text}]`）；
  ssr-h 经网关 WS relay 调用，与 `approval.decide` 同路径。
- 快照有两条数据源：① 当前会话 items 里的 `ask_user` 工具卡（status=`awaiting`，
  题目在 `details.question`）——desktop-ui 现役路径；② 快照顶层 `pendingQuestions`
  （无 threadId，跨会话条目）。
- 刷新链路半接好：`parts_fingerprint` 已监视 `pendingQuestions`（提问到达/离开会触发
  parts 重发），`render_parts_snapshot` 投影完整工具卡（awaiting 的 ask_user 卡本来
  就进 parts 渲染点）——**只缺渲染器与动作路由**。
- desktop-ui 参照：`AskUserFloatingPanel`（composer 上方浮层、一题一页、提交中防连点）、
  `question-data.ts`（单题/多题归一化，`allowText` 缺省 true，`multiSelect` 两种拼写）。

## 2. 设计决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | 操作面放在**输入框上方**（与审批/排队同属托盘区，`#ada-trays`），对话流里的 ask_user 卡只留记录；**提问卡本身是浮动覆盖**（2026-10-24 追加：`position: absolute; bottom: 100%` 锚在托盘区顶部，不占布局空间——desktop-ui 提问浮窗同形态；审批/排队维持 in-flow。容器因此不自滚动，长内容由各托盘内部滚动） | desktop-ui 硬约束「操作面唯一」；初版 in-flow 实测把对话流顶走（用户拍板改浮动） |
| D2 | 归一化在**核心**做（移植 question-data.ts 语义），插件只消费投影 | 与 todo 同原则：单题/多题摊平、选项归一这类语义只实现一次 |
| D3 | 数据源用**当前会话 items 的 ask_user/awaiting 卡**，不用顶层 `pendingQuestions` | 渲染点（首屏 / parts / live）本来就有当前会话 items；`pendingQuestions` 无 threadId，还得反查 |
| D4 | **不新增 capability**：`data.questions` 与 `data.todo` 同级直接注入 | 题目内容无敏感信息（providerAdmin 的 gate 是因为 headers 可能含凭据）；插件本就受信 |
| D5 | 默认实现是**核心 QuestionTray**；插件 `thread.tray` 插槽输出**非空则替换**提问段（审批/排队不受影响），插件停用/失败自动退回核心实现 | 插件系统既有原则「插件加载失败只影响它自己，界面退回默认实现」 |
| D6 | 多题**一次一题翻页**（desktop-ui 同款交互），实现为 `data-question-pager` 声明式契约：服务端渲染全部题目 + `hidden` 翻页条，`chat.js` 激活一次一题并按 callId 记忆页码；**无 JS 时翻页条隐藏、全部题目平铺**（阅读基线不丢）。**已填内容（勾选/文本/页码）在 parts 换帧与整页刷新后都保留**（sessionStorage 按 callId，提交后清除；可勾选控件按 name+value 对号、文本类按 name——两侧键规则一致，见 `stateFieldKey`） | 用户拍板（2026-10-24，初版"一表单全题平铺"实测版面过高，推翻 D6 原案）；契约行为核心实现一处，核心托盘与 `thread.tray` 插件共用（同 `data-ada-scroll-bottom` 先例），插件自己不写 JS |

## 3. 生产数据形状（提问事件 → 卡片 → 托盘）

`ask_user` 工具发出 `QuestionAsked`（`agent-toolkit/src/core/ask_user.rs`），
`set_tool_awaiting_question` 把它写进工具卡 `details.question`（store.rs）：

```jsonc
{
  "question": "首题文本",          // 多题时填首题（老渲染路径兜底）
  "choices": [{ "id": "c1", "label": "…", "description": "…" }], // id 引擎保底 c1,c2…
  "allow_text": true,             // snake_case；缺省 true
  "multi_select": false,          // snake_case；缺省 false
  "questions": [ { "id": "q1", "question": "…", "choices": [...], "allow_text": true, "multi_select": false } ] // 仅真多题时携带
}
```

作答回传口径与 desktop-ui 一致：`choice` = 选项 `id`（没有 id 用 label）；
无选项题可只发 `text`；「用户确认继续」= `text: "用户确认继续"`。

## 4. 表单字段约定（核心托盘与插件共用）

动作路由：`POST /questions/{call_id}/answer`，表单带 `hidden thread_id`（成功 303 回会话页，与审批托盘同款）。

| 场景 | 字段 |
|---|---|
| 单选 | `choice`（radio，同名互斥） |
| 多选 | `choices`（checkbox 同名多值） |
| 文本补充 | `text`（textarea，`allow_text` 时出现） |
| 多题 | 每题按题号前缀：`q_{id}__choice` / `q_{id}__choices` / `q_{id}__text`；路由拼成 `answers[{id,choice?,choices?,text?}]` |

规则：有 `q_` 前缀字段 → 多题路径，否则单题路径；全空提交 → flash 报错不发 RPC；
回执 `resolved=false`（已超时/已作答）→ flash「提问已失效」。

## 5. 阶段划分

### 阶段 1 — 核心链路

1. **`src/handlers/questions.rs`（新模块）**
   - `normalize_question(question_val, call_id)`：单题/多题摊平、choices 归一
     （`id` 缺省 `c{n}`、`label`/`value`/字符串项兼容）、`allow_text` 缺省 true、
     `multi_select` 兼容 snake/camel——语义对齐 desktop-ui `question-data.ts`；
   - `pending_questions_from_items(raw_items)`：逆序扫 `ask_user`+`awaiting` 卡取
     `details.question`，遇**非排队** user 消息停（排队消息不翻篇）；
   - `answer_params_from_form(form)`：表单 → `question.answer` 参数 JSON；
   - 单测：单题/多题/字符串选项/空提交/排队消息不截断。
2. **`src/agent.rs`**：`answer_question(call_id, params)` → `question.answer`。
3. **路由**（`src/main.rs` + `src/handlers/chat/mod.rs`）：
   `POST /questions/{call_id}/answer`，对照 `decide_approval` 的 303 + flash 行为。

### 阶段 2 — 核心提问托盘（默认操作面）

4. **`ui/components/trays.rs`**：`QuestionTray`——题干、选项（radio/checkbox）、
   `allow_text` 文本域、「提交回答」；无选项题出「继续」按钮；多题逐题分组。
5. **`src/handlers/chat/rows.rs`**：`question_trays_html(raw_items, thread_id)`，
   并入 `trays_html`（审批 → 提问 → 排队顺序）与计数。
6. **`counts.questions`**：`counts_view` 单列提问数（`awaiting` 的 ask_user 卡）。
7. **CSS**：`static/widgets.css` 补 `.ada-tray--question` 的选项/文本域样式。

### 阶段 3 — 插件化渲染

8. **`src/plugins/manifest.rs`**：新插槽 `thread.tray`（托盘区提问段）。
9. **`src/handlers/mod.rs` `plugin_data`**：注入 `data.questions`
   （归一化后的当前会话待答提问，含 `callId`/`askedAt`/`questions[]`）。
10. **`live_parts` + `load_chat`**：先渲染 `thread.tray` 插槽；输出非空替换核心提问段，
    否则用核心实现（D5 降级规则）。
11. **示例插件 `frontend-plugins/ask-user/`**：`slot.tsx` + CSS，
    演示 `data.questions` 消费与表单提交核心路由（文档标注：删除/停用即回核心托盘）。
12. **文档**：PLUGIN-DATA.md（`data.questions` 契约 + `thread.tray` 点位）、
    README.md 主界面一节、UED.md §7.6「提问」行、STATUS.md 划 TODO。

### 阶段 4 — 验证

13. `cargo build --manifest-path ssr-h/Cargo.toml`（`CARGO_BUILD_JOBS=2`、
    `CARGO_TARGET_DIR=../cargo_target_ada`——页面文件 2GB 限制）。
14. `cargo test --manifest-path ssr-h/Cargo.toml -- --test-threads=1`。
15. 手动验收路径：真网关 + 让模型调 `ask_user`，核对托盘出现/作答/消失、
    停用插件退回核心托盘；e2e 脚本扩展（假模型发 ask_user）列为后续。

## 6. 边界（本计划不做）

- 跨会话提问提示（只渲染当前会话的提问；其它会话的等待状态由侧栏 `waiting` 表达）；
- 一题一页翻页交互（需要 JS 状态机，留给插件 JS 增强）；
- verify 脚本自动化（假模型发 `ask_user` 的 e2e）；
- 提问超时计时器（引擎超时自动跳过，托盘随 parts 消失；只显示按 `askedAt` 的等待时长）。
