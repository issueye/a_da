# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**，因此只留
「接手就必须知道的事」；深度约定全文已迁至 **[docs/agent-conventions.md](docs/agent-conventions.md)**。

项目速览：Bun + TypeScript 的本地 AI 编码 Agent，UI 用 GPUIX（React 风格 GPU 渲染，无 Electron、
无 WebView）。模型侧全部在 `src/agent/core`，`src/agent/store.ts` 是运行时与界面之间的状态层。
**功能缺口清单在 [docs/unfinished-features.md](docs/unfinished-features.md)**：接活前先看一眼，
别把"已知未做"当成 bug 去修；做完一项顺手划掉。

## 开发与验证

```bash
bun install
bun run link        # 连本地 ../gpuix，克隆后必做一次
bun run dev         # 开发：保存即热重载
bun run typecheck   # 门一
bun test            # 门二；bun test src/agent 是真实回归线
bun run build       # 产出单文件 dist/a-da.exe（依赖同级 ../gpuix 已 build）
```

- **`typecheck` 与 `bun test` 是两个独立的门，两个都要过**，别只跑一个。
- 测试用 `A_DA_HOME` 指向临时目录（`scripts/test-preload.ts`），不要碰用户真实的 `~/.a-da`。

## 代码结构

- `src/agent/core` —— 模型侧全部逻辑（`agent-loop.ts` 主循环、`events.ts` 钩子契约）
- `src/agent/store.ts` —— 状态层（最大文件，运行时 ↔ 界面）；`src/agent/tools` 工具与官方插件
- `src/agent/{plugins,subagents,skills,prompts,compact,stats}` —— 各子系统
- `src/ui` GPUIX 界面组件；`src/platform` 原生能力（`win32.ts` 等）

## 三条会立刻绊倒你的规矩

- **测工具必须用 store 单例**（`import { store }`）：工具内部动态取单例，`new AgentStore()` 会测到
  一个工具根本看不见的实例。
- **UI 测试同一时刻只让一个真窗口活着**：按坐标派发的 `click` 会落到别的窗口上。
- **改核心前先查下面的 §索引**：每条都对应一个"看起来装上了、其实没生效"的静默坑。

## § 索引（`AGENTS.md §N` 一律指下表第 N 条，正文见 docs/agent-conventions.md）

| § | 一句话警告 |
|---|---|
| 1 | 子智能体 `allowedTools` 是写死白名单、与插件表**脱钩**：新工具不同步就白装 |
| 2 | `isWriteTool` **失败安全**：只读工具漏进 `READ_ONLY` 就被当写工具，plan/只读子体拿不到；非内置插件借走只读内置名也一律算写 |
| 3 | 子智能体只拿到 `profile.systemPrompt`，**没有** AGENTS.md 与主线程系统提示词 |
| 4 | 新写工具须进 `CHECKPOINT_TOOLS` 与 `checkpointPathsOf`（批量要逐文件进快照） |
| 5 | 工具结果的 `terminate: true` 结束的是**整轮**，不是"这批" |
| 6 | 全局 sequential，但整批都声明 `executionMode: 'parallel'` 时会重叠执行 |
| 7 | 子智能体收尾的 `wakeParent()` 必须在 `runningThreadIds.delete()` 之后；离线兜底单独补 |
| 8 | 别轮询 `check_subagent`，用 `await_subagents` 挂起等待 |
| 9 | 决策插件**绝不捏造确定性**：拿不到就失败，`calibrated` 仅 Jev 引擎为 true |
| 10 | 插件生效需**多处同时登记**（READ_ONLY／目录／allowedTools），各有守门测试 |
| 11 | `LoadedPlugin` 是唯一产物；第三方插件 id 由加载器定为「scope:文件名」 |
| 12 | 钩子**成对**是硬约束，工具集**只能收窄**；两处刻意偏差别当 bug 修 |
| 13 | 审批／压缩的效力**刻意不对称**；文末另有 UI 单窗口约束 |
| 14 | 审批：**策略**归插件 `approval-guard`、**执行**归核心 `askUser`；点位顺序不能反 |
| 15 | 陷阱：钩子"声明了却没人调用"是静默失效——**断言副作用**，别断言钩子被调用 |
| 16 | 界面只认 `ui/client` 四条通道；**加协议方法要同步三处**（有守门测试，改错方向会红） |
| 17 | 打包后的 exe **收不到自动化通道**（`console.log` 被劫持进日志）——二进制检查要用应用日志当证据 |

> 兼容说明：历史上写作「`AGENTS.md` §9」「`AGENTS.md` 第 1 条」的引用，按上表第 N 条理解。
> 编号在迁出后**原样保留**，故 `docs/` 与源码注释里的既有引用无需改动。
