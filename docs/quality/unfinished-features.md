# 未完成功能清单

> **维护约定**：每条写清「现状证据（`file:line` 或 grep 判据）／影响／最小实现路径／状态」。
> 状态取值：`未排期`（没人做）、`待拍板`（缺一个产品决定）、`明确不做`（设计上划出去了）。
>
> **最后核对**：2026-10-18（文档整理时重核）。核对方式＝全仓 grep + 跑门禁，**不是凭印象**。
> 改动功能后请顺手更新本文件。
>
> **注意**：本文件只写"**功能缺口**"。接线类问题（"看起来装上了其实没接线"）
> 的唯一口径在 [`docs/plans/wiring.md`](../plans/wiring.md)——
> 那里有逐条证据与处置状态，不要在这里重复。

## 门禁基线（核对时）

| 命令 | 结果 |
|---|---|
| `cargo test --workspace -- --test-threads=1` | ✔ exit 0（2026-10-19 分拆后全量复跑；测试属性 666，须串行） |
| `cargo xtask verify` | ✔ 五阶段静态门 + 全量测试 |
| `cargo xtask verify-spec` | ✔ 无违约 |
| `cargo xtask verify-wiring` | ✔ 无违约（协议方法 **77** 个都有 `dispatch` 臂；端口 **13** 个都有生产实现） |
| `cargo xtask compat` | ✔ 无违约 |
| `cargo xtask verify-docs` | ✔ 无违约 |
| `bun run typecheck` | ✔ exit 0 |

> `verify-archive` 门已随 TS 归档**一并删除**（2026-10-17）；"仓库里没有第二份引擎"
> 现在由 `verify-wiring` 的端口实现与依赖方向断言继续守着。

---

## 一、产品功能缺口（按价值排序）

### 1. MCP client　`待拍板`

- **现状**：全仓 `grep mcp` **零命中**（`crates/` 与 `desktop-ui/` 都没有），设计文档也从未提及。
- **已评估的最小路径**：stdio 传输 + `tools/list` / `tools/call` 桥接进 `ToolCatalog`
  （`mcp__<server>__<tool>` 前缀），配置放 `~/.a-da/mcp.json`；resource / prompt 后置。
- **需要决定**：是否接受"外部进程可以注册工具"，以及它的工具按什么分类——
  按 `is_readonly` 的失败安全语义（AGENTS.md §2），**未登记的一律按写操作处理**（该审批就审批）。

### 2. 审批的持久化 allowlist（"以后都允许"）　`未排期`

- **现状**：审批档位只有全局三档 `auto | ask | readonly`
  （`crates/agent-proto/src/dto.rs` 的 `ApprovalMode`）。
- 🔴 **同时是一处"声明了没接线"**：`ApprovalGuardConfig.auto_approve`
  （`crates/agent-adapter/src/approval/`）存在，但**全仓零消费者**——
  即使有人往里填工具名也不会生效。详见接线计划的缺口台账。
- **最小实现路径**：给 `auto_approve` 接上配置项与协议入口（生产装配目前始终用默认空配置），
  审批卡片加一个"以后都允许"的勾选；配置落 `config.json`。

### 3. 会话全文搜索　`未排期`

- **现状**：`SessionManager::list_sessions_for_workspace` 能列出会话摘要；
  没有搜索**会话内容**的能力（无索引、无关键词过滤）。
- **最小实现路径**：扫工作区下各会话 JSONL 内容并支持关键词高亮过滤。
  会话已是 JSONL（`FsSessionStore` 落盘），逐行读即可，不需要引入索引。

### 4. 文件树与编辑器视图　`未排期`

- **现状**：侧栏列工作区与会话；工作区文件**不渲染成树**，看代码只能靠工具调用与搜索。
- **底盘已就绪**：协议有 `fs.roots` / `fs.list` / `fs.mkdir`，
  应用内的 `FilePicker` 已用它们做目录浏览。做文件树可直接复用 `fs.list`
  （已是"目录在前 + 截断如实报告"的形状）。
- **编辑是更大的工程**：当前定位是"改代码都经工具 + 改动审阅面板"。

### 5. 成本统计（计价）　`未排期`

- **现状**：用量遥测很细（`assistantMessage.usage` 有 prompt/completion/total tokens，
  界面有上下文用量细分），但**没有计价**：全仓 `grep pricing|pricePerMillion` 零命中。
- **最小实现路径**：给 `PROVIDER_PRESETS` 加每百万 token 单价，按 `usage` 累计到会话与全局。

### 6. 子智能体无法"续跑"　`明确不做（按当前设计）`

- **现状**：协议方法 `subagent.resume` 已在 W6-T1 **按 R3 删除**（原先是只回 `{ok:true}` 的桩，
  前端"恢复执行"按钮点了没反应）。**W4-T6 之后子智能体上下文刻意是临时的**
  （`EphemeralSessionStore`：一次性委派，不污染主会话），因此没有"可恢复"的会话。
- **如果要做**：得先决定"子智能体会话是否持久化"——那是对 W4-T6 设计决定的反转，
  不能顺手加回来。

### 7. `stats.promptChars` 缺前端消费者　`未排期`

- **现状**：W6-T2 把它从硬编码 `{1200, 800}` 改成了**实测值**（系统提示词长度 +
  工具 schema 字符数之和），但前端 `desktop-ui/src/utils/context-breakdown.ts`
  **从未被调用**，`systemChars` 输入也没人提供——界面仍在客户端估算。
- **最小实现路径**：在上下文用量 Popover 打开时调 `stats.promptChars`，
  把它作为 `buildContextBreakdown` 的输入。

### 8. ~~图片输入从未送进模型~~　`✅ 已完成（2026-10-14）`

- **现状**：已接通。`thread.send` / `thread.editAndResend` 的 `images` 经
  `dispatch::normalize_images` 归一成 **data URI** → `spawn_thread_loop` 的 `current_images`
  → `run_agent_turn` / `run_turn_with_engine` → `TurnRequest::with_images`
  → `format_messages_for_model` 挂到那条用户消息上 → 适配器按协议编成内容块
  （OpenAI `image_url` / Anthropic `source.base64` / Responses `input_image`）。
- **零 IO 边界**：`agent-base` 读不了盘，所以裸路径（老盘上恢复出来的会话）在 RPC 入口
  就被读盘转成 data URI；读不到的图**丢掉并记日志**，绝不把取不到的路径送进请求。
- **声明侧**：`ada-coding` 与 PM 预设都改为 `capabilities.images: true`，
  `NetworkModelClient` 声明 `images: true`（客户端能编码图片）；
  单个模型认不认图仍由 `ModelEntry.supportsImages` 表达——两者不是一回事。
- **守门**：`prompt_format` 的单测、适配器三家协议的编码单测、
  `engine_bridge::test_images_reach_the_model_request`（端到端到模型请求）、
  `dispatch::test_normalize_images_*`。
- **仍不在范围内**：`run_turn_with_images` 的门禁（W5-T4）只在 `agent-host` 直连路径上，
  RPC 生产路径没有二次校验——两份产品声明现在都是 `true`，所以不构成不一致。

### 9. 钩子点位　`已落地（P3a，2026-10-16；P4 复核修正 2026-10-17）`

- **现状（已按代码复核）**：`agent_base::ports::hooks::points::IMPLEMENTED` 列 **33 个点位**，
  **全部**都有真实调用方（对照表：`engine/runtime/` 轮内、`dispatch/` 管理面、
  `subagents/runner.rs`、`toolkit/core/todo.rs`）：
  - 工具：`beforeToolCall` / `afterToolCall`（拦下 / 旁注 / 改写）
  - 审批：`beforeApproval` / `afterApproval`　轮次：`beforeTurn` / `afterTurn`
  - 会话：`beforeThreadCreate` / `afterThreadCreate` / `beforeThreadDelete` / `afterThreadDelete`
  - Agent：`beforeAgentStart` / `afterAgentEnd`　压缩：`beforeCompaction` / `afterCompaction`
  - 模型：`beforeLlmRequest` / `afterLlmResponse` / `beforeProviderRequest` /
    `beforeProviderHeaders` / `afterProviderResponse`；上下文：`beforeContext` / `afterContext`
  - 提示词：`beforeSystemPrompt` / `afterSystemPrompt`　只读面：`beforePersist` /
    `afterCheckpoint` / `beforeSkillLoad` / `afterSkillLoad`
  - 子智能体：`beforeSubagentStart` / `afterSubagentEnd`　待办：`beforeTodoUpdate` / `afterTodoUpdate`
  - 插件启停：`beforePluginToggle` / `afterPluginToggle`
  成对埋点 `hook.entered` / `hook.finished` / `hook.skipped`。
- ⚠️ **本文此前的"仍未接"清单已过时**（把上表一大半点位列为未接），**已删除**：
  那些点位既在 `IMPLEMENTED` 里、也都有调用方。判断口径以
  `points::IMPLEMENTED` + 调用点为准，不要看旧清单。
- **两处修正（P4，2026-10-17）**：
  1. **`beforeTodoUpdate` / `afterTodoUpdate` 曾经"可注册不上"**：`HookLevel::of` 漏了这两个
     分支 → `register()` 一律以 `unknown_point` 拒收，而 `IMPLEMENTED` 与
     `REPLACE_SUPPORTED` 都声称它们可用、`todo` 工具也真的在发事件——生产链路上永远是
     `no_subscriber`。**已补分级（数据面）**，并加了门禁测试
     `test_every_implemented_point_is_gradable`（`IMPLEMENTED ⊆ HookLevel` 值域），
     同类矛盾不会再静默出现。
  2. **`onThreadSwitch` 不存在**（旧清单里列着它）：代码里没有这个点位常量，
     `HookLevel::of` 也没有它。要做请先加常量 + 调用点，别只登记名字。
- **改写（`Replace`）**：`beforeToolCall` 与 `beforeTodoUpdate`（数据面）已开，
  与"改写后重校验（形状 / schema / 路径沙箱）+ 风险单调性"同批落地——改写不是授权，
  改完还要过同一套闸门。载荷只有 `args`：工具名、读写性、描述符字段都改不动。
  **其余点位不支持改写**（`points::REPLACE_SUPPORTED` 是单一真源，声明会被装配期如实拒）。
- **"受信"档位不存在**：控制面点位的 `Deny` 因此对第三方一律关闭（只有内置插件能用）。
  将来要放开，应由 manifest 的显式字段驱动，不靠目录猜。
- **熔断未做**：只有"超时 = 无意见""脚本抛错 = 无意见"两条失败安全口径，
  没有对反复失败的插件降权/熔断。

### 10. i18n　`未排期`

- **现状**：文案硬编码中文（部分中英混排）。`grep i18n|useTranslation` 零命中。
- **注**：产品声明里有 `identity.locale`（W5-T4 已接进系统提示词），
  但**界面文案**仍与 locale 无关。

### 11. 插件能力开关　`已接线（P4，2026-10-17）`

- **背景**：`PluginCapabilities` 的 8 个位被界面渲染成"关掉后会发生什么"的开关，
  而其中**只有 `hookTimeoutMs` 有真实消费者**——其余 7 位只被解析、无人读取。
  用户关掉它们**什么都不会发生**，界面还在承诺（7 条用户可见的不实描述）。
- **现在的处置**：**7 位全部接上判定，1 位删除**：

  | 开关 | 判定点 |
  |---|---|
  | `allowBuiltinShadow` | 装配管线 `AssemblyBuilder::with_builtin_shadow`（关掉＝同名插件工具不注册、内置保留，跳过的记 `builtin_shadow_disabled`） |
  | `allowSystemPromptReplace` | `agent_host::ContributionPolicy::filter_prompt_fragments`（关掉＝插件提示词片段不进系统提示词，被挡的进 `plugin.skipped` 诊断） |
  | `allowThirdPartyHooks` | `HookRuntime::register`（关掉＝第三方钩子**装配期即拒**，理由 `third_party_hooks_disabled`；内置不受影响） |
  | `allowTextRewrite` | `HookRuntime::action_disabled_by_capabilities`（关掉＝`note` 旁注不再注入） |
  | `allowThreadDeleteBlock` | 同上（关掉＝`beforeThreadDelete` 的拦下不生效） |
  | `allowCompactionReplace` | 同上（关掉＝`beforeCompaction` 的拦下/改写不生效） |
  | `hookTimeoutMs` | `HookBudget`（单点位预算，早已消费） |
  | ~~`allowPlanModeHooks`~~ | **已删除**（见下） |

- **门禁**：`cargo xtask verify-wiring` 新增 **check L2** —— 逐个字段要求
  `PluginCapabilities` 的每一位在**生产代码**里有判定点（判据是读取点写法本身，
  排除定义处与"只解析不判定"的 `manager.rs`）。新加开关却没人读，会在门禁上直接红。
- 🔴 **读开关的时机是进程装配期**（与 `hookTimeoutMs` 同源）：改开关要**重启产品**
  才生效，与"声明式装配只发生在启动/启停"一致。别把它当成热生效。
- **`allowPlanModeHooks` 为什么删掉**：它的承诺是"plan 模式下不跑钩子"，而 Rust 引擎
  **不知道会话处于哪个 mode**——`AgentMode` 只被解析、存进 store、投影给界面，
  执行侧没有任何分支读它。也就是说，它连"要判什么"都不存在。要做得先把 mode 接进引擎，
  届时连开关一起加（界面文案已同步删除）。

### 12. 事件变体的发射者缺口（3 个豁免，其中 1 个口径不一致）　`部分已闭、口径待定`

- **现状**：`cargo xtask verify-wiring` 的 `check_event_emitters` 要求每个 `AgentEventBody`
  变体都在 `crates/agent-base/src/engine/**` 的生产段里有发射者。当前明细：
  **13 个变体、10 个有发射者、3 个无**（`QuestionAsked` / `SubagentStarted` /
  `SubagentFinished`），三者登记在 `UNEMITTED_EVENT_ALLOW` 豁免表里。
- **2026-10-18 复核（本条是复核后新登记的，别再照抄旧叙述）**：
  - `QuestionAsked` **已经有生产发射者**，只是它在**工具层**：
    `crates/agent-toolkit/src/core/ask_user.rs:301`（同文件 `:749-751` 断言事件真的落进事件池）。
    门禁只扫引擎层，于是它仍留在豁免表里。**这不是功能缺口，是口径不一致**：
    INV-6 的原话是"引擎是 `AgentEvent` 的唯一生产者"。二选一——把发射收回引擎，
    或承认工具层是合法发射方并改判据；**别让豁免表变成没人再看的假条**
    （`crates/agent-rpc/src/runner/mod.rs` 的模块注释此前也在照抄"`AskUserTool` 直接返回错误"，
    已同批更正）。
  - `SubagentStarted` / `SubagentFinished` **仍然没有任何生产发射者**
    （`engine_bridge` 把它们映射成 `None`）——**子智能体生命周期确实没有上报**：
    界面看不到"谁在跑、跑到哪了"。
- **最小实现路径**：在子智能体运行器的起止处发这两个事件
  （`crates/agent-node/src/subagents/runner.rs` 有天然落点）；补完**必须删掉对应豁免**，
  否则门会报"陈旧豁免"。

---

### 13. 并行工具执行　`未排期`

- **现状**：`RunPolicy.max_parallel_tools` 只被 `agent-runtime` 装进结构体，引擎的工具循环
  （`crates/agent-base/src/engine/runtime/mod.rs`）始终按 `for raw_call in &tool_calls` 串行；
  `Execution::ParallelSafe` 只在 `dispatch` 里做界面标签；`ada-coding` 的 `parallelTools: 4` 不生效。
- **影响**：产品声明与运行行为不一致。`verify-spec` 把"字段被生产代码读取"当成消费者，
  抓不到"读进结构体但没执行"这一类空转。
- **最小实现路径**：只对 `Execution::ParallelSafe` 且无需审批的工具做有界并发，补一条端到端断言；
  或者删掉 `parallelTools` / `Execution::ParallelSafe` 的对外承诺，把它们标成未实现。

---

## 二、明确不做的（设计上划出去了，别当缺口）

- **第三优先钩子点位**：模型选择、上下文超限前的主动裁剪、并发批次控制、错误 / 重试、
  用户输入改写。**判据**：这些点位不在 `points::IMPLEMENTED` 与 `HookLevel::of` 的值域内——
  要做请先加常量 + 调用点 + 分级，别只登记名字（旧清单里那个 `onThreadSwitch` 就是这么被删掉的）。
- **`casual` 惰性工具档位**：契约里保留了该取值，运行层**明确拒绝并记 trace**，
  不会假装降级成功。开发计划里标注为"不在 MVP"。
- **决策插件的引擎驱动"自动模式"**：每轮多一次引擎调用（成本与延迟翻倍），
  没有可用引擎时只能靠启发式——按本项目"拿不到真实判断就不假装有判断"的原则不做。
  已实现的是确定性工具路由。
- **`beforeTurn.replaceText` / `afterAgentEnd.appendNote`**：本项目没有"改写已渲染回复"与
  "向已结束会话追加旁注"的交付通道，声明它们只会变成静默失效，所以契约里没有这两个字段
  （原因写在 `AGENTS.md` §12）。
- **`cargo xtask gen`（协议四产物生成）**：W5-T1 判定不划算，**正式降级**为
  "手写 + 三对副本双向校验"（决策与依据见 `docs/architecture/kernel-design.md` §6.2）。
- **`enum` 化协议方法常量 + 编译器穷尽性**：同上，降级为散装 `&str` + 机械校验。

> **P4 之后仍然缺的（2026-10-16 记）**：
> - **子进程档没有内存限额**（SP-70 只做了一半的护栏）：档位本身已经落地
>   （`--script-sandbox subprocess` → `runner.exe` 子进程 + `SubprocessScriptHost`，
>   超时即 `kill`），所以"插件把自己跑死"已经被隔离住了。但**"它死之前能吃掉多少内存"
>   没有被限制**：给子进程设 RSS 上限要看平台（Windows Job Object / Linux rlimit），本档没做。
>   另一半仍然成立：**进程内档（默认）同样没有内存护栏**——Boa 没有堆上限 API，
>   宿主 `tokio::timeout` 也拦不住同步段里的内存暴涨。
> - **`net` 策略的配置面只有环境变量**（SP-72 的一半）：超时与域名白名单**已经生效**
>   （`NetPolicy`，策略先于请求），但来源是运维环境变量
>   （`A_DA_PLUGIN_NET_TIMEOUT_SECS` / `A_DA_PLUGIN_NET_ALLOW`）——
>   配置文件 + 界面（和 `hookTimeoutMs` 那样可编辑）还没做。
> - **`fs` 没有 read/write 分离**：`@ada-needs fs` 是"读写都能"，不是 §3.5 设计的
>   `fs: [read|write]`。
> - **运行时错误的行号是"转译后 JS"的座标**（source map 未做）：转译错误已经带原始
>   行列与出错行原文，运行时错误带文件/位置/栈，但 oxc 的 codegen 会重排行，
>   所以行号与原 TS 不一一对应。
> - **诊断面板只聚合到"来源"这一层**（SP-74 的余下部分）：插件卡已渲染自述诊断、
>   诊断日志已按来源聚合（条数 + 最近点位），但还没有跨会话的持久化汇总、
>   也没有按点位的二次分组。
> - **待办清单的"上一次"在内存里**（SP-63 的口径）：权威清单不在 Rust 侧（模型每次带全量、
>   界面据参数渲染），所以工具只为"算 diff"记了每线程上一版；**重启后第一份快照没有历史可比**，
>   此时 `afterTodoUpdate` 如实说"首个快照"，而不是把整张清单报成新增。

---

## 三、插件系统的小尾巴（不影响使用）

专题理由：罗列的是**经核实仍然成立**的条目（原插件系统开发计划的 M3 记录
随 2026-10-18 文档整理删除，需要时从 git 历史取回）：

1. `Thread.pluginData` 没有分区大小上限（未决问题，超限行为需先定：截断 / 拒绝 / 诊断）。
2. **工作区级的能力开关覆盖在 Rust 后端不存在**（旧文写的"已经能读"是对 TS 时代的描述，**已作废**）：
   `PluginManager::get_capabilities(_workspace)` 显式忽略工作区参数，`config_path()` 恒为
   `~/.a-da/config.json`；`pluginCapabilities` / `pluginConfig` 都只写全局那份。
   要做"仅本项目"得先让配置按工作区分区。
3. ~~插件卡不列"注册了哪些钩子点位"~~ → **已接线（P4）**：`PluginItem.hooks` 由装配期
   活体视图填入（只列**真的注册上**的点位），插件卡渲染成钩子胶囊。
4. **钩子内动态注册（`ctx.registerTool/registerSkill/registerSystemPrompt`）目前只收集、不生效**：
   沙箱会如实把请求交出来（`HookRuntime::take_registrations` / `data.registrations`），
   但**没有生产调用方**把它们装进 catalog / 提示词。收集处已打 warn 说清"不会生效"，
   免得作者以为注册成功了。要做：启停式的重装配已经就位（`LiveContributions`），
   接法是把这些注册项并进下一次装配，而不是就地改活体（那会在轮内改工具表）。
   **`ctx.registerHook` 已删除**（2026-10-17）：它与工厂期的 `ada.on` 重复，且
   `handlerExport` 在执行期根本找不到函数（`call_hook` 只认 `typeof handler === 'function'`），
   留着等于承诺一个永远不会跑的点位。钩子只能在装配期声明。

---

## 四、工程与交付注意

- **构建**：本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；
  加 `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。
- **Rust 测试必须串行**：`cargo test --workspace -- --test-threads=1`。
  根因是共享全局态（`AppHome` 单例、若干测试读 `~/.a-da`）；
  测试默认 home 已指向临时目录，但仍有个别用例共享进程级状态。
- **两个独立的门**：`bun run typecheck` 与 `cargo test` **都要过**，别只跑一个。
- **TS 测试已整体冻结**：`bunfig.toml` 已把旧实现排除在测试发现之外，
  跑 TS 测试只会得到 "No tests found"。**门就是 `cargo test` 与 `bun run typecheck`。**
- **打包产物检查**：产物是 GUI 子系统（无控制台），自动化通道到不了管道，
  所以二进制检查只能用应用自己的启动日志当证据（详见 `docs/architecture/conventions.md` §17）。
- **冷启动退化未测量**：打包形态多了"spawn 主机 + 握手 + 首帧快照"三步，
  只验过"能画出首帧"，没有与进程内形态对比过数字。
- **前端重连**：W6-T6 已改为指数退避（500ms 起 / 30s 封顶 / ±20% 抖动）+ 代次保护 +
  断线立刻失败在途请求；策略在 `desktop-ui/src/client/reconnect-policy.ts`，
  由 `cargo xtask verify-wiring` 的结构性断言守住。
- **CLI `run`**：W6-T3 已从"只建会话就退出"改为真执行（装配 → 跑一轮 → 落盘），
  退出码 0/1/2 分明；`--dry-run` 可离线验证装配。
- **网关不回收自己拉起的子进程（进程内优雅关闭会被拖住）**　`未排期`
  - **现状**：`supervisor.rs::ensure_agent` 把 `Child` 句柄交给守护协程**只 `wait()`**
    （设计上孤儿由子进程的父进程看门狗兜底），网关自己不 kill 任何子进程。
  - **后果**：进程内优雅关闭（tokio runtime drop）时，若子进程仍活着，它的
    stdout/stderr 管道 reader 会让关闭一直等下去。**实测**：`auth_e2e` 的作用域测试
    会真的拉起 `ada-coding`，测试体 1.4 秒跑完、之后**卡 40+ 分钟**；手动杀掉子进程后
    立刻通过（测试侧已改为预登记桩实例、不再拉真进程）。
  - **最小实现路径**：网关 shutdown 时显式 kill 自己拉起的全部子进程（注册表里有 pid），
    或把子进程句柄收进一个可 shutdown 的守护器。注意别破坏"网关死了 agent 也跟着死"
    这条已有的看门狗语义。

### 14. HTTP API 的 Phase 3（长活 / 流式 / 幂等）　`未排期`

- **现状**：`crates/agent-gateway/src/http_api.rs` 已把 Phase 1/2 的 **60 个 unary 方法**
  映射为 `/api/*`（18 查询 + 42 写），另有 11 条网关管理面；Bearer/Origin/作用域、
  `/api/snapshot` ETag、错误码映射都已接线。
- **明确没做**：`thread.send` / `retry` / `editAndResend` / `abort` / `compact`、
  `plugin.load` / `reload` / `validate`、`provider.fetchModels` / `config.checkProvider`
  仍未 HTTP 化——它们需要 `202 + turnId/jobId` 与事件流配套；Phase 2 写接口也还没有
  `Idempotency-Key` / `If-Match`（当前 last-write-wins）。
- **最小实现路径**：长活统一 `POST -> 202 {jobId}` + `GET /api/jobs/{id}` 或直接复用
  WS 事件；写接口补 `Idempotency-Key` 与按 thread 的版本号；把每请求一条短 WS
  换成按实例复用的多路复用连接。

### 15. SSR 合并后的已知边界（2026-10-25）　`未排期`

- **反向代理的 Host 白名单**　`✅ 已支持（2026-10-25）`：新增 `--web-public-url`
  （或 `A_DA_WEB_PUBLIC_URL`），例如 `--web-public-url https://example.com`。
  guard 会放行该公网 Host（有无端口均可）与对应 Origin，https 时登录 cookie 自动加 `Secure`；
  未配置时仍严格只认绑定主机 + 监听端口。**不信任 `X-Forwarded-*`**，必须显式配置。
- **受限 token 的工作区作用域未下传浏览器**：`/__auth` 接受网关 token 后只记录"已认证"，
  没有把该 token 的 workspace scope 带进 SSR 会话；页面仍绑定网关默认工作区。
  API 面（`/api/*`、`/rpc`）仍按 token 作用域拒绝越权，**UI 面不构成越权读取**，
  但受限 token 的用户能看到默认工作区的页面壳。
  **最小实现路径**：session 里存 `TokenScope`，`WebState` 在切换/新建工作区时校验。
- **独立 `ssr-h` 二进制仍保留**：`cargo run -p agent-web --bin ssr-h` 只用于开发/验收，
  不随 `dist/` 发布。是否删除或改为 feature 开关，`待拍板`。
- **TSX 插件没有内存上限**：Boa 只有循环/递归/输出预算与 500ms wall-clock 预算；
  恶意/失控插件仍可能吃满内存。插件被定义为**本地受信任内容**，不在此期收紧。

---

## 五、契约字段的界面消费者审计（2026-10-11）

方法：把 `crates/agent-proto/client-ts/dto.ts`（界面契约）里 171 个字段逐个在
`desktop-ui/src/**` 里查标识符。**28 个字段界面从不读**，逐个定性如下——
**不是都该修**，但每一条都要有归属（接线 / 明确留给别的客户端 / 删声明）。

### 已接线（本轮修掉）

- `FsListing.truncated` / `FsListing.omitted`：后端早就"如实报告截断"（`fs_service.rs`
  的 `MAX_ENTRIES`），但 `FilePicker` 从不显示 → 用户看到一个少了一半的目录却毫无提示。
  已在文件列表顶部显示"已截断显示（省略 N 项）"。

### 真缺口（建议排期）

| 字段 | 归属 | 说明 |
|---|---|---|
| `ServerCapabilities.resync` / `ServerEventsCapability.snapshotSeq` / `ClientSnapshot.seq` | 缺口检测 | 服务端发了单调 `seq` 与"支持重同步"能力位，**界面完全没用**：INV-6 的"缺口即重同步"在客户端尚未实现（当前靠全量快照兜住，所以不痛，但契约是空的） |
| `LoadedPlugin.diagnostics` / `PluginItem.diagnostics` / `PluginListResponse.diagnostics` | 插件诊断 | ✅ **已补全**（P4·SP-74）：扫描期如实写入"已停用 / 本页工具清单由源码文本推断（实际生效的贡献以脚本执行结果为准）"（`plugins/manager.rs`），界面由 `PluginDiagnostics` 渲染（**`info` 也显示**、`hint` 一起显示——只显示 warn/error 会让"正常"与"没被检查过"长得一样）。原先那条"执行面尚未接线"的 info 已在 P1 接线后删除，见下表 `plugin.setEnabled` 的 `report` |
| `LoadedPlugin.blockedTools` / `ResolvedPluginCapabilitiesDto.invalid` / `.overrides` | 插件诊断 | "哪些工具被挡住了、哪些声明无效、哪些被覆盖"界面不显示（与 §三.3 同族） |
| `PluginListResponse.secrets` | 插件密钥 | 密钥状态（是否已设置）界面不显示 |
| `plugin.setEnabled` 回执里的 `report`（SP-46 新增） | 插件启停 | 服务端现在会回"这次启停**实际**改变了哪些贡献"（工具/片段/技能的新增与移除、提示词字节变化），界面只刷新开关状态、不显示这份差异——**数据已就位，渲染待做**（P3a 接 `afterPluginToggle` 时一并处理） |
| `WorkspaceSnapshot.scanning` | 工作区扫描 | 扫描中状态界面不提示 |
| `SubagentProfile.allowedTools` | 子智能体 | 选子智能体时看不到它的工具白名单（只有 `systemPrompt` 的间接体现） |
| `PluginToolInfo.isWrite` / `.parameters` | 插件工具 | 插件工具卡不标"写工具"，也不展示参数 schema（**数据已由 P4 的活体视图填上**，只差渲染） |
| `FsEntry.mtime` | 文件选择器 | 只有大小没有修改时间 |

> **P4（2026-10-17）已修的两条**：
> - **插件贡献字段不再是空的**：`plugin.list` / `plugin.load` 读装配期的**活体视图**
>   （`agent_node::plugins::PluginContributionView`），磁盘插件的工具 / 技能 / 提示词片段 /
>   钩子点位终于有值——此前静态目录扫描让贡献计数恒为 0、核心工具页的"被插件重实现"永不亮。
>   启停时由 `LivePluginReassembler` 原地替换（与技能活体同一模式）。
> - `PluginItem.hooks`（新增字段）：插件卡列出"这个插件会动手做哪些事"。

### 明确留给别的客户端（不是缺口）

`UiSnapshot.{sidebarOpen, paletteOpen, searchOpen, pendingDraft}`：这些是**界面状态回显**，
桌面端自己持有状态、只写不读；保留是给 CLI / Web / 未来多端接入用的。
`ProductInfo.archetype`、`InitializeResult.sessionId`、`PluginManifest.author`、
`LoadedPlugin.declarative`、`PluginItem.isPackage` / `.sizeBytes`、
`ServerEventsCapability.granularity`、`ServerCapabilities.rollback`：信息性字段，界面暂时不需要。
`ServerCapabilities.hooks` 自 P3a 起**不再属于本条**（点位已接线，见 §一.9），界面暂未消费它。
`CapabilitySpec.streaming`（产品声明位）：引擎**恒定流式**，该位没有分支消费点，只用于向客户端
表达"本产品支持流式"。由 `verify-wiring` 的 check L 盯着——它要么在这里登记，要么就得接上消费者
（未实现的能力位不得声明为 true）。

---

## 相关文档

| 文档 | 用途 |
|---|---|
| [`docs/plans/wiring.md`](../plans/wiring.md) | **接线类问题的唯一口径**：缺口台账、任务表、执行记录、断言变更 |
| [`docs/architecture/kernel-design.md`](../architecture/kernel-design.md) | 设计口径（分层、端口、协议单源、装配） |
| [`AGENTS.md`](../../AGENTS.md) | 接手须知与 § 索引 |
| [`docs/protocol/README.md`](../protocol/README.md) | 接口文档（WS 线协议 + HTTP API 统一口径） |
| [`docs/plugins/README.md`](../plugins/README.md) | 插件平台现状（贡献模型、点位、能力开关、排错） |
| [`docs/quality/feature-catalog.md`](feature-catalog.md) | 已实现功能清单 |
