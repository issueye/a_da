# ssr-h 交互改造开发计划

| 项 | 值 |
|---|---|
| 日期 | 2026-10-24 |
| 输入 | [INTERACTION-REDESIGN.md](INTERACTION-REDESIGN.md)（问题 01–18 + 方案 21） |
| 性质 | **开发计划（未开工）**：任务卡 T00–T15、批次 M0–M2、验收标准与出场门 |
| 约束 | 不改网关 / agent-rpc 协议（全程只动 `ssr-h/`；T15 已核实无需后端改动）；不动 desktop-ui / plugin-hub |
| 节奏 | 一任务一 commit（沿用 `fix(ssr-h)` / `feat(ssr-h)` 前缀），一批一出场 |
| 实施进度 | **2026-10-24：M0 全部完成；M1 的 T06/T07/T08/T11 完成；M2 的 T13/T14/T15 完成**（T13/T15 由并行提交落地）。剩余：T09 无 JS 服务端确认、T10 AppError 统一、T12 provider-admin 脏检查。**T15 的交互形态已改**：新建弹窗整体移除，模式/工作区选择挪到空会话页（对齐 desktop-ui 的 EmptyConversationView） |

---

## 1. 目标与非目标

**目标**

- M0：消灭正确性 / 信任类问题（裸串文案、双口径时长、排队误伤、读屏静默、触屏不可见）；
- M1：建立**局部提交通道**与**失败回填机制**——composer 周边动作不再打断阅读位置 / 展开态 / 草稿；边缘环境（无 JS）闭合；
- M2：二级页离线壳、新建会话选模式、快捷键归属落档。

**非目标**

- 命令面板、TabStrip、右侧面板、附件 / @ / 斜杠、图片预览——维持 STATUS P2，不在本计划排期；
- 「测试连接」发起真实请求——依赖后端 `config.checkProvider` 增强，本计划只做文案纠偏（T12）；
- 排队项稳定 id——依赖后端队列 RPC 改造，本计划用文本指纹过渡（T03），根治项挂起；
- 不引入前端框架、不改 SSE 三类事件协议。

---

## 2. 批次总览

| 批次 | 主题 | 任务 | 工期（串行估） |
|---|---|---|---|
| **M0 止血** | 正确性与可感知性 | T00 文档同步、T01 flash 文案、T02 时长口径、T03 排队指纹、T04 aria、T05 触屏 | ≈ 2 人日 |
| **M1 主轴** | 局部提交 + 状态保持 + 无 JS 闭合 | T06 局部提交协议、T07 失败回填、T08 状态保持、T09 服务端确认、T10 错误页统一、T11 托盘限高、T12 provider-admin | ≈ 6 人日 |
| **M2 收尾** | 离线壳与能力补齐 | T13 二级页离线壳、T14 快捷键落档、T15 新建会话选模式 | ≈ 2 人日 |

任务间依赖见 §4；T13 / T15 无前置依赖，人手富余可随时插队。

---

## 3. 任务卡

> 每卡：对应建议编号 → 改动文件 → 实现要点 → 降级路径 → 验收 → 风险。「建议 NN」均指 INTERACTION-REDESIGN.md 的编号。

### M0 批

**T00 文档同步（0.5h，建议 §5）**
- `docs/STATUS.md`：勾掉 P1「审批档位页内切换」（代码已落地：chat/mod.rs:239-285、main.rs:211）；删掉与「已完成 §2」重复的 TODO 条目「侧栏会话列表增量刷新」；把 04–11 对应的新条目按本计划批次并入 TODO。
- 验收：STATUS 与代码现状零冲突。

**T01 flash 文案映射 + 守门（0.5d，建议 04）**
- `src/handlers/mod.rs:167-206`：补 `thread_renamed` → `已重命名会话`、`thread_deleted` → `已删除会话`、`answered` → `已提交回答`。
- 守门测试：仿 `tests/ui_scale.rs` 扫 CSS 的先例，新增单测扫描 `src/` 里 `"notice="` / `notice=` 模板字面量，收集全部 notice key，与 `flash_from_query` 的映射表比对——**只允许显式映射，禁止落到 `other =>` 兜底**（白名单豁免可后续按需加）。
- 验收：重命名 / 删除会话、提交回答，横幅均为中文；删掉一条映射测试即红。

**T02 审批等待时长统一 startedAt（0.5d，建议 05）**
- `src/handlers/chat/rows.rs:851-854`：`waited_label` 改读 `startedAt`（复用同文件 `started_ms`）；`src/ui/components/trays.rs:173-192` 的 `waited_from_at` 保留，但只喂真实时间戳。
- 单测（trays.rs 已有 `#[cfg(test)]` 模块可挂）：`at=1`（序号特征值）不再产出「已等待 …」；`startedAt = now-3s` 产出「已等待 3s」。
- 验收：同一张等待批准卡，行内「待批 X」与托盘「已等待 X」一致。
- 风险：老盘历史会话可能缺 `startedAt`——缺失时**不显示**等待时长（宁缺毋错），不做 `at` 兜底。

**T03 排队操作文本指纹校验（0.5d，建议 06）**
- 渲染侧 `src/ui/components/trays.rs:195-240`：提升 / 删除表单各加 `<input type="hidden" name="expect" value="{text 截断 200 字符}">`（`QueueItem.text` 已在渲染现场）。
- 服务端 `src/handlers/chat/mod.rs:1207-1229`：promote / remove 先 `state.agent.snapshot()`（agent.rs:107），按与渲染同一投影函数取当前 `queue[index].text`，与 `expect` 不一致 → 回 flash「排队已变化（可能已被其它客户端改动），请确认后再操作」，**不执行 RPC**。投影函数从 rows.rs 的队列构建处提取共用，避免两份口径。
- 验收：双开两个页签，A 删队首后 B 点「删除第 2 条」→ flash 提示而非误伤；正常路径不受影响。
- 风险：同文本相邻排队项区分不了——可接受（内容相同则语义等价）；根治等后端稳定 id。

**T04 错误播报与托盘 aria（0.5h，建议 08/09）**
- `templates/chat.html:57`、`templates/layout.html:49`：`kind == "error"` 时输出 `role="alert"`，其余维持 `role="status"`。
- `templates/chat.html:120`：`#ada-trays` 加 `role="region" aria-live="polite" aria-label="待处理事项"`。
- 验收：NVDA 复现两条——动作失败有播报；新审批到达有播报；polite 不打断输入中的读屏焦点。

**T05 触屏 hover 补丁（0.5h，建议 11）**
- `static/shell.css`：`.chat-copy`（10-28 行）与 `.ada-thread__actions`（329-334 行）补 `@media (hover: none)` 常驻规则，对齐 `.ada-ws__actions` 的既有补丁（1255-1259 行）。
- 顺手 `grep -n "opacity: 0" static/*.css` 收口同类显隐，发现即补，并把清单写进 commit message。
- 验收：DevTools 触控模拟下代码块复制、行内重命名 / 删除可见可点。

### M1 批

**T06 局部提交协议（1.5d，建议 21——本计划核心一笔）**
- **协议**：动作者表单加 `data-partial`；`chat.js` 捕获 submit（在 data-confirm 通过之后）→ `fetch(action, { method:'POST', body:FormData, headers:{ 'Accept':'application/json' } })`；服务端 handler 检测 `Accept: application/json` 时返回 `200 { ok, notice }` / `{ ok:false, error }`（文案复用 T01 映射表），**不 303**；否则照旧 303（无 JS 降级原样）。CSP `connect-src 'self'` 已放行同源 fetch。
- **改造 handler**（每个提取「执行 + 结果」内部函数，两种返回包装）：`select_model`（chat/mod.rs:937）、`select_effort`（957）、`decide_approval`（1000）、`set_approval_mode`（1126）、queue promote/remove/clear（1207/1219/1231）。
- **客户端收尾**：成功 → 就地反馈（触发表单按钮短暂「已提交 ✓」态）+ 立即手动跑一次 `/live` 拉取（复用 chat.js:642-667 的 `applyOob`/`pollOnce` 逻辑，不等下一帧 parts）；失败 → 表单上方插入 aria-live 就地错误文案，**页面不动**。
- **不做**：发送（语义是导航，保持整页 POST）、会话 / 工作区管理（本就要跳转）。
- **兜底**：fetch 异常或响应非 JSON → 撤销拦截、原生重提。
- 验收：切模型 / 切档位 / 批准 / 排队操作后——滚动位置不动、展开态不动、草稿不动、URL 不变；禁 JS 后同一批动作全部仍走 303。
- 风险：与 data-confirm 委托的先后顺序（确认通过后才 fetch）——单测 + 手工矩阵覆盖；htmx 片段端点（fetch 远程模型）不涉及。

**T07 失败回填：发送草稿 + 弹窗表单值（1d，建议 03/18）**
- **发送草稿**（纯客户端）：`chat.js` 在 composer 表单 submit 时写 `sessionStorage['a_da_draft:{threadId}']`；页面加载时若 composer 为空且存在草稿 → 回填 + 辅助区提示「已恢复未发送的草稿」；URL 带 `notice=sent` 时清除。断网发送失败（`?error=` 回跳）后草稿自然回来。
- **弹窗表单回填**（components.js 通用化）：提交带 `data-restore` 的弹窗表单时把字段值存 sessionStorage（键 = form action）；`openDialogFromQuery`（components.js:262-271）立起弹窗后回填。首批挂接：provider-admin 模型编辑 / 新增弹窗（page.tsx:183-215）。
- 验收：①断网发长文 → 失败横幅 + 输入框原文还在；②模型编辑弹窗填一半提交失败 → 回跳后弹窗立起且值在。
- 风险：多标签页同名键竞争——按「最后写入赢」，可接受。

**T08 状态保持过渡：openKeys / chatTop / 焦点（0.5d，建议 01/02/12；T06 落地后剩余面缩小，保留为兜底）**
- `components.js:20` 的 `openKeys` 防抖同步到 `sessionStorage['a_da_open:{threadId}']`，`bindDisclosures` 时恢复；`chat.js` 的 `chatTop` 同步 `sessionStorage['a_da_chatpos:{threadId}']`。
- **恢复触发条件**：URL 含 `notice=` / `error=`（动作回跳签名）或 `performance.navigation` 为 reload 时恢复；直接打开会话仍默认置底（既有语义不变，chat.js:176-190）。
- 焦点：`<ada-modal>` open 记 opener、close 归还（ada-modal.js:48-72）；回跳带 `notice/error` 时不抢焦点（维持现状），仅空会话 autofocus（UED §8.5 既有口径）。
- 验收：重命名 / 删除其它会话回来后，阅读位置与展开态原样；刷新页面同样原样。

**T09 无 JS 危险动作服务端确认（1d，建议 07 + 重试确认）**
- **机制**：高危表单标 `data-server-confirm`；服务端 handler 发现缺 `confirm=1` → 渲染确认页（复用 T10 的 ErrorPage 外壳 + data-confirm 文案 + 「确认执行」按钮重提带 `confirm=1`）。JS 在场：app.js 确认弹窗通过后补 hidden `confirm=1` 再提交 / fetch——弹窗体验不变。
- **覆盖清单**：删会话（sidebar.rs:283）、清空队列（chat/mod.rs:200、trays.rs:230）、删供应商（provider-admin page.tsx:79）、删模型（page.tsx:160、model_table.rs:89）、切「完全访问」档（chat/mod.rs:265）；另给「重试上一轮」补 data-confirm（chat/mod.rs:88，与中止同口径）。
- 验收：禁 JS 后删会话出现确认页；正常路径弹窗行为与现在一致；`confirm=1` 伪造不构成风险（它只是二次点击的等价物，真正门槛是登录态 / token）。

**T10 AppError 统一走 ErrorPage（0.5d，建议 10）**
- `src/handlers/mod.rs:56-66` 的内联裸页改为复用 404 路径的 ErrorPage 渲染（mod.rs:925-938 同款外壳）。
- 验收：空标题建会话（chat/mod.rs:859）、缺 provider 切模型（944）等路径得到带壳错误页 + 「返回会话」。
- 风险：ErrorPage 渲染依赖模板上下文——确认其能在无快照状态下渲染（离线时侧栏标「未连接」，chat/mod.rs:626-632 已有先例）。

**T11 排队区限高 + 多审批汇总（0.5d，建议 15）**
- `trays.rs`：排队行包一层 `.ada-tray__scroll`（`max-height: 180px; overflow:auto`）；多条审批时头部加「等待批准 ×N」汇总行（各卡保留——参数必须逐卡可见）。
- 换帧后排队区滚动位置重置——接受（瞬态列表）；如需保持，复用 chat.js `panelScroll` 按 `data-key` 的既有机制，列为可选增强。
- 验收：10 条排队时 composer 仍在视口内；审批 + 提问同屏时两个托盘有清晰分组头。

**T12 provider-admin：脏检查 + 文案纠偏（1d，建议 17）**
- 新增 `frontend-plugins/provider-admin/static/provider-admin.js`（manifest `scripts` 注入，机制已有）：表单 `input/change` 记 dirty；拦截站内链接（点击时若 dirty → 复用核心 `data-confirm` 委托弹确认「未保存的修改将丢弃」）+ `beforeunload` 兜底。
- 文案纠偏：`page.tsx:117` 按钮与结果片段「测试连接」→「校验参数」，结果文案明确「仅检查参数完整性，未访问端点；验可达性请用『获取远程模型』」（与 UED §9「不用已完成描述未确认状态」对齐）。
- 注意：改源码插件后同步 `dist/frontend-plugins/`（README 🔴 已记），或验收时用 `--plugins-dir ssr-h/frontend-plugins`。

### M2 批

**T13 二级页离线壳（1d，STATUS P1 既有项）**
- `handlers/dashboard.rs`、`extensions.rs`、providers 相关：网关失败时不再抛 AppError，渲染「侧栏 + 页面壳 + 未连接 + 重新连接」（对齐 chat/mod.rs:626-632 的聊天页先例）。T10 完成后二级页至少有壳，本任务把它升到「保留布局可导航」。
- 验收：拔网关后 `/overview`、`/extensions`、`/ext/provider-admin/providers` 均保留侧栏与「未连接」横幅，恢复后刷新自愈。

**T14 快捷键归属落档（0.5h，建议 13）**
- `docs/UED.md` §8.5 更新：Ctrl/⌘+K 当前=侧栏会话搜索（chat.js:291-297）；命令面板落地时接管 Ctrl+K，侧栏搜索改 `/` 聚焦。纯文档，避免届时静默双绑。

**T15 新建会话选模式（1d，建议 16；已核实无后端改动）**
- 链路事实：网关 `POST /threads` 为 `BodyMode::MergeOptional`（agent-gateway/src/http_api.rs:473），`thread.create` 收 `mode` 参数（agent-rpc dispatch mod.rs:362-367）——`agent.rs` 的 `create_thread_in`（179-185）在 body 里加 `mode` 即可。
- UI：新会话弹窗（`data-modal="new-thread"`）工作区 radio 列表上方加模式 segmented control（默认 coding；pm 用 UED §6.1 amber 语义色）；`POST /threads` 表单加 `mode` 字段。开始后顶栏只读展示（现状已渲染）。
- 验收：以 pm 模式新建会话，发一条消息后顶栏显示 PM 徽标（与 desktop-ui 同口径）。

---

## 4. 依赖与实施顺序

```text
T00 ─┐
     ├─ M0（T01–T05 相互独立，可并行；出场需全绿）
T01 ─┴─> T06（失败语义复用 T01 文案表）
T06 ─────> T07（失败回填依赖 T06 的「不跳转」失败面；发送草稿部分无依赖可先行）
T06 ─────> T08（T06 覆盖后，剩余整页动作越少，T08 范围越小——按剩余面裁剪）
T10 ─────> T09（确认页复用 ErrorPage 外壳）、T13（离线壳基于统一错误面）
T03 的投影函数提取 ──> T06 的 queue 三动作（同一批 handler，建议同分支连做）
独立：T04、T05、T11、T12、T14、T15（随时可做）
```

建议分支节奏：M0 一批合入；T06+T03（同触 handler 面）一条分支；T07+T08 一条；T09+T10 一条；其余独立短分支。

---

## 5. 测试与出场门

| 门 | 内容 | 频率 |
|---|---|---|
| 单测 | `cargo test --manifest-path ssr-h/Cargo.toml`（现有 guard / markdown / questions / ui 全套 + 本计划新增：T01 守门、T02 时长、T03 指纹） | 每 commit |
| 实机流式 | `node tools/verify-ssr-streaming.mjs` | 每批出场 |
| 实机工作区 | `node tools/verify-ssr-workspaces.mjs` | M0 / M1 出场（T06 触碰发送链路邻接面） |
| 手工矩阵 | 禁 JS（动作全走 303/确认页）、触控模拟、NVDA、暗色主题、≤760px 抽屉——过「发送→审批→提问→排队→重试」全链路 | 每批出场 |

各批 Definition of Done：

- **M0**：五张卡验收全过 + 单测绿 + 两条实机脚本绿 + STATUS 已同步（T00）。
- **M1**：T06 的「三不动」验收（滚动 / 展开 / 草稿）逐项留证（截图或录屏入 PR 描述）；禁 JS 矩阵确认确认页生效；其余卡验收过。
- **M2**：T13 三页离线验收 + T15 pm 模式端到端 + UED/STATUS 落档。

---

## 6. 风险与回退

| 风险 | 缓解 |
|---|---|
| T06 fetch 化引入回归（最大单笔） | 逐 handler 迁移（先模型/档位，再审批，最后队列）；每步禁 JS 路径同步验证；异常兜底=原生重提，等于一行回退 |
| fetch 跟随 303 的隐性重定向 | 协议上服务端按 `Accept` 分流返回 JSON、**不**依赖 fetch 的 redirect 行为；单测锁「JSON 请求永不 303」 |
| sessionStorage 草稿 / 状态键泄漏 | 键全带 `{threadId}` 命名空间；`pruneQuestionStates` 同款清理模式（chat.js:467-484）扩到草稿键 |
| T09 确认页被自动化脚本绕过 | `confirm=1` 只是二次点击等价物，安全门槛本就在 token / Origin 校验（guard.rs），不新增攻击面 |
| 排队指纹误报（同文本相邻项） | 语义等价可接受；flash 文案引导刷新重看 |
| 插件改动 dist 不同步（T12） | 沿用 README 🔴 口径：验收命令带 `--plugins-dir ssr-h/frontend-plugins`，收口时同步 dist |

回退总原则：一任务一 commit，任何一张卡出问题 `git revert` 单点回退，不牵连批次。

---

## 7. 工期汇总

| 批次 | 任务 | 串行工期 |
|---|---|---|
| M0 | T00–T05 | ≈ 2 人日 |
| M1 | T06–T12 | ≈ 6 人日（T06+T03、T07+T08、T09+T10 三条线并行可压到 ≈ 3.5 人日） |
| M2 | T13–T15 | ≈ 2 人日 |

**合计 ≈ 10 人日（串行）/ ≈ 7 人日（三线并行）**。建议排期：M0 立即（一天半内清完），M1 以 T06 为先导分三条线，M2 收尾；T15 用户感知最强，若只保三张卡，保 T06 / T07 / T15。
