# a_da UI ↔ Agent 主机对接协议（JSON-RPC 2.0）

> 状态：**草案 v0.1，待评审**（本文只定协议，不动代码；§11 的"待拍板"已按建议定案）
> 目标：把现在"进程内单例 `store`"换成"UI 进程 ↔ agent 主机进程"的 JSON-RPC 2.0 对接面，
> **覆盖当前全部功能**，并为分阶段落地划清子集。
> **交付形态不变**：这是内部拆分，编译产物仍然只有一个 `dist/a-da.exe`（§0.2 第 4 条、§1.8）。
> 依据：实测当前实现——`src/agent/store.ts`（4065 行）、UI 对 store 的 **233 处引用 / 70 个不同成员**
> （把 `*.test.tsx` 也算进来是 460 处 / 79 个）、UI 直接调用的 **17 个管理器函数**、
> agent 侧反向抓 store 的 **7 处**。

---

## 0. 范围与不变量

### 0.1 两端各是谁

| 端 | 跑什么 | 不跑什么 |
|---|---|---|
| **UI 进程（客户端）** | GPUix 原生窗口与渲染、主题、快捷键、命令面板、剪贴板、系统通知、目录选择、资源管理器、窗口控制、markdown/diff 渲染、草稿、纯计算的展示数学 | 主循环、工具执行、插件代码、文件读写、配置/密钥 |
| **Agent 主机（服务端）** | `runAgentLoop`、LLM 调用、工具执行与 `checkWorkspaceSandbox`、检查点、会话落盘、插件加载与钩子执行（jiti）、技能/提示词/子智能体管理器、config 与 secrets、统计 | 任何渲染、任何窗口/原生 UI 能力 |

**这不是"把渲染搬到服务端"**：GPUix 是原生 GPU 渲染，跨进程传帧等于自建远程桌面。协议只承载**状态与命令**。

**本机默认形态**：两个角色在**同一个 exe** 里（无参数 = UI 自己 spawn 自己 `--host`），见 §1.8；
"两个进程"是运行期事实，"两个二进制"不是交付事实。

### 0.2 四条不变量

1. **后端权威**：会话、消息、工具卡、运行状态、审批等待、文件改动，真值都在主机。客户端是**只读复制 + 命令发起方**。
2. **客户端不得直接碰世界**：一切文件/配置/密钥/进程操作都走 RPC；客户端不做 `readFile`。
3. **纯 UI 状态不进协议**（见 §9.3）：弹窗开合、草稿、滚动位置等都是客户端本地状态。
4. **交付物仍然只有一个二进制**（硬约束，见 §1.8）：**这是内部拆分，不是交付形态的改变**。
   `bun run build` 依旧只产出 `dist/a-da.exe` 一个文件——用户双击的、CI 校验的、要发出去的，
   都是同一个 exe。不引入第二个可执行文件、不引入安装器、不要求用户配端口或启动第二个程序。

> 第 4 条是**否决性约束**：任何让交付变复杂的方案（两个二进制的 zip、必须先跑 host 再开 UI、
> 需要写配置文件告诉 UI 主机在哪）都不采纳。协议里一切"远端主机"的能力都是**同一套代码的额外用法**，
> 不是默认路径。

### 0.3 JSON-RPC 2.0 用法约定

- **一帧一消息**：WebSocket 文本帧承载一个 JSON-RPC 对象；**支持批处理数组**（单批建议 ≤ 32 条）。
- **params 一律 by-name（对象）**，不用 by-position：可读、可加字段。
- **id 空间分离**：客户端请求用**自增整数**；服务端请求（审批/提问）用**字符串**（如 `"srv-17"`）。两边永不撞号。
- **方向约定**（method 名前缀，非规范要求）：
  - `域.动作` —— 客户端 → 服务端**命令**（有响应）
  - `evt.*` —— 服务端 → 客户端**通知**（无 id，单向，可丢可合并，带 `seq`）
  - `req.*` —— 服务端 → 客户端**请求**（有 id，客户端必须应答）
- **两个规范没有的能力，用 `$/` 扩展**（LSP 惯例）：
  - 取消：`$/cancel`（客户端 → 服务端，带被取消请求的 id）
  - 进度：`evt.progress`（服务端通知，带原请求 id）

---

## 1. 连接与生命周期

### 1.1 传输

- WebSocket，子协议 `ada.rpc.v1`；本机默认 `ws://127.0.0.1:<port>/rpc`，远端必须 `wss://`。
- 客户端在 **URL query 或 `Sec-WebSocket-Protocol`** 里带令牌（见 §1.6）。
- 单连接多路复用：所有命令、事件、反向请求共用一条连接；不按域开多连接。
- **本机单文件模式**（默认路径，交付形态不变）见 §1.8：一个 exe 内部起两个角色，UI 自己 spawn 自己。

### 1.2 握手

```jsonc
// → 客户端
{ "jsonrpc": "2.0", "id": 1, "method": "session.initialize", "params": {
    "protocolVersion": "1.0",
    "client": { "name": "a-da-ui", "version": "0.1.0", "platform": "win32" },
    "capabilities": { "images": true, "streaming": true, "ui.notify": true }
} }

// ← 服务端
{ "jsonrpc": "2.0", "id": 1, "result": {
    "protocolVersion": "1.0",
    "server": { "name": "a-da-host", "version": "0.1.0", "appVersion": "0.1.0" },
    "capabilities": {
      "plugins": true, "skills": true, "prompts": true, "subagents": true,
      "changes": true, "debug": true, "background": true,
      "imageTransport": ["path", "dataUrl"],
      "maxFrameBytes": 4194304
    },
    "sessionId": "sess_7f3a",
    "seq": 0
} }
```

- 版本不匹配：`-32000 ProtocolVersionMismatch`，`error.data.serverVersion` 给出服务端版本。
- 握手前只接受 `session.initialize`；其余命令回 `-32001 Unauthorized`（或直接以关闭码 4401 断开）。

### 1.3 快照与序号（**重连正确性的地基**）

- 所有 `evt.*` 通知都带 `seq`（**单调递增，服务端全局**）。
- `session.snapshot` 返回完整可渲染状态 + 当时的 `seq`：

```jsonc
{ "jsonrpc": "2.0", "id": 2, "method": "session.snapshot", "params": { "include": ["threads", "ui", "config.public"] } }
// → result
{ "seq": 4211,
  "activeThreadId": null,                 // 服务端不持有 UI 焦点，这里只是"上次的"参考
  "threads": [ /* Thread DTO，messages 默认只带尾部 N 条，见 §2.4 */ ],
  "workspace": { "project": "E:/codes/x", "files": 418, "dirs": 51, "scanning": false, "entries": null },
  "config": { "approval": "auto", "effort": "max", "mode": "code", "model": "deepseek-chat",
              "contextWindow": 131072, "supportsImages": true },
  "pending": { "approvals": [ /* PendingApproval */ ], "questions": [ /* PendingQuestion */ ] },
  "plugins": { "diagnostics": [ /* PluginDiagnostic */ ] } }
```

- **缺口即重同步**：客户端发现 `evt.seq > lastSeq + 1` → 调 `session.resync { fromSeq }`；服务端能补发就补发，补不了回 `-32010 NeedResync`，客户端重新 `session.snapshot` 并**丢弃本地未确认 delta**。

### 1.4 保活

- 客户端每 20s 发 `session.ping`（普通请求，30s 超时）；服务端 60s 无任何入站帧则关闭（4408）。

### 1.5 断线语义

- **主机不因 UI 断开而停止**：正在跑的回合、后台命令、子智能体继续跑；事件在内存里按 `seq` 环形缓冲
  （建议 ≥ 2000 条或 5MB）供重连补发。
- 若断开期间"待审批/待提问"超时，按服务端策略收尾（默认超时 → 视为拒绝，`answeredBy: 'aborted'`），
  并在 `evt.approval.settled` 里如实说明。

### 1.6 认证与安全

| 场景 | 要求 |
|---|---|
| 本机 | 绑定 `127.0.0.1`；每次启动生成一次性令牌，通过 stdout/文件交给 UI；`Authorization: Bearer <token>` |
| 远端 | `wss://` + 长期令牌或配对码；令牌只存主机侧 secrets |
| 关闭码 | `4401` 认证失败 / `4403` 无权限 / `4408` 空闲超时 / `4409` 版本不符 |

- **服务端是信任边界**：客户端来的路径一律按"工作区之外"处理，交由 `checkWorkspaceSandbox` 判定；
  UI 需要的文件读取（如 diff 展示）走**白名单化的读接口**（`change.diff`），不提供通用 `file.read`。

### 1.7 多客户端

- 允许多个 UI 连同一主机。**审批与提问是独占资源**：
  - `req.approval.decide` / `req.question.ask` 默认广播给所有客户端（`target: "all"`，卡片都亮）；
  - **先答者生效**，其余客户端收到 `evt.approval.settled` 并撤下卡片；重复应答回 `-32006 AlreadyAnswered`；
  - 需要"只在某个客户端问"时用 `target: { clientId }`（客户端 id 由 `session.initialize` 的 `client.id` 给定）。

### 1.8 本机单文件模式：一个二进制，两个角色

> 这一节落实 §0.2 的第 4 条。**它对用户完全透明**：双击的还是那个 exe，没有任何新东西要装、要配、要开。

**同一个 exe 两个入口，靠 argv 分流**（`app.tsx` 现在不解析 argv，需要加这一段）：

| 启动方式 | 角色 |
|---|---|
| `a-da.exe`（无参数，即今天的行为） | **UI**：起 GPUix 窗口，并按需 spawn 主机 |
| `a-da.exe --host` | **主机**：只跑 agent（`store` 的后端那一半），不开窗口 |
| `a-da.exe --host --stdio` | 主机 + stdio 传输（调试用；`@gpuix/react/automation` 的 `connectStdio` 已是同类先例） |

**启动顺序（本机）**：

```
UI 进程（a-da.exe）
  ├─ 生成一次性令牌 token（32 字节随机）
  ├─ spawn(process.execPath, ['--host', '--port', '0', '--token', token])
  │    └─ 主机绑 127.0.0.1 的随机空闲端口，stdout 回一行握手行：
  │       {"ready":true,"port":51234,"pid":9876,"protocolVersion":"1.0"}
  ├─ 读到 ready 行 → 连 ws://127.0.0.1:51234/rpc（Authorization: Bearer <token>）
  └─ session.initialize → session.snapshot → 开始渲染
```

- **为什么本机也走 WS 而不是直接进程内调用**：协议只有一套，本机与远端走同一条码路——
  少一套"只在远端才走"的分支，就是少一类只在远端复现的 bug。进程内调用仅保留给**开发与测试**（见下）。
- **端口**：`--port 0` 让系统分配，避免固定端口冲突；stdout 的 ready 行是唯一的发现渠道，
  **不写任何配置文件**（用户不需要知道端口）。
- **令牌**：由 UI 生成、经命令行传给子进程、只在本机回环上用；主机不落盘。
- **生命周期**：主机默认**随 UI 退出**——UI 退出前 kill 子进程，主机自己也监听"父进程消失 / stdin 关闭"
  自杀，避免孤儿进程（Windows 上用 Job Object 兜底更稳）。`--detach` 留给"常驻主机 + 远端 UI"，
  **不是默认路径**。
- **开发与测试**：`bun run dev`（`bun --hot app.tsx`）与全部 UI 测试继续走 `InProcessTransport`，
  **不 spawn**——真窗口的单窗口约束（`AGENTS.md` §13）与测试速度都不受影响。
- **打包照旧**：`scripts/build.ts` 仍然只有一个 `outfile`（`dist/a-da.exe`）。因为**两个角色在同一个
  bundle 里**（同一个 `app.tsx` 按 argv 分流），不需要第二个 entrypoint、第二个产物或额外资源文件。
- **验收**：`scripts/binary-check.ts` 现在只验"启动并画出欢迎页"；阶段 F 起再加一条——
  **同一个 exe 能 spawn 自己 `--host`、完成一次 `session.initialize` 并把首帧画出来**，
  即"单文件 + 内部拆分"有可执行的证明。

**落地时要先验的四件事**（都属于工程细节，不影响协议）：

1. 编译后的 Bun 单文件 exe 用 `process.execPath` 再 spawn 自己、并正确收到 argv（Bun 编译产物应当支持；
   仓库里 `scripts/launch-own.ts` 已有 spawn 子进程 + stdio 通道的先例，但那是 `bun app.tsx` 形态）。
2. 随机端口的 ready 行在冷启动/杀毒软件拦截下的时序（给 `--host` 一个总超时 + 失败时回退到 `--stdio`）。
3. UI 被强杀（任务管理器）时子进程不残留——父进程消失检测 + Job Object 双保险。
4. **host 分支绝不能碰到渲染层**：`app.tsx` 现在顶部就 `import './src/platform/init'`、结尾 `render(...)`，
   静态 import 会连 GPUix 原生 addon 一起加载。落地时改成**动态 import 分流**：

   ```ts
   if (process.argv.includes('--host')) await import('./src/agent/host/main')
   else await import('./src/ui/main')       // 里面才 init 平台 + render
   ```

   否则 `--host` 进程会多一次原生初始化（最坏情况是多出一个空窗口），而"一个二进制两个角色"的
   前提是**两个角色互不牵连**。

---

## 2. 数据模型（线上 DTO）

### 2.1 直接复用（已是纯 JSON，无需改造）

| 类型 | 来源 | 说明 |
|---|---|---|
| `Thread` | `src/agent/types.ts:140` | `id/title/createdAt/workspace/items/messages/mode/parentId/subagentId/isSubagent/lastSystemPromptChars/lastToolSpecsChars/pluginData` |
| `Item` | `src/agent/types.ts:31` | user / assistant / thinking / tool / notice / compact 六类 |
| `AgentMessage` | `src/agent/core/types.ts` | `role/content/images/usage/...` |
| `AgentQuestion` | `src/agent/types.ts:20` | 提问卡（choices/allowText/status） |
| `DebugEntry` | `src/agent/types.ts:165` | 调试日志条目（**分页发**） |
| `LoadedPlugin` / `PluginItem` | `src/agent/plugins/types.ts`、`src/agent/tools/loader.ts` | 插件卡（含 `status/diagnostics/blockedTools/contributions`） |
| `SkillSummary` / `PromptItem` / `SubagentProfile` | skills / prompts / subagents | 管理页数据 |
| `PluginDiagnostic` / `ToolConflict` | plugins / tools/registry | 诊断与冲突 |
| `ThreadStats` / `ContextUsageSummary` | `src/agent/types.ts:87`、`src/agent/stats` | 统计 |
| `WorkspaceInfo` | `store.workspaceInfo` | `{files, dirs, scanning}` |

**只有 `AgentTool` 是不可跨进程的**（含 `execute` 函数）：协议里一律用工具**名**（`toolName`）与 JSON schema 文本表示，
实例只在主机内部按工作区构造。

### 2.2 新增的线专用类型

```ts
interface Page<T> { items: T[]; total: number; cursor?: string; hasMore: boolean }

/** 增量文本：只在客户端没有对应 item 时才需要 upsert，正常路径只发 delta */
interface MessageDelta { threadId: string; itemId: string; textDelta?: string; thinkingDelta?: string }

/** 工具卡更新：状态机推进 + 输出增量（与 store 的 ToolCard 字段一一对应） */
interface CardPatch {
  threadId: string; callId: string
  status?: 'awaiting' | 'running' | 'done' | 'error' | 'denied'
  outputDelta?: string
  patch?: string                       // 统一 diff 文本（改动审阅/回滚用）
  details?: Record<string, unknown>    // 结构化结果（dataUrls 走这里或走 file.*）
  errorMessage?: string
}

interface PendingApproval { callId: string; threadId: string; toolName: string; args: unknown
                            isWrite: boolean; mode: 'auto'|'ask'|'readonly'; reason?: string; at: number }
interface PendingQuestion { callId: string; threadId: string; question: AgentQuestion }

/** 列表用的会话摘要：`Thread` 去掉 `items`/`messages`（大字段不随列表发） */
type ThreadMeta = Omit<Thread, 'items' | 'messages'> & { itemCount: number; running: boolean }

/** 改动审阅的一行（现状是 `store.getThreadFileChanges()` 的内联返回结构，此处定型成线上 DTO） */
interface FileChange { path: string; latestPatch: string; additions: number; deletions: number
                       editsCount: number; reverted: boolean; cardIds: string[] }

interface Progress { id: number|string; done?: number; total?: number; label?: string; phase?: string }
```

### 2.3 id 的生命周期

| id | 谁生成 | 用途 |
|---|---|---|
| `threadId` | 服务端 | 会话键；子智能体会话也是普通 thread（`isSubagent: true`） |
| `itemId` | 服务端 | 会话流条目；**客户端不得自造**（重连后靠它对齐 delta） |
| `callId` | 服务端 | 一次工具调用；工具卡、审批、提问都以它为主键 |
| `planStepId` | 服务端 | 任务清单（`todo`）步骤 |
| 请求 `id` | 各自一端 | 见 §0.3 |

### 2.4 大对象与二进制

| 对象 | 策略 |
|---|---|
| 会话历史 `messages` | `thread.get {tail?: N}`；完整历史只在 `includeMessages: "all"` 时给（默认 tail 200） |
| 工作区文件清单 `entries` | 默认**不发全量**，只发 `{files, dirs}`；`workspace.entries {cursor, limit}` 分页（默认 200/页） |
| 调试日志 `log` | 环形缓冲（服务端建议上限 2000 条）；`debug.log {sinceId, limit}` 增量拉 |
| 图片附件 | 两种模式，由 `capabilities.imageTransport` 协商：`path`（同机，客户端传本地路径）/ `dataUrl`（远端，客户端内联 base64）；单帧超限回 `-32008 TooLarge`，客户端改用 `file.put` 分片 |
| 统一 diff | `change.diff {threadId, path}` 返回文本（客户端自己渲染，`patchStats` 是纯函数留客户端） |

---

## 3. 客户端 → 服务端：命令目录

> "现状"列指当前实现位置，用于逐条对照。所有命令的 `params` 都允许附 `clientTag?: string`（回显在事件里，便于多客户端定位）。

### 3.1 `session.*`

| method | params | result | 现状 |
|---|---|---|---|
| `session.initialize` | 见 §1.2 | 见 §1.2 | 新增 |
| `session.snapshot` | `{include?}` | 见 §1.3 | 新增（对应 `store` 初始状态） |
| `session.resync` | `{fromSeq}` | `{seq, events[]}` 或 `-32010` | 新增 |
| `session.ping` | `{}` | `{at}` | 新增 |
| `session.shutdown` | `{reason?}` | `{}` | 新增（`win32.ts` 的退出路径） |

### 3.2 `thread.*`

| method | params | result | 现状 |
|---|---|---|---|
| `thread.list` | `{workspace?, includeSubagents?}` | `ThreadMeta[]` | `store.threads` / `projects` / `projectThreads` |
| `thread.get` | `{threadId, tail?, includeItems?}` | `Thread` | `store.active`（改为按 id 取） |
| `thread.create` | `{workspace, mode?}` | `Thread` | `newThread()`（store.ts:1482） |
| `thread.delete` | `{threadId, cascade?}` | `{deleted: string[]}` | `deleteThread()`（:1217） |
| `thread.setMode` | `{threadId, mode}` | `Thread` | `setMode()` / `thread.mode` |
| `thread.setWorkspace` | `{threadId, workspace}` | `Thread` | `setThreadWorkspace()`（:1615） |
| `thread.send` | `{threadId, text, images?, clientTag?}` | `{accepted: true}` | `send()`（:1723） |
| `thread.abort` | `{threadId}` | `{aborted: boolean}` | `stop()`（:1759） |
| `thread.editAndResend` | `{threadId, itemId, text, images?}` | `{accepted: true}` | `editUserMessageAndResend()`（:2169） |
| `thread.compact` | `{threadId, customInstructions?, trigger}` | `{success, reason?}` | `compactThread()`（:3205） |
| `thread.stats` | `{threadId}` | `ThreadStats` | `activeThreadStats`（:286） |
| `thread.clear` | `{threadId}` | `{}` | 清空会话（若产品需要） |

### 3.3 `queue.*`（排队指令）

| method | params | result | 现状 |
|---|---|---|---|
| `queue.list` | `{threadId}` | `QueuedItem[]` | `store.queue`（:361） |
| `queue.promote` | `{threadId, index}` | `QueuedItem[]` | `sendQueuedImmediately()`（:2083） |
| `queue.remove` | `{threadId, index}` | `{text, images?}` | `removeQueuedItem()`（:2126） |
| `queue.clear` | `{threadId}` | `{}` | `clearQueue()`（:2149） |

### 3.4 `subagent.*`（运行态；**档案**见 `subagentProfile.*`）

| method | params | result | 现状 |
|---|---|---|---|
| `subagent.start` | `{subagentId, task, additionalContext?, parentThreadId?, async?}` | `{threadId}` | `startSubagentThread()`（:2233） |
| `subagent.steer` | `{subagentThreadId, message, summary?}` | `{status}` | `steerSubagentThread()`（:2701） |
| `subagent.resume` | `{subagentThreadId, instruction?, async?}` | `{threadId}` | `resumeSubagentThread()`（:2771） |
| `subagent.list` | `{parentThreadId?}` | `ThreadMeta[]` | `store.projectThreads` 过滤 |

> 门禁（`beforeSubagentStart`）留在主机；被拦下时 `subagent.start` 回 `-32004 Denied`，`error.data.gate` 带
> `{ allowed:false, judged, reason, calibrated }`——与今天 `gateSubagent` 抛的那句话等价。
> `check_subagent` / `await_subagents` / `send_subagent_message` 是**工具**，不是 UI 命令，不进本协议。

### 3.5 `approval.*` / `question.*`（客户端应答服务端请求）

| method | params | result | 现状 |
|---|---|---|---|
| `approval.listPending` | `{threadId?}` | `PendingApproval[]` | 卡片 `status:'awaiting'` + `pendingQuestions` |
| `approval.decide` | `{callId, approved}` | `{accepted}` | `decide()`（:1781）——**重连补答用的旁路** |
| `question.answer` | `{callId, choice?, text?}` | `{accepted}` | `answerQuestion()`（:1870）——同上 |
| `question.listPending` | `{threadId?}` | `PendingQuestion[]` | `pendingAnswerQuestions`（:1896） |

**首选路径是应答 `req.*`**（JSON-RPC 响应，见 §5）；`approval.decide` / `question.answer` 只用于
"断线重连后请求 id 已经丢了"的场景——主机按 `callId` 找回待决请求并结案。两条路径最终都走同一份判定，
**不存在"直接批准"的旁路**。

**语义要点（与今天一致，别改）**：
- 审批/提问的**超时与中止判定在服务端**；`answeredBy: 'user' | 'aborted'` 如实区分"用户说不"与"没人答"。
- 插件的 `beforeApproval` 策略仍在主机内联执行；它通过 `ctx.askUser` 触发的二次确认，对客户端就是一次普通的
  `req.approval.decide`（`reason` 里带插件给的高危理由）。

### 3.6 `workspace.*`

| method | params | result | 现状 |
|---|---|---|---|
| `workspace.info` | `{}` | `WorkspaceInfo` | `store.workspaceInfo`（:223） |
| `workspace.rescan` | `{}` | `{accepted}` → 完成后 `evt.workspace.scanned` | `refresh()`（:3394） |
| `workspace.entries` | `{cursor?, limit?, filter?}` | `Page<string>` | `store.entries`（:229） |
| `workspace.add` | `{path}` | `{project}` 或错误 | `addProject()`（:1547） |
| `workspace.remove` | `{workspace}` | `{removed: string|null}` | `removeProject()`（:1677） |
| `workspace.openPublic` | `{threadId?}` | `{workspace, threadId?}` | `openPublicWorkspace()`（:1515） |
| `workspace.projects` | `{}` | `{projects: string[], labels: Record<string,string>}` | `projects` / `labelFor` / `isPublic` |

### 3.7 `config.*`（**密钥永不回明文**）

| method | params | result | 现状 |
|---|---|---|---|
| `config.get` | `{scope: 'public'\|'provider'\|'appearance'\|'approval'\|'effort'}` | 对应 DTO | `readSavedConfig` / 各字段 |
| `config.set` | `{scope, value}` | `{ok, message?}` | `saveProvider()`（:1453）、`setApproval()`、`setEffort()`、`setAppearance()` |
| `config.checkProvider` | `{config?}` | `{ok, message}` | `checkProvider()`（:1471） |
| `config.presets` | `{}` | `ProviderPreset[]` | `PROVIDER_PRESETS` / `ProviderPreset`（`src/agent/config.ts:42,52`） |

### 3.8 `plugin.*`

| method | params | result | 现状 |
|---|---|---|---|
| `plugin.list` | `{workspace?}` | `PluginItem[]`（含 `status/diagnostics/blockedTools/tools/skills/prompts`） | `scanPlugins()` |
| `plugin.reload` | `{workspace?}` | `{names: string[]}` → `evt.plugin.changed` | `reloadPlugins()`（:1435） |
| `plugin.setEnabled` | `{pluginId, enabled, scope?: 'global'\|'workspace'}` | `{}` | `togglePlugin()` |
| `plugin.delete` | `{filePath}` | `{ok}` | `deletePlugin()` |
| `plugin.createTemplate` | `{scope, name, code?}` | `{filePath}` | `createPluginTemplate()` |
| `plugin.config.get` | `{pluginId}` | `Record<string, unknown>` | `readPluginConfig()` |
| `plugin.config.set` | `{pluginId, values}` | `{}` | `savePluginConfig()` |
| `plugin.secret.set` | `{pluginId, key, value}` | `{}` | `savePluginSecret()` |
| `plugin.secret.state` | `{pluginId}` | `Record<string, boolean>`（**只回"是否已设置"**） | `readPluginSecret(...).length > 0`（PluginsDialog:171） |
| `plugin.capabilities.get` | `{workspace?}` | `{capabilities, invalid}` | `readPluginCapabilities()` |
| `plugin.capabilities.set` | `{patch, scope?}` | `{}` | `savePluginCapabilities()` |
| `plugin.capabilitiesMeta` | `{}` | `CapabilitySwitchMeta[]` + `describePluginRestrictions` 的输出 | `plugins/capabilities-view.ts`（可留客户端，见 §9.3） |
| `plugin.conflicts` | `{}` | `ToolConflict[]` | `defaultToolRegistry.getConflicts()` |
| `plugin.builtinCatalog` | `{}` | `BuiltinToolInfo[]` | `BUILTIN_TOOLS_CATALOG`（**必须在服务端**：它要与真实注册表一致） |
| `plugin.diagnostics` | `{workspace?}` | `PluginDiagnostic[]` | `getPluginDiagnostics()` |

### 3.9 `skill.*` / `prompt.*` / `subagentProfile.*`

| method | params | result | 现状 |
|---|---|---|---|
| `skill.list` | `{workspace?}` | `SkillSummary[]` | `scanSkills()` |
| `skill.setEnabled` | `{skillId, enabled}` | `{}` | `toggleSkill()` |
| `skill.create` | `{name, description, content, scope?}` | `{path}` | `createSkillTemplate()` |
| `skill.update` | `{skillId, content}` | `{ok}` | `updateSkill()` |
| `skill.delete` | `{skillId, workspace?}` | `{ok}` | `deleteSkill()` |
| `prompt.list` | `{workspace}` | `PromptItem[]` | `scanPrompts()` |
| `prompt.setEnabled` | `{promptId, enabled, workspace}` | `{ok}` | `togglePrompt()` |
| `prompt.create` | `{workspace, name, description, content, scope, isSystem?}` | `PromptItem` | `createPrompt()` |
| `prompt.update` | `{item}` | `{ok}` | `updatePrompt()` |
| `prompt.delete` | `{filePath}` | `{ok}` | `deletePrompt()` |
| `prompt.composite` | `{workspace, mode}` | `{text, chars}` | `getCompositeSystemPrompt()`（**替代** Composer 的 `getCompositeSystemPromptSync`，见 §9.2） |
| `subagentProfile.list` | `{workspace?}` | `SubagentProfile[]` | `getSubagents()` |
| `subagentProfile.save` | `{profile}` | `{ok}` | `saveSubagent()` |
| `subagentProfile.setEnabled` | `{id, enabled, workspace?}` | `{}` | `toggleSubagent()` |
| `subagentProfile.delete` | `{id, workspace?}` | `{ok}` | `deleteSubagent()` |

### 3.10 `change.*`（改动审阅）

| method | params | result | 现状 |
|---|---|---|---|
| `change.list` | `{threadId}` | `FileChange[]`（见 §2.2） | `getThreadFileChanges()`（:1923，**由 items 派生**，客户端也可自算） |
| `change.count` | `{threadId}` | `{count}` | `getThreadChangeCount()`（:1914） |
| `change.diff` | `{threadId, path, cardId?}` | `{diff: string, additions, deletions}` | 卡片 patch / 检查点 |
| `change.revertCard` | `{threadId, cardId}` | `{ok}` | `revertCard()`（:2019） |
| `change.revertFile` | `{threadId, path}` | `{ok}` | `revertFile()`（:2035） |
| `change.revertAll` | `{threadId}` | `{ok}` | `revertAllChanges()`（:2056） |

> `revert*` 会写盘（走检查点），**必须**在主机；`change.list/count` 是纯派生，客户端可从 `items` 自算——
> 协议两者都给，实现时二选一（建议：先给 RPC，客户端算的版本作为离线降级）。

### 3.11 `debug.*`

| method | params | result | 现状 |
|---|---|---|---|
| `debug.state` | `{}` | `{currentModel, contextWindow, supportsImages, hostVersion}` | `currentModel` / `contextWindow` / `supportsImages` |
| `debug.log` | `{sinceId?, limit?}` | `Page<DebugEntry>` | `store.log`（:221，改用增量） |
| `debug.log.clear` | `{}` | `{}` | `clearLog()`（:3423） |
| `debug.trace` | `{text}`（通知语义，允许无响应） | `{}` | `store.trace()`（:994，UI 侧动作留痕） |
| `debug.hostInfo` | `{}` | `{appVersion, protocolVersion, platform, homeDir}` | `getAppHome()`（PluginsDialog 展示用） |

> **`debugOpen` 不进协议**：它是"调试面板开不开"的 UI 开关（`store.ts:207`），主机侧 `trace()` 无论如何都记录
> （`store.ts:994-997` 不判这个标志）。面板开合纯客户端。

### 3.12 焦点上报（客户端 → 服务端**通知**）

| method | params | 说明 |
|---|---|---|
| `ui.activeThread` | `{threadId: string \| null}` | 焦点变化（`selectThread`/`selectProject`/`openPublicWorkspace` 后）。**不是为了同步焦点**（焦点是客户端的），而是让主机知道该为哪个工作区准备上下文：现在 `store.selectThread` 会触发 `refresh()`（重扫工作区 + 重载插件）与 `onThreadSwitch` 插件钩子，这两件事在主机侧 |

> 这是本协议唯一"客户端通知主机 UI 状态"的地方，原因写在 §11.1——插件加载目前挂在"当前焦点"上，
> 拆分后必须改成按 workspace，这个通知是过渡期的兼容手段。

### 3.13 `$/cancel`

```jsonc
{ "jsonrpc": "2.0", "method": "$/cancel", "params": { "id": 42, "reason": "user" } }   // 通知
```
- 支持取消的命令：`workspace.rescan`、`plugin.reload`、`change.revertAll`、`thread.compact`、`session.snapshot`。
- **不支持**取消的：`thread.send`（用 `thread.abort`）、`approval.decide`（本身就是应答）。
- 被取消的请求回 `-32009 Cancelled`。

---

## 4. 服务端 → 客户端：通知（`evt.*`，无 id，带 `seq`）

| method | params | 何时发 | 客户端动作 |
|---|---|---|---|
| `evt.thread.upserted` | `{thread: ThreadMeta\|Thread}` | 会话新建/重命名/工作区变更/子会话建立 | 侧栏与标签栏更新 |
| `evt.thread.removed` | `{threadId, cascade: string[]}` | 删除（含级联） | 关标签、清本地缓存 |
| `evt.thread.items` | `{threadId, reset: true, items: Item[], tail: number}` | 重连/压缩/大幅改写 | 整段替换 |
| `evt.item.upserted` | `{threadId, item: Item}` | 新条目（用户消息、工具卡、通知行、提问卡） | 插入或替换同 id |
| `evt.item.patch` | `{threadId, itemId, patch: Partial<Item>}` | 条目字段变化（非流式） | 合并 |
| `evt.message.delta` | `MessageDelta` | 流式（**合并后发**，见 §7.1） | 追加文本/思考 |
| `evt.card.updated` | `CardPatch` | 工具卡状态机与输出增量 | 合并卡片 |
| `evt.queue.updated` | `{threadId, queue: QueuedItem[]}` | 排队变化 | 替换队列 |
| `evt.thread.running` | `{threadId, running: boolean, waiting?: boolean}` | 开始/结束/等待审批 | 状态灯、发送按钮 |
| `evt.workspace.scanned` | `{files, dirs, at}` | 扫描完成 | 侧栏计数 |
| `evt.log.appended` | `{entries: DebugEntry[]}` | 调试日志（批量） | 追加（本地环形上限） |
| `evt.stats.updated` | `{threadId, stats: ThreadStats, context?: ContextUsageSummary}` | 回合结束/压缩后 | 底栏与上下文环 |
| `evt.plugin.changed` | `{names: string[], diagnostics: PluginDiagnostic[]}` | 启停/重载/删除 | 刷新插件页 |
| `evt.change.count` | `{threadId, count}` | 卡片写入/回滚后 | 标题栏 `改动 N` |
| `evt.approval.pending` | `{approval: PendingApproval}` | 闸门挂起 | 亮出审批卡 |
| `evt.approval.settled` | `{callId, approved, answeredBy, by?: clientId}` | 已决/超时/中止 | 撤卡（多客户端同步） |
| `evt.question.pending` | `{question: PendingQuestion}` | `ask_user` 挂起 | 浮出提问卡 |
| `evt.question.settled` | `{callId, answeredBy}` | 已答/中止 | 撤卡 |
| `evt.progress` | `Progress` | 长命令阶段 | 进度提示 |
| `evt.notify` | `{level: 'info'\|'warn'\|'error', text}` | 事件流提示（原 `store.push`） | 顶部提示条 |
| `evt.host.log` | `{text}` | 主机侧 trace 文本 | 调试面板 |

---

## 5. 服务端 → 客户端：请求（`req.*`，有 id，客户端必须应答）

| method | params | 客户端 result | 超时 |
|---|---|---|---|
| `req.approval.decide` | `PendingApproval & { timeoutMs? }` | `{approved: boolean}` \| `{aborted: true}` | 服务端判超时（默认 120s） |
| `req.question.ask` | `PendingQuestion & { timeoutMs? }` | `{choice?: string, text?: string}` \| `{aborted: true}` | 同上 |
| `req.ui.notify` | `{level: 'info'\|'done'\|'error', title, body?, threadId?}` | `{shown: boolean}` | 10s（失败不影响任务） |

> **不放 `req.confirm` / `req.ui.reveal` / `req.ui.copy`**：删除确认、在资源管理器中打开、复制到剪贴板
> 都是**用户动作的本地结果**，主机没有理由发起（真要发起也行，但没必要入协议）。
> `req.ui.notify` 保留，因为"任务完成弹通知"现在是主机侧在收尾时发起的（`store.drain` 里的完成通知）。

**审批回合示例**（一次工具调用被闸门拦下）：

```jsonc
// ← 服务端请求（id 是字符串，与客户端整数 id 不冲突）
{ "jsonrpc": "2.0", "id": "srv-3", "method": "req.approval.decide",
  "params": { "callId": "call_88", "threadId": "t1", "toolName": "run_command",
              "args": { "command": "git push --force" }, "isWrite": true,
              "mode": "ask", "reason": "命中高危模式「git push」", "at": 1759000000000 } }

// → 客户端应答
{ "jsonrpc": "2.0", "id": "srv-3", "result": { "approved": false } }

// ← 服务端随后广播（其他客户端撤卡）
{ "jsonrpc": "2.0", "method": "evt.approval.settled",
  "params": { "seq": 4212, "callId": "call_88", "approved": false, "answeredBy": "user", "by": "ui-a" } }
```

---

## 6. 错误码

标准：`-32700` 解析失败、`-32600` 非法请求、`-32601` 方法不存在、`-32602` 参数非法、`-32603` 内部错误。

应用级（`-32000..-32099`，`data` 必须结构化，便于界面直接说人话）：

| code | 名称 | 典型场景 | `data` |
|---|---|---|---|
| -32000 | `ProtocolVersionMismatch` | 握手版本不符 | `{serverVersion, minClient}` |
| -32001 | `Unauthorized` | 令牌无效/未握手 | `{}` |
| -32002 | `NotFound` | thread/callId/pluginId 不存在 | `{kind, id}` |
| -32003 | `NotReady` | 扩展未加载完、工作区未索引 | `{what, retryAfterMs?}` |
| -32004 | `Denied` | 子智能体门禁拦下、审批拒绝、插件策略拒绝 | `{gate?, reason}` |
| -32005 | `Timeout` | 审批/提问/长命令超时 | `{callId, timeoutMs}` |
| -32006 | `AlreadyAnswered` | 多客户端抢答 | `{callId, by}` |
| -32007 | `WorkspaceDenied` | 路径越出工作区（`checkWorkspaceSandbox`） | `{path, workspace}` |
| -32008 | `TooLarge` | 帧/图片超限 | `{limitBytes, gotBytes}` |
| -32009 | `Cancelled` | `$/cancel` 生效 | `{id}` |
| -32010 | `NeedResync` | seq 缺口无法补发 | `{fromSeq, currentSeq}` |
| -32011 | `Busy` | 同一 thread 已有命令在跑且不可并发 | `{threadId, runningCommand}` |
| -32012 | `ConfigInvalid` | 配置写入被拒（类型/取值） | `{key, invalid: string[]}` |

---

## 7. 并发、取消、进度

### 7.1 流式事件合并（**体验与带宽的关键**）

- 主机侧对 `evt.message.delta` 做**合帧**：同一 `itemId` 的增量按 ~16–33ms 或 ~1–4KB 合并后再发；
  一帧里同一 item 只出现一次（`textDelta` 拼接）。
- `evt.log.appended` 也合并（≥100ms 或 ≥50 条）。
- 客户端**不得**依赖"每个 token 一个事件"；也不能依赖 delta 一定成对出现（用 `evt.item.upserted` 对齐）。

### 7.2 顺序保证

- 同一 `threadId` 的命令在服务端**串行**（沿用今天的队列语义：`queues` + `drain`）；跨 thread 并行。
- `evt.*` 的 `seq` 是**全局单调**；客户端按 `seq` 排序应用，乱序则等待或触发重同步。

### 7.3 进度

```jsonc
{ "jsonrpc": "2.0", "method": "evt.progress",
  "params": { "seq": 4213, "id": 42, "done": 3, "total": 9, "label": "扫描工作区" } }
```

---

## 8. 版本与兼容

- `protocolVersion`（`major.minor`）：**major 不匹配拒绝握手**，minor 只做能力位协商。
- `capabilities` 是**加法**：新能力（如 MCP、多模态）先加能力位，客户端按位降级，不认识的字段一律忽略。
- 未知 `method` → `-32601`；未知 `evt.*` → **静默忽略**（前向兼容）。
- 与服务端 `engines` / `APP_VERSION`（`src/agent/version.ts`）的关系：协议版本管**协议**，`engines` 管**插件与应用**，两者独立。

---

## 9. 覆盖度自检（"当前所有功能"逐条对账）

### 9.1 UI 实际用到的 70 个 store 成员 → 逐条归类（四组不重叠，合计 70）

**A. 进协议（命令，29 个）**
`addProject`、`answerQuestion`、`checkProvider`、`clearLog`、`clearQueue`、`compactThread`、
`decide`、`deleteThread`、`editUserMessageAndResend`、`getThreadChangeCount`、`getThreadFileChanges`、
`newThread`、`openPublicWorkspace`、`refresh`、`removeProject`、`removeQueuedItem`、
`resumeSubagentThread`、`revertAllChanges`、`revertCard`、`revertFile`、`saveProvider`、`send`、
`sendQueuedImmediately`、`setApproval`、`setEffort`、`setMode`、`setThreadWorkspace`、`stop`、`trace`

**B. 进协议（快照字段 / 事件流，16 个）**
`active`（→`thread.get`）、`approval`、`contextWindow`、`currentModel`、`effort`、`entries`（→分页）、
`isThreadRunning`、`isThreadWaiting`、`log`（→分页增量）、`pendingAnswerQuestions`（→`question.listPending`）、
`projects`、`projectThreads`、`queue`、`supportsImages`、`threads`、`workspaceInfo`

**C. 留客户端（纯 UI / 纯派生，23 个）**
`activeId`、`appearance`、`applyPromptToComposer`、`changesOpen`、`clearPendingDraft`、`debugOpen`、
`isPublic`、`labelFor`、`mode`、`openTabs`、`pendingDraft`、`project`、`running`、`setChangesOpen`、
`setPaletteOpen`、`setPlugins`、`setSettings`、`showConfirm`、`subscribe`、`toggleAppearance`、
`toggleDebug`、`closeTab`、`openTab`
（`project`/`running`/`mode` 由 `activeId` 与已复制的 thread 派生；`labelFor`/`isPublic` 是纯展示换算；
`openTab`/`closeTab`/`openTabs` 是标签栏状态。）

**D. 焦点上报（客户端 → 服务端通知，2 个）**
`selectThread`、`selectProject` → `ui.activeThread`（见 §3.12）。

> 对账结论：**A+B+D = 47 个进协议**，C = 23 个留在客户端；47 + 23 = 70 ✓（= 实测 UI 用到的 70 个）。
> 另有 UI 没直接用、但同样需要命令的四个方法：`startSubagentThread`、`steerSubagentThread`、
> `requestUserAnswer`、`isAwaitingAnswer`（已补在 §3.3–3.6），它们**不计入上面四组**。

### 9.2 UI 直接调用的 17 个管理器函数 → 对应方法

| 现状调用 | 协议方法 |
|---|---|
| `defaultExtensionLoader.scanPlugins/togglePlugin/deletePlugin/createPluginTemplate` | `plugin.list/setEnabled/delete/createTemplate` |
| `defaultPromptManager.scanPrompts/togglePrompt/createPrompt/updatePrompt/deletePrompt` | `prompt.*` |
| `defaultPromptManager.getCompositeSystemPromptSync`（**渲染期同步读盘**，`Composer.tsx:153/233`） | 删掉：用 `Thread.lastSystemPromptChars`（已存在），或 `ContextUsageSummary` 里 `source:'system_prompt'` 那条的 `chars` |
| `defaultSkillManager.scanSkills/toggleSkill/createSkillTemplate/deleteSkill` | `skill.*` |
| `defaultSubagentManager.getSubagents/toggleSubagent/deleteSubagent` | `subagentProfile.*` |
| `readPluginCapabilities` / `savePluginCapabilities` | `plugin.capabilities.get/set` |
| `readPluginSecret` / `savePluginSecret` | `plugin.secret.state` / `plugin.secret.set` |
| `savePluginConfig` / `readPluginConfig` | `plugin.config.get/set` |
| `readSavedConfig`（SettingsDialog:196） | `config.get` |
| `BUILTIN_TOOLS_CATALOG`（PluginsDialog:12） | `plugin.builtinCatalog` |
| `getAppHome` / `PUBLIC_WORKSPACE_LABEL`（展示用） | 快照里给 `homeDir` / `publicWorkspaceLabel`，或留客户端常量 |
| `getModelContextWindow` / `computeContextBreakdown` / `patchStats` / `parseHookTimeout` / `CAPABILITY_SWITCHES` / `describePluginRestrictions` | **纯函数，留客户端**（输入来自快照/事件） |

### 9.3 明确**不进协议**的清单（14 项）

窗口控制（`platform/win32`）、剪贴板读写、系统通知、目录选择（`platform/dialog`）、资源管理器（`platform/explorer`）、
快捷键与命令面板、标签栏开合、侧栏开合、搜索面板、滚动与贴底、草稿文本、markdown/代码/diff **渲染**、
主题配色应用、`patchStats`/`contextBreakdown` 这类纯展示数学。

> 判据：**它是否需要文件/进程/网络/模型**。需要 → 主机；只是"画" → 客户端。

### 9.4 功能对账（按用户可见功能）

| 功能 | 覆盖方法 |
|---|---|
| 发消息 / 流式回复 / 思考链 | `thread.send` + `evt.item.upserted` + `evt.message.delta` |
| 工具卡（状态、输出增量、diff、失败/拒绝） | `evt.card.updated` + `change.diff` |
| 审批（含插件策略、只读档、高危二次确认） | `req.approval.decide` + `evt.approval.*` |
| 向用户提问（`ask_user`） | `req.question.ask` + `question.*` |
| 排队指令（插队/删除/清空） | `queue.*` + `evt.queue.updated` |
| 会话（新建/切换/删除/标签/公共区） | `thread.*`、`workspace.openPublic` |
| 子智能体（派发/续跑/转向/等待） | `subagent.*` + `evt.thread.*`（子会话就是 thread） |
| 任务清单悬浮框 | `evt.item.upserted`（`todo` 条目；客户端自行收纳） |
| 计划 / 创造模式 | `thread.setMode`（+ 主机侧工具集变化经 `evt.stats.updated`/`thread.get` 反映） |
| 上下文压缩 | `thread.compact` + `evt.thread.items(reset)` + `evt.stats.updated` |
| 改动审阅与回滚 | `change.*` |
| 技能 / 提示词 / 子智能体档案 / 插件管理 | `skill.*`、`prompt.*`、`subagentProfile.*`、`plugin.*` |
| 设置（供应商/审批档/effort/自检/外观） | `config.*` |
| 统计与上下文占用 | `thread.stats` + `evt.stats.updated` |
| 调试面板 | `debug.*` + `evt.log.appended` |
| 工作区索引与文件数 | `workspace.*` + `evt.workspace.scanned` |
| 图片附件 | `thread.send.images`（`path` 或 `dataUrl`） |
| 后台命令（`run_background`/`check_task`/`kill_task`） | 工具，主机侧；界面只吃工具卡 |
| 完成通知 | `evt.notify` → 客户端调系统通知 |

---

## 10. 分阶段落地（协议子集）

| 阶段 | 协议子集 | 验收 |
|---|---|---|
| **A** 接口化（无网络） | 定义 `AgentClient`（本文 §3 的方法签名）+ `InProcessTransport`；UI 只依赖接口 | `typecheck` + 全量测试保持绿；UI 不再直接 import 管理器（编译器兜底） |
| **B** 事件化 | §4 全部 `evt.*`；客户端自持复制视图 | 流式文本、工具卡、队列行为与今天逐项等价 |
| **C** 真 WS（本机） | §1 全节 + §3.1/3.2/3.3 + §5 | 断线重连不丢状态；两个窗口同连互不干扰 |
| **D** 文件类 RPC | §3.6–3.11 | `PluginsDialog`/`SettingsDialog`/`SkillsPanel`/`ChangesPanel` 的 agent 侧 import 清零 |
| **E** 远端与多客户端 | §1.6/1.7、`req.*` 归属策略 | 远端 wss 可用；抢答/撤卡正确 |
| **F** 单文件双角色 | §1.8：`app.tsx` 按 argv 动态分流（`--host` / UI）；UI 用 `process.execPath` spawn 自己、连回环 WS | `bun run build` **仍只产出 `dist/a-da.exe`**；双击仍能启动；`scripts/binary-check.ts` 新增"自 spawn 主机 + 完成一次 `session.initialize` + 画出首帧" |

> **用户可见行为在 A→F 全程不变**：始终是"双击一个 exe、出一个窗口"。
> 阶段 A/B 完全不引入网络（`InProcessTransport`）；阶段 C 起本机默认走 §1.8 的自 spawn，
> 但那是实现细节——**交付物从第一步到最后一步都是同一个 `dist/a-da.exe`**。

---

## 11. 已定案（原"待拍板"，按建议定案）

| # | 事项 | 决定 | 理由与连带影响 |
|---|---|---|---|
| 1 | `activeId`（会话焦点）归谁 | **归客户端**；主机的插件加载改按 **workspace**，不再按"当前焦点" | 焦点是 UI 概念。现状 `store.project` 由 `active.workspace` 推导、`refresh()` 按它重载插件——不改的话两个窗口切焦点会互相重载插件。过渡期用 `ui.activeThread` 通知（§3.12），终态主机按"有会话可见/在跑"的 workspace 集合准备上下文 |
| 2 | 图片传输 | **先内联 `dataUrl` + 单帧上限 4MB**，超限明确报 `-32008 TooLarge`；分片 `file.put` 不做 | 现在 `images` 是本地 tmp 路径、后端在请求时读盘，而 `imageToDataUrl`（`agent-loop.ts:41`）**已经放行 `data:` 前缀**——后端不用改。分片要写一套续传，收益不抵复杂度 |
| 3 | `entries` 分页 | **200 条/页 + 客户端 LRU**；`session.snapshot` 只给 `{files, dirs}` | 现在是 418 个文件一次给；大仓库会变成兆级 JSON |
| 4 | 调试日志保留 | 主机**内存环形 2000 条，不落盘**；导出由 UI 复制当前缓冲 | 落盘要处理轮转与隐私（日志里含路径与命令），收益低 |
| 5 | 多客户端审批 | **默认广播抢答**（`target: 'all'`），先答者生效、其余收 `evt.approval.settled` 撤卡 | 与"用户决定"的取向一致；独占模式留给 `target: { clientId }` |
| 6 | `change.list/count` 谁算 | **主机 RPC 为准**，客户端从 `items` 自算仅作离线降级 | 两边都算会漂移；但派生规则是纯函数（`patchStats`），兜底成本极低 |
| 7 | **交付形态** | **仍然只交付一个二进制**：内部拆进程，交付物不变 | 见 §0.2 第 4 条（否决性约束）与 §1.8（`a-da.exe` 双角色、自 spawn、生命周期、打包照旧） |

### 11.1 落地时要实测的四件事（不是待拍板，是待验证）

1. 编译后的 exe 用 `process.execPath` 自 spawn 并正确收到 argv（Windows 上实测）。
2. 随机端口 + ready 行的冷启动时序、杀毒软件拦截（失败回退 `--stdio`）。
3. 强杀 UI 时子进程不残留（父进程消失检测，必要时 Job Object）。
4. host 分支不初始化渲染层（`app.tsx` 顶部静态 `import` 要改成动态分流，见 §1.8）。
