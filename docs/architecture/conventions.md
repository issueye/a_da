# Agent 协作约定（原 AGENTS.md 正文）

> 本文件是根目录 `AGENTS.md` 的**深度约定全文**。为照顾 `docs/` 与源码注释里既有的
> `AGENTS.md §N` 引用，这里**原样保留了 AGENTS.md 的章节编号**（§1–§19）。
> 历史上被引作「AGENTS.md 第 N 节」的地方，指的都是本文件对应章节。
> 根 `AGENTS.md` 是导航版，会被自动注入系统提示词；细节按需来本文件查。

> ### ⚠️ 现行对照（文档整理时补，2026-10-18）
>
> 下面 §1–§15 的正文写于 **TS 时代**（2026-10-19 已从正文移除，只保留这张对照表），其中的文件路径与函数名**多数已不存在**
> （那套实现已整体删除）；仍然有效的是**约束本身**。要改代码时以右侧为准：
>
> | § | 约束（仍然有效） | 现行落点 |
> |---|---|---|
> | 1 | 子智能体的工具表要与工具目录同源，别写死白名单 | `agent_toolkit::registry::standard_tool_descriptors` + `filter_subagent_tools`（`crates/agent-node/src/subagents/`） |
> | 2 | 工具读写性**失败安全**：认不出的一律按写处理 | `agent_node::plugins::plugin_tool_is_write`（未知 → `true`）；元数据真源是 `ToolDescriptor.access` |
> | 3 | 子智能体只拿 `profile.system_prompt`，不拿主线程提示词 | `ProfilePrompt`（`agent-base` 端口 + `agent-node` 实现） |
> | 4 | 新写工具要能被检查点覆盖 | 描述符的 `RollbackPolicy`（不再有手写的 `CHECKPOINT_TOOLS` 名单） |
> | 5 | `terminate` 结束的是**整轮**，不是这一批 | 描述符的 `Termination` |
> | 6 | 审批一次只该问一件事；工具并发受策略约束 | `RunPolicy.max_parallel_tools`（**当前引擎未消费，实际始终串行**；改并发前先补语义断言） |
> | 7 / 8 | 委派与等待：别轮询，用挂起等待 | `AgentBus` 端口（`crates/agent-node/src/subagents/agent_bus.rs`；网关实现 `GatewayAgentBus` 在 `agent-rpc`） |
> | 9 | 决策：拿不到就失败，**绝不捏造确定性** | 决策插件已于 **2026-10-18 整体下线**（`decide` / `check_gate` / 工具包 / 实现一并删除）；只保留这条原则 |
> | 10 / 11 | 插件生效要"多处同时登记"；`LoadedPlugin` 是唯一产物 | 现行口径：贡献在**装配期结算**（工具 / 提示词 / 技能 / 钩子），见 [`../plugins/README.md`](../plugins/README.md) |
> | 12 | 钩子**成对**是硬约束；钩子不得扩张工具集 | `crates/agent-base/src/ports/hooks.rs`（`points::IMPLEMENTED`、`HookLevel`、`allows`） |
> | 13 | 审批与压缩的效力**刻意不对称** | 压缩点位 `beforeCompaction` / `afterCompaction`；审批见 §14 |
> | 14 | 审批：**策略**归插件 `approval-guard`、**执行**归核心 | `agent-node/src/plugins/builtins.rs`（`approval-guard`）+ 引擎 `ApprovalGate` |
> | 15 | 陷阱：断言**副作用**，别断言钩子被调用 | 现行诊断 `hook.entered` / `hook.finished` / `hook.skipped`（成对、缺席可见） |
> | 16 | 加 / 删协议方法要同步多处 | 现在是**四处**：`spec/proto/*.json` → `methods.rs` → `dispatch/` → `client-ts/methods.ts`（见 [`../../spec/proto/README.md`](../../spec/proto/README.md)） |
> | 17 | 打包后的 exe 用**应用日志**当证据 | 仍是现行口径（`launcher.log`，见 `electron/`） |
> | 18 | 工具结果是**结构化回执** | `status` / `duration_ms` / `started_at` / `finished_at`（界面耗时与状态徽章依赖它） |
>
> 编号**原样保留**（它们被源码注释大量引用），即使某条的正文已经是历史。

---

## 一、现行约定（§16–§19）

### 16. 界面只认 `ui/client` 的四条通道；加协议方法要**同步四处**

界面与 agent 侧是**真拆**：打包形态下 Electron 宿主监督 `ada-gateway`，
网关按工作区拉起 `ada-coding` 实例（见 [`../gateway/README.md`](../gateway/README.md)）。
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

**加一个协议方法要同步四处**（漏一处就是"文档有、界面调不到"或"界面调了、主机没实现"）：

1. `src/shared/protocol/methods.ts` —— 方法名 + `params` + `result`（`ParamsOf`/`ResultOf` 由它推导，
   类型会替你抓错）；
2. `src/agent/host/dispatch.ts` —— 主机侧实现（**唯一**允许直接调 store 与管理器的地方）；
3. 界面调用点 —— 走 `client.request`。

新方法**必须**在 `spec/proto/*.json` 里有登记条目，并在
[`../protocol/README.md`](../protocol/README.md) 里说清怎么用；实现期发现偏差就回写文档
（字段口径见 [`../../spec/proto/README.md`](../../spec/proto/README.md)，
别让文档承诺 A、实现是 B）。

还有两条拆分后新增的"会咬人"的点：

- **hooks 必须排在提前 return 之前**：`ComposerTelemetryBar` 曾在"空会话居中 `return null`"之后
  调用 `usePromptChars`，会话一有消息就触发 React 的 "Rendered more hooks than during the previous
  render"。这个错误**不会让测试失败**，只会出现在测试输出的 React 报错里——看全量输出时要留意。
- **进程本地状态别放错边**：`confirmModal` 带回调，永远只能留在客户端；快照里的字段必须是
  可序列化的纯数据（`ClientSnapshot` 是唯一口径）。

**文件/目录选择器（`FilePicker`）是窗口级模态层**，两件事别踩：

1. **别把选择器渲染在下拉/弹层内部**：`Select` 一关，它会被一起卸载（实测：点"添加工作区"后
   选择器闪一下就不见了）。它和确认框一样属于窗口级：请求经客户端本地状态传递
   （`client.ui.pickFiles({ mode, ... })`），由 `AgentWindow` 统一渲染一份。
2. **它那两条接线用"接缝测试"覆盖，不要写真窗口 e2e**：`picker-requests.ts` 里是两个纯函数
   （请求形状 + `onPicked` 之后该发什么命令），`picker-requests.test.ts` 不挂窗口、跑得飞快。
   真窗口 e2e 试过并放弃了——单独跑绿、全量跑红，诊断显示点击根本没到本窗口
   （`client.state.filePicker` 为空、下拉项计数 0），根因就是本文件 §13 末尾那条"只让一个真窗口活着"：
   bun 把各测试文件放在同一进程里并发跑，按坐标派发的 click 落到了别人的窗口上。
   与其把测试改成"重试到偶然通过"，不如把接线抽成可确定性测试的东西。


---

### 18. 工具结果是**结构化回执**，不是字符串——绕过它不报错，只让界面静默变空

每次工具调用都由 `agent-base` 引擎的工具执行出口统一收尾，`AgentMessage::ToolResult`
的 `content` 就是一份 pretty JSON 回执（模型读到的也是它）：

```json
{ "status": "success|error", "ok": true,
  "duration_ms": 12, "durationMs": 12,
  "started_at": …, "startedAt": …, "finished_at": …, "finishedAt": …,
  "data": <工具输出能解析成 JSON 时的那份>, "output": "<原始文本>", "details": { … } }
```

同一批值还会落到 `ToolResult` 的 `status` / `duration_ms` / `started_at` / `finished_at` 字段上。
读侧一律是"**先看字段，再回捞 content JSON**"，且**两种命名都要认**：

- `session/types.rs` 用 serde `alias` 同时接受 `duration_ms` 与 `durationMs`（老会话落盘的是 snake_case）；
- `session/manager.rs` 重建 `Item::Tool`（读历史 / 做快照）时，字段为 `None` 会从 content JSON 里回捞
  （两种命名都试），再交给 `state/store.rs` 的 `finish_tool_call` 落到工具条目上；
- 界面（SSR 的 `crates/agent-web/src/handlers/chat/rows.rs`）同理：
  `item.durationMs ?? structuredData?.duration_ms ?? structuredData?.durationMs`，状态徽章读 `structuredData.status`。

**坑的形状**：任何"不走执行器"的 `ToolResult`（拦下、超时、兜底、子智能体里的短路分支）如果直接把业务文本
塞进 `content`，**没有任何报错**——模型照样读得到，但工具卡片上的耗时、状态徽章、起止时间全是空的，
因为回捞逻辑只认上面那几个键。`subagents/runner.rs` 里"只读子体被拦下"那条分支就是照这份形状手工补的
（同样字段、`duration_ms: 0`）；新写这类分支时照它抄，别只写一段 `err_text`。

守门在 `agent-rpc` 的工具回执测试：断言 `content` 是合法 JSON、`parsed["status"] == "success"`，
且 `duration_ms` / `started_at` / `finished_at` 都在。只写 `assert!(content.contains("已写入"))` 那种断言
**测不出这个**（它只证明文本还在，证明不了回执还在）。

### 19. 文件工具的换行：**匹配按 LF 归一化，写回按文件 EOL 还原**

`read_file` 用 `str::lines()` 出内容，所以 **CRLF 文件里的 `\r` 永远不会出现在工具输出里**。
模型据此拼出的 `old_string` 只带 LF，而磁盘上是 CRLF——`edit_file` 早先直接 `after.find(&old_s)`，
跨行片段必然匹配不上，报"原文本在 … 中不存在"。实测表现是模型反复换写法重试
（一次成功一次失败），人看着像工具坏了。同一个坑还有两个变体：

- 反向（LF 文件 + 带 CRLF 的片段）同样匹配不上；
- 模型自己"转义过头"，把换行写成字面 `\r\n` 四个字符。**这时绝不能直接写回**——
  那会把 `\r\n` 四个字符塞进源码（`edit_file` 不解转义，`new_string` 原样落盘）。

现在的规矩在 `agent-toolkit/src/newline.rs`：

1. **匹配走 `LfView`**：把 haystack 折成 LF 再找，同时保留"归一化位置 → 原文位置"的映射表。
   命中区间必须映射回原文切（`\r\n` 是 2 字节，按长度加减会错位、切在字符中间还会 panic）。
   唯一性判定也在归一化视图上做，报错里带命中次数。
2. **写回按文件主导换行还原**（`apply_eol`）：只在命中区间里替换，其余字节原样保留。
   否则一次小改会把整份 CRLF 文件变成 LF，diff 变成"每行都动了"。
3. **`write_file` 覆写已存在文件时跟随其原有换行**（`follow_eol`）：新建文件不猜，给什么写什么。
4. **字面转义只有二级兜底**：`looks_newline_escaped` 要求"整段没有真实换行 + 含 `\n`/`\r`"才把
   `\r\n` 当换行解读。有真实换行就不猜——否则 `println!("a\nb")` 这种正常代码会被改坏。
5. **回执要说实话**：动用过归一化就带一句"（换行已按文件实际的 CRLF 归一化）"，
   别让模型以为原文就长这样。
6. **落空报错要给下一步**：说清文件实际换行、是否出现字面转义，并在能定位时指出
   "old_string 的首个非空行出现在第 N 行"——这一条把盲试变成了可执行的重试。

守门用例在 `fs_tools.rs` 的 `tests` 与 `newline.rs` 的单测里（LF↔CRLF 双向、多字节字符跨 CRLF、
批量 `edits`、写回保持约定、命中两次的计数、落空的行号提示）。
新增任何"按文本匹配再写回"的工具都走这套，别自己再写一遍 `find`。


---

## 二、开发与验证

```bash
cargo build --workspace                       # 基座 + 节点 + 桥接 + 产品
cargo test --workspace -- --test-threads=1     # 门二（必须串行：有共享全局态的用例）
bun run typecheck                             # 门一：Electron 主进程/预加载的 bun 构建（TS 前端已移除）
cargo xtask verify-wiring                     # 接线门：方法覆盖 / 描述符单源 / 端口实现 / 依赖方向
cargo xtask verify-spec                       # 声明门：产品/预设 spec 校验
cargo xtask verify-docs                       # 文档门：归档字样白名单 + 协议文档事件主题同步
cargo xtask verify --fast                     # 四道静态门一次跑完
bun run electron:dev                          # Electron 桌面端开发
bun run electron:pack                         # 打包 Electron 免安装版
```

- **`typecheck` 与 `cargo test` 是两个独立的门，两个都要过**，别只跑一个。
- **Rust 测试必须串行**：`AppHome` 是 `OnceLock`，提问链路仍有全局单例，并行会互相污染。
- 🔴 **测试绝不能写用户真实的 `~/.a-da`**：集成测试用 `AgentStore::with_home` / `TempAppHome`，
  插件与技能用 `PluginManager::with_home` / `SkillManager::with_home`；不要用 `std::env::set_var`。
- 本机若遇到 `os error 1455` / `link.exe 1102`，用 `CARGO_BUILD_JOBS=2`；`cargo` 默认 target 编译 `ring`
  报 MSVC `D8050` 时用 `CARGO_TARGET_DIR=../cargo_target_ada` 复用缓存。
- 打包后的 exe 没有控制台，二进制检查只能用应用日志（`launcher.log`）当证据。
