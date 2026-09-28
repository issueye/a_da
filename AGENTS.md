# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**（见
`README.md` 的「项目说明」一节），所以只写真正会咬人的东西，不重复 README 的项目介绍。

项目速览：Bun + TypeScript 的本地 AI 编码 Agent，UI 用 GPUIX（React 风格 GPU 渲染）。
模型侧全部在 `src/agent/core`，`src/agent/store.ts` 是运行时与界面之间的状态层。

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
`isWriteTool(name)` 就是 `!READ_ONLY.has(name)`。

- 新增**只读**工具却忘了加进 `READ_ONLY` → 它会被当成写工具，**只读子智能体与 plan 模式
  都拿不到**（mode 过滤器会把它剔除）。官方插件里真正只读的 `git_status`/`git_diff`/`git_log`/
  `get_outline`/`inspect_project` 就踩过这个。
- 反过来，把有副作用的工具错列进 `READ_ONLY` 会让只读模式形同虚设。判据是「是否改动工作区或
  系统状态」：`run_test_focused` 执行测试命令（可能产生构建产物），因此**算写工具**；
  `read_url_content` 只发网络请求，算只读。

扩展（工作区里的第三方代码）注册的工具默认按写处理，这是刻意的。

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

## 二、开发与验证

```bash
bun install
bun run link        # 把本地 ../gpuix 的包连进来，克隆后必做一次
bun run dev         # 开发：保存即热重载
bun run typecheck   # 门一
bun test            # 门二
bun run build       # 产出单一可执行文件 dist/a-da.exe
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

- `src/ui/PluginsDialog.test.tsx` 的 `creates custom prompt and applies prompt content to
  composer` 是一个**既有失败**（在未改动的 HEAD 上同样失败，与改动无关）。改动前后跑全量
  测试都会看到这 1 个红，不必为此改代码——但如果你碰巧修好了，记得说一声。
