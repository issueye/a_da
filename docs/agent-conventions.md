# Agent 协作约定（原 AGENTS.md 正文）

> 本文件是根目录 `AGENTS.md` 的**深度约定全文**。为照顾 `docs/` 与源码注释里既有的
> `AGENTS.md §N` 引用，这里**原样保留了 AGENTS.md 的章节编号**（§1–§15 与一/二/三节）。
> 历史上被引作「AGENTS.md 第 N 节」的地方，指的都是本文件对应章节。
> 根 `AGENTS.md` 是 ≤60 行的导航版，会被自动注入系统提示词；细节按需来本文件查。

---

## 一、改动前必读：几条会咬人的约定

### 1. 子智能体的工具白名单与插件注册表是**脱钩**的（踩过这个坑）

`BUILTIN_SUBAGENTS`（`src/agent/subagents/builtins.ts`）里每个内置子智能体都有一份
**写死的 `allowedTools` 严格白名单**（只有 `general_purpose` 是 `['*']` 通配）。

后果：**把新工具注册进 `defaultToolRegistry` 并不会让它被子智能体拿到。** 白名单没跟上，
子智能体就用不了——现象是「子智能体步骤特别多」，因为它退化成一轮只读一个文件。

真实案例：`batch-ops` 插件加了 `read_files`/`edit_files` 之后，三个专用子智能体（researcher、
code_reviewer、tester）依然调不到多文件读取，因为白名单是插件存在之前写的。同理，后来加的
git-tools / code-outline / project-inspector / test-runner 四个官方插件也全被挡在门外。

**新增任何面向子智能体的能力时，必须同时更新白名单。** `src/agent/subagents/builtins.test.ts`
是守门测试，复刻了 store 的真实过滤逻辑，白名单漏了它会红。

### 2. `isWriteTool` 是**失败安全**的：默认一切皆写

`ToolRegistry.READ_ONLY`（`src/agent/tools/registry.ts`）是一份只读白名单，
`isWriteTool(name)` 的判据是"名字在名单里**且不是被非内置插件借走的**"：

- 名字不在 `READ_ONLY` 里 → 写工具；
- 名字在名单里、但**注册它的插件 scope 不是 `builtin`**（含来源缺失）→ 仍按写工具处理。
  因为 `READ_ONLY` 是名字级的名单，而插件可以借走内置名字（`allowBuiltinShadow` 默认开）：
  一个叫 `read_file` 的插件工具若继承只读身份，plan 模式会放行、readonly 审批档不会问、
  只读子智能体也拿得到——审批闸门的依据（设计文档 §6.4.3）与 `effectiveToolNames` 回执一起失真。
  官方内置插件的只读工具（`git_status` / `get_outline` 等）scope 是 `builtin`，不受影响。

改动要点仍然是"失败安全"：

- 新增**只读**工具却忘了加进 `READ_ONLY` → 它会被当成写工具，**只读子智能体与 plan 模式
  都拿不到**（mode 过滤器会把它剔除）。官方插件里真正只读的 `git_status`/`git_diff`/`git_log`/
  `get_outline`/`inspect_project` 就踩过这个。
- 反过来，把有副作用的工具错列进 `READ_ONLY` 会让只读模式形同虚设。判据是「是否改动工作区或
  系统状态」：`run_test_focused` 执行测试命令（可能产生构建产物），因此**算写工具**；
  `read_url_content` 只发网络请求，算只读。

扩展（工作区里的第三方代码）注册的工具默认按写处理，这是刻意的——包括它借用了内置名字的情况。

### 3. 子智能体拿不到主线程的 composite 系统提示词

子智能体只收到 `profile.systemPrompt`（外加由 store 注入的 `notify_parent`），**不包含**
`AGENTS.md`、项目约定、以及用户在提示词管理里启用的系统规范。所以：

- 想让子智能体用某个高效工具，**工具描述 + 它自己的 systemPrompt** 两处都要引导，
  指望主线程的系统提示词带过去是无效的；
- 子智能体要能加载技能（如 `batch-efficiency`），白名单里必须有 `Skill`。

### 4. 新增写工具必须接入检查点与改动审阅

写工具要在两处登记，否则「撤销此次改动」和改动审阅面板看不见它：

1. `src/agent/store.ts` 的 `CHECKPOINT_TOOLS` —— 决定执行前是否快照；
2. `checkpointPathsOf()` —— 单文件工具读 `args.path`，批量工具（如 `edit_files`）读 `args.files`，
   **批次里每个文件都要进快照**。

批量编辑还要在返回值里给 `details.files = [{ path, patch, additions, deletions }]`，
`getThreadFileChanges` 靠它把一次调用的合并 patch 拆回逐文件，改动面板才能逐文件 diff 与回滚。

### 5. 工具结果的 `terminate` 能直接结束整轮

`runAgentLoop` 里，工具结果、`beforeToolCall`、`afterToolCall` 任一带 `terminate: true` 都会置
`terminateBatch`，导致该批工具跑完后 `break` 整个循环（`endReason = 'completed'`）。它不是
「结束这批」，是**结束这一轮**。别拿它当批次控制用。

### 6. 并发执行：全局 sequential 默认值有个例外

主循环传 `toolExecution: 'sequential'`（审批一次只问一件事，命令之间不抢工作目录）。
但**整批调用都显式声明 `executionMode: 'parallel'`** 时会重叠执行（并发委派只读子智能体
就是这种情况）；批次里只要混进一个写工具或 sequential 工具，整批退回串行。

### 7. 子智能体结束时的自动唤醒必须放在 `finally` 里

`startSubagentThread` / `resumeSubagentThread` 结束时要调 `wakeParent()` 唤醒挂起等待的
父会话。两个坑：

- 必须**在 `runningThreadIds.delete()` 之后**（也就是 `finally` 里）调用，否则
  `suspendForSubagents` 的「看护对象是否全部结束」会算错；
- 离线兜底路径从 `return` 出去、**走不到 `finally`**，那里要单独补一次唤醒。

`notify_parent` 刻意绕过 profile 白名单（否则只读子智能体唤醒不了父智能体，委派机制就断了），
并且**不在通用工具表里**——由 store 在建子智能体工具表时追加，会话 id 也在那时注入
（不能靠运行时「找唯一在跑的子会话」推断，并发时会认错人）。

### 8. 别用轮询代替等待

主智能体派发后台子智能体后应调 `await_subagents` 挂起等待，不要反复 `check_subagent`：
每一次轮询都是一整轮模型请求、要把整个上下文重发一遍。模型的行为由提示词和工具描述共同
塑造（`src/agent/subagents/manager.ts` 的委派准则），改机制时别忘了同步改引导文案。

### 9. 决策插件：绝不捏造确定性

`src/agent/tools/builtin-plugins/decision/` 是给智能体补「结构化判断」的地方。改它时守住三条：

- **拿不到真实判断就失败，不要编数字。** 启发式引擎只产出刻意中性的占位值（confidence 0）
  并标注「请勿据此决策」；`decide` 在没有可用引擎时返回 `ok: false`。
- **概率必须标注 `calibrated`。** `true` 仅限 Jev 引擎（专用 System One 模型，概率是校准的）；
  本地自评是**多次采样的投票占比**，`calibrated` 恒为 `false`。聊天模型自报的概率普遍虚高，
  所以 `LocalEngine` 刻意不采信它，只作参考放进 `confidence`——**不要"优化"成直接采信自报值**，
  那会让校准性名存实亡。
- **失败方向按场景分。** `decide` 失败即失败；`check_gate` 默认 fail-close（无引擎时判不通过），
  因为「门禁永远放行」比「要求复核」危险。改动这个默认值要先想清代价不对称。

另注意：`design_decision` 与 `decide` 收到的是**模型生成的 JSON**，必须过
`validateDesign` / `normalizeQuestions` 的严格校验（`choice` 的 criteria 是对象、`score` 是数组、
`noul` 不带 criteria）。校验规则在两处各有一份实现，改一处要同步另一处。

### 10. 插件系统有一串"必须同时改"的白名单，每个都有守门测试

插件要真正生效，往往要在**多个各自独立的位置**同时登记。漏掉任何一处的表现都是
"看起来装上了、就是没用"，而且不报错：

| 位置 | 漏了会怎样 | 守门测试 |
|---|---|---|
| `ToolRegistry.READ_ONLY`（`tools/registry.ts`） | 只读工具被当成写工具，plan 模式与只读子智能体拿不到 | `plugins/equivalence.test.ts`、子智能体测试 |
| `BUILTIN_TOOLS_CATALOG`（`tools/registry.ts`） | 插件管理页的「内置核心工具」清单缺条目（`resume_subagent` 就是这么漏的） | `plugins/equivalence.test.ts` 的「目录与真实工具表对得上」 |
| `BUILTIN_SUBAGENTS[].allowedTools`（`subagents/builtins.ts`） | 子智能体调不到该工具，退化成一轮读一个文件 | `subagents/builtins.test.ts` |

新增核心工具时**三处都要过一遍**，然后跑 `bun test src/agent/plugins/equivalence.test.ts`
让它替你确认。

### 11. 插件契约：`LoadedPlugin` 是唯一产物，id 由加载器决定

- 两条加载路径（内置 `BUILTIN_PLUGINS` 数组、工作区/全局的 jiti 扫描）都必须产出
  `LoadedPlugin`（`plugins/types.ts`）。新增能力字段时同步改 `PluginContributions`
  **和** `loader.ts` 的 `parseExtensionModule` / `finalizePlugins`，并考虑
  `plugin-system-dev-plan.md` 里的下一个里程碑是否已有约定。
- 第三方插件的 `manifest.id` **一律由加载器按「scope:文件名」生成**，插件自报的 id 会被覆盖
  ——id 是启停表、诊断、工具溯源的键，允许自报会让它们对不上。
- 状态判定集中在 `finalizePlugins`：依赖缺失/初始化抛错 → `broken`、缺必填配置 → `not-ready`
  （这两种**不注册工具**）、版本不匹配 → `incompatible`（仍加载）、工具名冲突 → `conflict`。
  判定顺序是 broken > not-ready > incompatible > conflict。
- 插件之间的名字冲突**只记录不拒绝**（后注册者胜 + `console.warn` + 诊断）。"是否允许遮蔽
  内置"是 M2 能力开关的事，加载层别自作主张拦下来。
- 事件监听器按插件分组持有（`eventListeners: Map<pluginId, Set>`）。`autoLoadExtensions`
  开始时必须 `clearListeners()`：只清工具不清监听器，`onEvent` 订阅会一轮一轮累积。

### 12. 钩子层：成对是硬约束，工具集只能收窄

钩子（`core/events.ts` 的 `AgentHooks`）是插件**能改变控制流**的通道，与只读的
`onEvent` 刻意分开。改这一层时守住六条：

1. **成对**：凡是有状态延续的点位，`before*` 必须有配对的 `after*`，且 `after*` 在
   `before*` 被短路时**照常执行**。新增点位要同步四处：`AgentHooks`、`HOOK_PAIRS`
   （或 `UNPAIRED_HOOKS`——纯判定/纯通知/O 单向点位进这里）、`hooks-pairing.test.ts` 的
   `HOOK_KEYS`（后两者会让 typecheck 或测试直接红）、以及**点位真正被调用的地方**
   （循环 / store / 工具 / 管理器）。漏配是最难发现的缺陷——插件会在"以为自己生效了"的
   状态下工作；而"接了契约却没人调用"同样静默。
2. **工具集只能收窄**（唯一不可配置项）：收窄在 `agent-loop.ts` 的 `narrowTools()` 里做，
   取的是**授权实例**而不是钩子递过来的实例（否则插件能顶着 `read_file` 的名字塞自己的
   实现）。工具名按集合语义处理，`applyTools` 的签名与 specs 都**从去重后的 map 生成**。
3. **循环里的钩子调用必须走 `callHook()`**：它兜住异常并记 trace。别直接 `await hooks.x()`——
   插件打崩主循环是这里最容易犯且最难查的错。
4. **无钩子时整体跳过**：每个调用点都写成 `if (hooks?.beforeTurn)`；点位缺席时**不进
   try/catch、不计时**。这是"没有插件时零额外开销"的实现方式，`turn-hooks.test.ts` 有一条
   空对象与不传钩子的对照用例。
5. **能力开关关掉后必须可见**：`hook-runtime.ts` 负责过滤，并在丢弃某个插件的意图时写一条
   trace 说明原因。新增开关时，要在 `PluginCapabilities` + `coerceCapabilities` + 读取
   路径三处都接上——用户改了配置却看不出任何变化，等于没实现。
6. **子循环也要挂钩子**：主循环、`store` 的两条子智能体循环、`subagents/runner.ts` 的同步
   兜底，四处都要带上 `hooks` / `hookContext`（`kind` + `subagentId`）。漏一处的表现是
   "插件在某些场景下莫名其妙不生效"。

**两处与设计文档的刻意偏差**（写在这里免得后来者当成 bug 去"修"）：
- `afterToolCall` 在工具被拦截时**不触发**：什么都没执行，没有需要清理的状态；让 shell 钩子
  对一个从未运行的命令触发反而是错的。
- `afterTurn.replaceText` / `afterAgentEnd.appendNote` **没有进契约**：本项目没有"改写已渲染
  回复"与"向已结束会话追加旁注"的交付通道，声明它们只会变成静默失效。要加就得先有通道。

### 13. 审批与压缩：两个方向的效力刻意不对称

这两处是 M3 新开的干预点，规则和其它 before* 不一样，改的时候别改成"对称"的：

- **`beforeApproval`**：`deny` 总是被采纳（收窄），且**链不短路**——一个插件不该能推翻另一个
  插件的否决；`allow` 在 **readonly 审批档位下被忽略**并写日志（那一档的语义就是"写操作必须经
  我确认"）。与 `failOpen: false` 同源的原则：**用户明确表态过的事，不让插件悄悄改掉**。
  审批**只在闸门本来要问用户时才跑**——不问就没有"免问"可言。
- **`beforeCompaction`**：追加保留消息**任何配置下都生效**（只会让压缩少做点）；整体替换
  `CompactSelection` 受 `allowCompactionReplace` 约束（默认开）。判定应用是 `compact/verdict.ts`
  里的纯函数，按**对象引用**匹配要保留的消息。
- **`beforeSubagentStart`** 的失败方向见 §12 与 `subagents/access.ts` 的判定表：未配 `failOpen`
  就是**放行 + 提示**（"用户没表态"不等于"要求安全"），显式 `false` 时拿不出依据也拦。
- 这三个点位的 `after*` 都是**纯观察**（审批/压缩已经发生），别给它们加返回值语义。

- 第二优先的六个点位（`beforeLlmRequest`/`afterLlmResponse`、`beforeSystemPrompt`、
  `beforeSkillLoad`/`afterSkillLoad`、`beforePersist`、`afterCheckpoint`）里，**后四个是单向的**：
  组装完成、落盘、检查点这些是终态，没有可配对的 `after`/`before`。它们都登记在
  `UNPAIRED_HOOKS` 里，配对守门测试据此把"漏登记"和"刻意不成对"区分开。
- `beforePersist` 挂在**异步**链路上（`persist` 本身仍是"发起即返回"）：别为了让它同步而把
  `persist` 改成 async——它有一堆调用点在 fire-and-forget。

另外，UI 测试有一条硬约束：**同一时刻只让一个真窗口活着**。GPU 测试渲染器开的是真窗口，
两个窗口同时活着时按坐标派发的 `click` 会落到另一个窗口上——新开一条会 mount 窗口的用例，
实测让标签栏那组五条用例集体翻红（`store.activeId` 停在别的会话上）。要在弹窗里加断言，
就写进已有的那条用例里（见 `PluginsDialog.test.tsx` 的做法：`beforeAll` 准备数据、已有用例多切一次页）。

还有一条：**测试渲染器报的窗口尺寸是"真实窗口"，不是 `createTestRoot({ width, height })`
传的那个尺寸**（实测 `renderer.getWindowSize()` 返回 1536×1061，而用例开的是 800×600）。
所以组件里凡是读 `useWindowSize()` / `getWindowSize()` 算出来的尺寸（布局上限之类），
断言期望值必须用**同一个信号**去算，别拿用例自己的宽高推——否则会得到"看起来测了、
其实在验一个跟组件无关的数"。好消息是量出来的东西很准：`maxHeight` 生效、
`overflowY: 'scroll'` 真的会创建滚动容器（判据是 `renderer.getScrollOffset(id)` 非 null；
非滚动元素返回 null），所以"夹住高度 + 内部滚动"这类写法可以直接按像素断言。

### 14. 审批：策略归插件（`approval-guard`），执行归核心（`askUser`）

改审批相关代码前先理解这条分工——它决定了什么能改、什么不能：

- **能改的（策略）**：哪些工具免问、哪些命令要二次确认、拒绝时给模型什么理由。
  这些都在内置插件 `tools/builtin-plugins/approval-guard.ts` 里，用户可在插件配置里改。
- **不能改的（执行）**：弹卡片、卡片状态、等点击、中止/超时收尾、把理由写回历史。
  这些在 `store.gate()` 与 `store.waitForUserApproval()`，**两者共用同一份等待实现**——
  别为新场景复制一份，否则"中止算不算拒绝""卡片 id 用哪个"会在两边漂移。

`ctx.askUser` 是插件**唯一**能发起询问的通道，且**只能转发真实点击**：核心没有、也不要加
"直接批准"的接口。插件想放行只能返回 `{ decision: 'allow' }`（那受 readonly 档位约束、且会
写日志）。`answeredBy` 必须如实区分 `'user'` 与 `'aborted'`——两者结果上都是拒绝，
但事后审计要能分辨"用户说不"与"用户没答"。

**点位顺序不能反**：`beforeToolCall`（插件：这个调用根本不该发出去）在
`beforeApproval`（要不要问用户）**之前**。反过来会让用户被问一个注定被拦的调用。
两者的 `deny` 都写成 `toolResult` 回给模型，**不要抛错**——抛错会被模型理解成
"工具执行失败"从而反复重试，而不是"被策略拦下"。

### 15. 陷阱：钩子"声明了却没人调用"是静默失效，只能靠副作用断言发现

`beforeToolCall`/`afterToolCall` 曾在 `AgentHooks` 里存在、被 `hook-runtime` 完整合成
（含超时、计时、折叠）、并被 `hook-runtime.test.ts` 直接测过——**但主循环从来没调它们**
（只调 `options.*` 的 store 版本）。插件注册的这两个点位是死代码，没有报错、没有日志，
单测全绿。

教训：**测钩子时断言副作用，而不是断言钩子被调用**。`turn-hooks.test.ts` 里那条
"被 block 时工具真的没执行"（用计数器 spy 工具）就是这个用途——它是唯一能暴露
此类缺陷的写法。新增点位时照抄这个模式。

### 16. 界面只认 `ui/client` 的四条通道；加协议方法要**同步三处**

M0–M3 把界面与 agent 侧拆开了（真拆：打包形态是两个进程，见 `docs/ui-host-split-dev-plan.md`）。
规矩只有一条，但它是硬的：**`src/ui/**`（非测试）不许 import agent 侧的实现**。

四条通道（界面做什么都从这四个里选一个）：

| 通道 | 用途 | 例子 |
|---|---|---|
| `client.state` | **读**（主机快照 + 客户端本地 + 本地推导） | `client.state.active.items`、`client.state.isThreadRunning(id)` |
| `client.request(method, params)` | **协议命令**（要主机做事） | `thread.send`、`plugin.list`、`change.revertFile` |
| `client.ui.*` | **纯客户端动作**（界面怎么看） | 确认框、标签开合 |
| `client.subscribe(cb)` | 订阅状态变化（React 里用 tick 套路） | `AgentWindow` |

守门在 `src/ui/protocol-boundary.test.ts`（4 条断言，读文件的那种测试）：
① UI 不 import `agent/store`、不出现 `store.` 用法；② 待迁移清单不许腐烂；③ 协议层不 import 实现模块；
④ UI 只允许 import agent 侧的**纯函数与纯常量**（按**符号**判，不按模块判——`agent/prompts` 这种
模块同时导出管理器与纯函数，按模块放行等于把管理器也放进去）。

**加一个协议方法要同步三处**（漏一处就是"文档有、界面调不到"或"界面调了、主机没实现"）：

1. `src/shared/protocol/methods.ts` —— 方法名 + `params` + `result`（`ParamsOf`/`ResultOf` 由它推导，
   类型会替你抓错）；
2. `src/agent/host/dispatch.ts` —— 主机侧实现（**唯一**允许直接调 store 与管理器的地方）；
3. 界面调用点 —— 走 `client.request`。

新方法**必须**在 `docs/jsonrpc-protocol.md` 里有对应条目；实现期发现偏差就回写文档
（协议 §2 的"实现状态"块就是这个用途，别让文档承诺 A、实现是 B）。

还有两条拆分后新增的"会咬人"的点：

- **hooks 必须排在提前 return 之前**：`ComposerTelemetryBar` 曾在"空会话居中 `return null`"之后
  调用 `usePromptChars`，会话一有消息就触发 React 的 "Rendered more hooks than during the previous
  render"。这个错误**不会让测试失败**，只会出现在测试输出的 React 报错里——看全量输出时要留意。
- **进程本地状态别放错边**：`confirmModal` 带回调，永远只能留在客户端；快照里的字段必须是
  可序列化的纯数据（`ClientSnapshot` 是唯一口径）。

### 17. 打包后的 exe 收不到自动化通道——二进制检查要用应用日志当证据

`src/platform/init.ts` 会把 `console.log` **劫持**到 `A_DA_HOME/app_debug.log`，只有在
"检测到控制台窗口"时才同时写真实 stdout。而 `@gpuix/react/automation` 的 `launch()` 通道
恰恰是走 `console.log('data: …')` 的——打包产物是 PE 子系统 2（**无控制台**），于是响应只进日志、
到不了管道，`launch()` 会**一直等下去**（不是报错，是卡住）。

所以：

- **窗口级交互验证（点击/截图/查 testId）用开发态**（`bun app.tsx`，有控制台，自动化通）
  或 `bun test` 里的真窗口用例；
- **二进制验收（`scripts/binary-check.ts`）用应用自己的启动日志当证据**，并且
  **每一步都有硬超时、失败必收尾、失败留日志尾部**——"卡住"是最难查的失败，不允许再发生。

`bun scripts/binary-check.ts` 现在验两关：UI 角色（画出首帧 + 日志里确认走的是 `ws` 传输，
即"自己 spawn 了自己当主机"）与主机角色（`--host` 报端口 + WebSocket 握手 + 快照）。

## 二、开发与验证
```bash
bun install
bun run link        # 把本地 ../gpuix 的包连进来，克隆后必做一次
bun run dev         # 开发：保存即热重载
bun run typecheck   # 门一
bun test            # 门二；bun test src/agent 是真实回归线
bun run build       # 产出单一可执行文件 dist/a-da.exe
bun scripts/binary-check.ts   # 打包后验"单文件双角色"（需先 build）
```

- **`typecheck` 与 `bun test` 是两个独立的门，两个都要过**，别只跑一个。
- 应用是单文件编译产物，构建依赖同级 `../gpuix` 已 `bun install && bun run build`。
- 测试用 `A_DA_HOME` 指向临时目录（见 `scripts/test-preload.ts`），不要在测试里碰用户真实的
  `~/.a-da`。
- 写 store 相关测试时注意：`store` 是**模块级单例**，但也可以 `new AgentStore(workspace)`。
  工具内部是动态 `import('../../store')` 拿单例的，所以**测工具必须用单例**（`import { store }`），
  用 `new AgentStore()` 会测到一个工具根本看不见的实例。
- 测试里造「正在运行」的会话需要摸私有集合，惯用写法是
  `store as unknown as { runningThreadIds: Set<string> }`。

## 三、已知问题

### 1. 【已修复，陷阱仍在】GPU 测试渲染器：可视区外的元素"看得到、点不到"

`PluginsDialog.test.tsx` 曾有两条长期失败的用例（`creates custom prompt…` 与
`switches to skills tab…`），根因已定位并修复（2026-09）：

弹窗主体是滚动容器，**新建的条目排在十几个内置项之后，落在可视区外**。
此时 `painted()` 仍能在绘制文本里看到它（getPaintedText 含被裁剪的内容），
但 `click()` 按**窗口坐标**派发，点在窗口外等于没点——handler 不执行、也不报错，
用例只能靠超时烧 11 秒后失败。实测按钮 y≈947 > 窗口 760。

**给后来者的两条铁律**（写需要"新建条目 → 点它的卡片按钮"这种用例时）：

- 先用 `mount()` 返回的 `scrollIntoView(locator)` 把元素滚进可视区再点
  （`PluginsDialog.test.tsx` 里有实现）；
- GPU 渲染器的滚轮方向是**反的**：`wheel(deltaY)` 为**负**才是向下滚；
  且滚到顶/底后同向滚轮是无操作（不报错、bounds 不变），别把它误判成"方向没反"。

修好后附带收益：这两条用例不再烧超时，全量测试从 ~136s 降到 ~35s。

### 2. 【已解决】"新增插件技能导致 src/ui 成组测试退化"的真相

先前记录为：只要存在一个非重复的插件技能，`bun test src/ui/` 就从
`98 pass / 1 fail / ~30s` 退化为 `81 pass / 18 fail / ~110s`，且单独跑任何
文件都是绿的。

真相：**技能数只是相关性，不是因果。** 机制是上面 §1 的两条用例——新技能把
技能/提示词列表撑高，卡片按钮越过 y=760 视口边界 → 点击落空 → 两条用例各烧
11 秒超时后抛错，且抛错发生在 `app.close()` 之前、泄漏原生窗口，**毒化了并发
跑的其他 UI 测试文件**（Bun 在同一进程内并发跑测试文件）。"多一个技能就炸"
只是因为那一个技能刚好把按钮推过了视口边缘。

修复 §1 后，带 decision 插件技能的全量测试稳定 `473 pass / 0 fail / ~35s`
（连跑三次验证）。修这类"成组退化"时先找**抛在 app.close() 之前的用例**，
它们是污染源。
