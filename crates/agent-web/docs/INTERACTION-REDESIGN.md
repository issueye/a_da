# ssr-h 交互分析与改造建议

| 项 | 值 |
|---|---|
| 日期 | 2026-10-24 |
| 范围 | `ssr-h/` 全部交互面：浏览器脚本（`static/*.js`）、服务端渲染与路由（`src/ui`、`src/handlers`、`templates/`）、前端插件（`frontend-plugins/`） |
| 性质 | **分析 + 建议，未实施**；编号 01–20 供 STATUS/后续 commit 引用 |
| 依据 | 代码实测（各条附 file:line）、[UED.md](UED.md) §3 设计原则、[STATUS.md](STATUS.md) 已知 TODO |
| 基线 | 主链路（发送 / 流式 / 审批 / 提问 / 排队 / 重试 / 工作区树）已齐整；本文只谈「哪里还硌手」 |

---

## 1. 一页结论

交互模型本身是对的：**SSR 首屏 + 原生表单 POST → 303 回跳 + flash，动态面靠 SSE 三类事件增量换帧，JS 只做渐进增强**。
无 JS 基线、键盘可达、渐进披露这些"难而正确"的事都已经在做。剩下的毛刺集中在五类：

1. **整页跳转打断三件事**（阅读位置 / 展开态 / 草稿）——切一次模型，翻到一半的历史被拽到底、展开的工具全折回去、输入一半的正文清空（01–03）；
2. **反馈体系有洞**——三个动作成功后 flash 直接显示英文 key（04），错误 flash 读屏不播报（08），新审批/提问静默到达（09）；
3. **同一事实两个数字 / 陈旧帧误伤**——审批等待时长行内与托盘两套口径（05），排队操作按下标寻址无校验（06）；
4. **边缘环境不闭合**——无 JS 时危险动作零确认（07），触屏上复制按钮与行内重命名/删除不可见（11），表单校验失败落裸错误页（10）；
5. **文档滞后**——审批档位页内切换已在代码落地，STATUS 仍标未完成（见 §5）。

---

## 2. 交互模型速写（现状）

```text
首屏 SSR ──┬─ 动作面：原生 <form POST> → 303 回会话页 + ?notice=/?error= → flash 横幅
           │    （发送 / 中止 / 重试 / 审批 / 提问作答 / 排队 / 模型与档位 / 会话与工作区管理）
           └─ 动态面：GET /threads/{id}/stream（SSE）
                ├─ transcript  结构变化整段换 #transcript
                ├─ item        单卡原位替换（流式正文）
                └─ parts       托盘/角标/顶栏/输入框辅助区/侧栏列表 按键换帧（≥500ms）
           断流：EventSource 自动重连 + 2s fetch 轮询 /live 兜底（收到真事件才停轮询）
```

换帧时已经保住的东西（**改造时别弄坏**）：`details` 展开态按 `data-key`（components.js:20）、
展开面板内部滚动位置（chat.js:85-115）、插件气泡开合按 `id` 还原（chat.js:603-618）、
提问表单草稿按 callId 进 sessionStorage（chat.js:394-530）、工作区分组开合进 localStorage（chat.js:554-582）、
侧栏列表滚动位置（chat.js:304-311）。

---

## 3. 问题清单

> 每条：现象 → 证据 → 影响 → 建议。🔴 表示"正确性/信任问题"，其余为体验问题。

### A. 整页跳转打断状态

**01 🔴 阅读位置被打断：composer 周边动作全量 303，回跳后无条件置底**
切模型、切推理等级、审批决定、切审批档位、排队提升/删除/清空，全部是 `redirect()` 回会话页（src/handlers/chat/mod.rs:937、957、1000、1126-1150，src/main.rs:211）。回跳即页面重建，chat.js 的 boot 与首次整段同步都**无条件置底**（static/chat.js:176-190）。用户正翻到历史中段批准一个请求，回来发现自己被拽到最底部。
建议：短期在回跳 URL 上带锚点/偏移标记（如 `?pos=<scrollTop>` 或 `#ada-keep`），chat.js 见标记时走 `restoreScroll` 而非置底；中期见 21（局部提交）。

**02 展开态不跨刷新**
工具/思考的展开状态只存 JS 内存（static/components.js:20 `openKeys`，Object，不落 storage）。整页跳转（01 触发）或手动刷新后全部折叠。对比：工作区分组（chat.js:554-582）与任务卡（frontend-plugins/task-plan/static/task-plan.js:13）都存了 localStorage。
建议：`openKeys` 落 sessionStorage（按 threadId 命名空间），容量顾虑时只存最近 N 条。

**03 🔴 草稿三处全丢**
输入框 `<textarea>` 服务端渲染恒为空（ui/components/composer.rs:129，无 value 回填）；切模型/档位等任何 POST（01 清单）后草稿清空；**发送失败**只带 `?error=` 回跳（src/handlers/chat/mod.rs:975-988），长文一次全丢——这是全清单里用户损失最大的单点。
建议：① 发送失败时把原文带回回跳页回填 composer（query 或一次性 sessionStorage）；② composer 加通用草稿保存（input 防抖写 `a_da_draft:{threadId}`，submit 成功清除，首载回填）。①优先，②跟随 21 一起做收益最大。

### B. 反馈体系

**04 🔴 三个 notice 无文案映射，flash 显示英文裸串**
`flash_from_query` 未登记 `thread_renamed` / `thread_deleted` / `answered`（src/handlers/mod.rs:167-206，兜底 `other => other.to_string()`），而三者确会发出（src/handlers/chat/mod.rs:1072、1104、1189）。用户重命名会话后看到横幅写着 `thread_renamed`。
建议：补三条中文文案（`已重命名会话` / `已删除会话` / `已提交回答`）；顺手给「新增 notice 必须登记文案」加一条守门（测试枚举 `redirect(...notice=…)` 的字面量比对，或至少写进 PR checklist）。

**05 🔴 审批等待时长两套口径**
行内 `elapsed_label` 明确拒绝用 `at` 算耗时——注释写着「它是排序用的序号，不是时间戳」（src/handlers/chat/rows.rs:348-364，改用 `startedAt`）；而审批托盘 `waited_label` 恰恰拿 `at` 当毫秒时间戳（src/handlers/chat/rows.rs:851-854 → ui/components/trays.rs:173-192）。同一张等待批准的卡，行内显示「待批 3s」、托盘可能显示离谱的「已等待 …」。
建议：托盘改用 `startedAt`，与行内同源；`waited_from_at` 保留但只喂真实时间戳。

**06 🔴 排队提升/删除按下标寻址，无服务端校验**
`POST /threads/{id}/queue/{index}/promote|remove` 直接用渲染帧里的快照下标（src/handlers/chat/mod.rs:1207-1229；按钮在 ui/components/trays.rs:209-243）。SSE 换帧间隙里另一个客户端删了一条，陈旧帧上点「删除第 2 条」就误伤现在的第 2 条。对比：供应商模型编辑用下标但有越界防护与专门提示「可能已被其他客户端改动，请刷新」（src/handlers/providers.rs:724-730），两边口径不一致。
建议：表单里带消息文本指纹（hidden `expect`），服务端不匹配时回 flash「排队已变化，请确认后再操作」；根治是排队项暴露稳定 id（依赖后端，列为跟进项）。

### C. 边缘环境（无 JS / 触屏 / 读屏）

**07 🔴 无 JS 时危险动作零确认**
`data-confirm` 完全由 JS 捕获 submit 实现（static/app.js:88-105）；`<noscript>` 下删会话（连带删盘上文件，ui/sidebar.rs:283）、清空队列、删供应商/删模型的表单一按即发、不可恢复。
建议：高危 POST 服务端要求 `confirm=1` 参数；无 JS 渐进增强路径下首次提交渲染一个带确认按钮的中间页（复用 ErrorPage 外壳）。JS 在场时照旧走共享确认弹窗，多带一个 hidden 字段即可。
同口径问题：「中止当前轮次」带 data-confirm（src/handlers/chat/mod.rs:189、310），同为打断性的「重试上一轮」在＋菜单里裸提交（src/handlers/chat/mod.rs:88）——重试会丢弃当前轮已产出的内容重来，值得同款确认。

**08 错误 flash 用 role="status"，读屏不播报**
flash 不分级一律 `role="status"`（templates/chat.html:57、templates/layout.html:49）。错误发生时屏幕阅读器用户得不到任何提示。
建议：`kind == "error"` 时输出 `role="alert"`（或 `aria-live="assertive"`），其余保持 status。

**09 新审批/提问静默到达**
托盘容器 `#ada-trays` 无 aria-live（templates/chat.html:120）。agent 停下来等批准/等回答——这是必须让用户立刻知道的事——对读屏用户完全无感知。
建议：`#ada-trays` 加 `role="region" aria-live="polite" aria-label="待处理事项"`；polite 即可（assertive 会打断正在的输入），视觉层面已有 in-flow 托盘保证可见性。

**10 表单校验失败落裸错误页**
`AppError` 渲染内联样式裸页「请求失败」（src/handlers/mod.rs:56-66），而 404 用的是带应用外壳的 ErrorPage（src/handlers/mod.rs:925-938）。空标题建会话、缺 provider/model 切模型、未知推理等级/审批档位等参数校验失败（src/handlers/chat/mod.rs:859、944、963、1136）都会把用户甩出界面壳。
建议：`AppError` 统一走 ErrorPage 模板（保留文案，换壳）；长期把「参数校验失败」从 AppError 改为回 flash（这些表单都是站内动作，不该有裸页）。

**11 触屏上复制按钮与会话行操作不可见**
`.chat-copy` 默认 `opacity:0`，仅 `pre:hover` / `:focus-visible` 显出（static/shell.css:10-28）；`.ada-thread__actions`（行内重命名/删除）同样 hover 才显（static/shell.css:329-334）。触屏没有 hover，两者实际不可发现（opacity:0 的元素仍可盲点，但没人知道在那）。对比：工作区组头操作 `.ada-ws__actions` **已经有** `@media (hover: none)` 常驻补丁（static/shell.css:1255-1259）——同一件事三种口径。
建议：给 `.chat-copy` 与 `.ada-thread__actions` 补同款 `hover: none` 常驻规则；顺手全库 grep 一遍 `opacity: 0` 的 hover 显隐，统一收口。

### D. 键盘与焦点

**12 POST 回跳后焦点落 body；弹窗关闭不归还焦点**
任何动作 303 回来，焦点丢到页面起点（composer 无 autofocus 策略，全库仅登录页与 Modal 有）。`<ada-modal>` open 时焦点进弹窗（static/ada-modal.js:48-60），close 时不记录也不归还触发元素（static/ada-modal.js:62-72）——键盘用户确认完一个删除，焦点凭空消失。
建议：① modal open 时记 opener，close 时归还；② 动作回跳页若带 `?focus=composer` 之类的标记则聚焦输入框（与 01 的锚点方案共用机制）。

**13 Ctrl+K 已被侧栏搜索占用**
static/chat.js:291-297 把 Ctrl/⌘+K 绑到「展开侧栏并聚焦会话搜索」。STATUS P2 的命令面板若沿用 原桌面端 的 Ctrl+K，落地即冲突。
建议：现在就在 UED §8.5 记一笔——命令面板接管 Ctrl+K 后，侧栏搜索降级为 `/`（聚焦输入框类快捷键惯例）或保留在命令面板内；避免届时悄悄双绑。

**14 Esc 一刀切关所有浮层**
Esc 同时关闭全部 `details.ada-popover`（static/chat.js:165-170）与全部模型选择器（static/chat.js:701-706）。两类浮层叠开时一次全灭（modal 有栈、逐层关，static/ada-modal.js:111-131，是对的）。
建议：popover 类也做「只关最上层」：按 DOM 深度/最后打开时间取一个关。低优。

### E. 布局与信息密度

**15 多托盘纯堆叠，无优先级与限高**
托盘按 审批→提问→排队 DOM 顺序硬拼接（src/handlers/chat/rows.rs:1031-1035）；多条审批各自成卡同屏堆叠；排队一长就把 composer 挤出视口。审批与提问同时待处理时（agent 先问后批的常见序列）没有视觉分层，Tab 顺序=审批优先纯因 DOM 在前。
建议：① 排队区 `max-height` + 内部滚动 + 「N 条排队」汇总头（现在是每条一行全展开）；② 审批与提问同屏时给托盘区一个小节导航或至少视觉分组标题加重；③ 多条审批保留各自成卡（各自参数必须可见），但加统一「等待批准 ×N」头。

**16 新建会话不能选模式（coding / pm）**
模式在顶栏/工具行只读展示；「新会话」弹窗只选工作区（static/app.js:135-154 的 `data-new-thread` 拦截）。STATUS P1 已列。
建议（交互形态）：模式选择并进现有新会话弹窗——工作区 radio 列表上方加一排模式 segmented control（默认 coding，pm 用 UED §6.1 的 amber 语义色），`thread.create` 带 mode 参数一次提交；开始后保持只读（原桌面端 口径）。

### F. 插件页

**17 provider-admin 表单丢失风险与误导按钮**
连接参数表单（frontend-plugins/provider-admin/page.tsx:96-121）没有任何脏检查：切页签/切左列供应商/刷新（都是纯链接导航，page.tsx:7-13）即丢已填内容——而 Base URL/代理/请求头往往是一长串。「测试连接」按钮名义是测试，实际 `config.checkProvider` 只做参数校验、不发真实请求（README 注记 + src/handlers/providers.rs 测试端点）。
建议：① 表单脏时拦截导航（beforeunload + 站内链接点击确认，可复用共享确认弹窗）；② 在后端能力补齐前，按钮文案与结果文案改为「校验参数」级别的事实表述，避免「测试通过」误导用户以为端点可达（这与 UED §9「不用已完成描述未确认的状态」直接冲突）。

**18 弹窗内表单回跳链路依赖 `?dialog=`，无失败回填**
`openDialogFromQuery`（static/components.js:262-271）让弹窗表单提交失败后回跳时弹窗自动立起来——机制是对的；但模型编辑弹窗（page.tsx:183-199）失败时用户已填的修正值不回填（同 03 的表单版）。
建议：失败回跳时把表单值带回（query 或一次性 sessionStorage），与 03 共用一套「失败回填」工具。

---

## 4. 改造路线

| 批次 | 条目 | 工作量 | 验收 |
|---|---|---|---|
| **P0 正确性**（一天内可清完） | 04 补 flash 文案 + 守门 | 小 | 重命名/删除会话、提交回答后横幅显示中文 |
| | 05 等待时长统一 startedAt | 小 | 同一张审批卡行内与托盘认时一致 |
| | 06 排队操作带文本指纹校验 | 小 | 双开两页，一页删队首后另一页点删除 → flash 提示排队已变化 |
| | 08/09 error flash `role=alert`、托盘 aria-live | 小 | 读屏（NVDA）复现：动作失败有播报；新审批到达有播报 |
| | 11 触屏 hover 补丁两处 | 小 | 手机/DevTools 触控模拟：代码块复制、行内重命名可见可点 |
| **P1 主轴**（局部提交或状态保持三件套） | 21 composer 工具行动作改 fetch 局部提交（切模型/档位/审批决定/队列），成功走既有 parts 通道回刷 | 中-大 | 切模型后滚动位置、展开态、草稿原样；无 JS 仍走整页 POST 降级 |
| | 03/18 失败回填（发送草稿 + 弹窗表单值） | 中 | 发送失败（断网模拟）后正文回填在输入框 |
| | 01/02/12 回跳锚点 + openKeys/焦点 sessionStorage（21 的过渡方案，若 21 排期远则先做） | 中 | 批准请求后停留在原阅读位置，展开不折 |
| | 07 无 JS 危险动作服务端确认 | 中 | 禁 JS 后删会话出现确认页；正常路径弹窗行为不变 |
| | 10 AppError 统一 ErrorPage | 小-中 | 空标题建会话得到带壳错误页 |
| | 15 排队区限高 + 多托盘汇总 | 小-中 | 10 条排队时 composer 仍在视口内 |
| | 17 provider-admin 脏检查 + 按钮文案纠偏 | 小-中 | 填一半切供应商弹确认；「校验参数」不再暗示可达性 |
| | 二级页离线壳（STATUS P1 既有项） | 中 | 拔网关后 /overview、插件页保留壳显示未连接 |
| **P2 能力面**（对齐 STATUS P2，本文只补交互细节） | 16 新建会话选模式（并入现有弹窗） | 中 | 新会话即 pm 模式，顶栏只读展示 |
| | 13 命令面板绑定规划（Ctrl+K 归属先拍板） | — | UED §8.5 更新 |
| | 14 Esc 逐层关闭；图片预览；附件/@/斜杠；TabStrip；右侧面板 | — | 见 STATUS |

**21 号建议展开**（P1 的核心一笔，值得单独说清）：composer 工具行的四个动作面（模型/推理等级/审批档位/审批决定，可选加队列操作）改成 `fetch` POST + 成功后手动触发一次 `/live` 拉取（或直接信任下一帧 parts），失败就地内联错误文案。这一笔同时消灭 01（跳转打断）、03 大半（草稿不再被周边动作清空）、04 的暴露面（高频动作不再依赖 flash）。发送本身**保持整页 POST**——它语义上就是导航（新消息成为锚点、置底合理），且 SSE 首帧全量同步本来就会接管后续。

---

## 5. 与 STATUS.md / UED.md 的对齐

- **STATUS P1「审批档位页内切换」已实现但未勾**：审批档位弹层（四档、当前项打勾、「完全访问」带 data-confirm）已在输入框工具行落地（src/handlers/chat/mod.rs:239-285），路由 `/approval/mode` 已注册（src/main.rs:211），flash 文案已登记（src/handlers/mod.rs:189-192），菜单随 parts 的 `composer-approval` 换帧保开合（static/chat.js:389）。建议 STATUS 勾掉该条并把它记进「已完成 §1」。
- **STATUS 内部重复**：「侧栏会话列表增量刷新」在「已完成 §2」标了 2026-10-24 完成，TODO P1 里同一条仍是未勾状态——顺手删掉 TODO 侧。
- 本文 **04/05/06/07/08/09/10/11** 均为 STATUS 未登记的新条目，建议按 §4 的批次并入 TODO。
- UED §7.6 已写明「SSR 版审批/排队用 in-flow 托盘」的取舍，本文 15 不推翻它，只做限高与汇总。

---

## 6. 现状做得好的（改造时保持）

- 折叠跨换帧按 item id 恢复、面板内部滚动位置还原（chat.js:53-115）——同类 SSR 方案里少见的细；
- 提问多题翻页 + 草稿 sessionStorage 跨刷新（chat.js:394-530），无 JS 平铺兜底（trays.rs）；
- SSE「收到真事件才算活」（chat.js:709-715），避免网关挂掉时假在线关掉兜底轮询；
- 模型选择器 / 审批档位 / ＋菜单全部「无 JS 可用，JS 只补点外关闭」（model_picker.rs、chat/mod.rs:239-285、components.js:231-248）;
- `<ada-select>` 键盘完备（方向键/Home/End/Enter/Esc，static/ada-select.js:126-172），页签带完整 ARIA + 方向键 + replaceState（static/components.js:333-423）；
- 确认弹窗全站唯一组件、有模态栈与焦点圈（static/ada-modal.js），杜绝 `window.confirm`；
- 插件气泡/工作区分组/任务卡的展开态在 parts 换帧后全部有还原路径。
