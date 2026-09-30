# 未完成功能清单

> **维护约定**：每条写清「现状证据（`file:line` 或 grep 判据）／影响／最小实现路径／状态」。
> 状态取值：`未排期`（没人做）、`待拍板`（缺一个产品决定）、`明确不做`（设计上划出去了）。
> 最后核对：2026-09-29，核对方式＝全仓 grep + 跑门禁，**不是凭印象**。改动功能后请顺手更新本文件。

门禁基线（核对时）：`bun run typecheck` exit 0；`bun test src/agent` 470 pass / 0 fail；
全量 `bun test` 617 pass / 0 fail（79 个文件）。

---

## 一、产品功能缺口（按价值排序）

### 1. `@` 提及：界面在承诺一个不存在的功能　`未排期`

- **现状**：输入框占位文本写着 `描述要 Agent 完成的任务 (Ask anything, @ to mention, / for actions)`
  （`src/ui/Composer.tsx:1364`）。`/` 动作是真实实现（`src/ui/SlashCommandMenu.tsx`），
  `@` 没有：全仓没有 mention 解析或补全，`store.entries`（工作区文件清单）只被图片附件菜单用
  （`src/ui/Composer.tsx:525`，`AppendMenu` 里的图片过滤）。
- **影响**：用户照提示敲 `@` 什么都不会发生。这是全仓**唯一一处"界面在骗人"**。
- **最小实现路径**：仿 `SlashCommandMenu` 做触发 + 补全（候选来自 `store.entries`、技能库、
  子智能体列表），选中后插入引用并让模型看到；短期至少把占位文本里的 `@ to mention` 去掉。

### 2. MCP client　`待拍板`

- **现状**：全仓无 `mcp` 字样；设计文档也从未提及（`grep -rn "mcp" docs/*.md` 零命中）。
- **已评估的最小路径**：stdio 传输 + `tools/list` / `tools/call` 桥接进 `ToolRegistry`
  （`mcp__<server>__<tool>` 前缀），配置放 `~/.a-da/mcp.json`；resource / prompt 后置。
- **需要决定**：外部进程可以注册工具这件事是否接受，以及它的工具按什么分类——按
  `READ_ONLY` 的失败安全语义，未登记的一律按写操作处理（该审批就审批）。

### 3. 审批的持久化 allowlist　`未排期`

- **现状**：审批只有全局三档 `auto | ask | readonly`（`src/agent/store.ts` 的 `ApprovalMode`），
  没有"记住这个工具，以后别再问我"。
- **变通**：插件现在可以用 `beforeApproval` 实现等价策略（M3-5，见 `README` 的钩子一节）。
- **最小实现路径**：在 `config.json` 里按「工具名 + 工作区」记 allowlist，审批卡片上加一个
  "以后都允许"的勾选。

### 4. 会话管理缺三件：重命名 / 导出 / 全文搜索　`未排期`

- **现状**：`grep -rn "renameThread|exportThread|searchThreads"` 全仓零命中。会话能新建、切换、
  删除（删除是完整的：级联 + 归档钩子 + 缓冲清理）。标题来自首条消息摘要
  （`src/agent/store.ts:155` 的 `titleFrom`）或插件建议（M3-7）。
- **最小实现路径**：
  - 重命名：`defaultSessionManager.updateSessionTitle` 已经能改（M3-7 顺带做成"文件不存在则建"），
    只缺界面入口；
  - 导出：会话落盘就是 JSONL，导成 Markdown 需要一个渲染器；
  - 全文搜索：`listSessionsForWorkspace` 已能列出文件，扫文件内容即可。

### 5. 文件树与编辑器视图　`未排期`

- **现状**：侧栏只列工作区与会话，`store.entries`（`scanWorkspace` 的产出）不渲染成树；
  看代码只能靠工具调用与工作区搜索。
- **最小实现路径**：加一个可折叠的树组件复用 `entries`；代码编辑是更大的工程
  （当前定位是"改代码都经工具 + 改动审阅面板"）。

### 6. 成本统计　`未排期`

- **现状**：用量遥测很细（`src/ui/ContextUsagePopover.tsx` 与调试面板的 token 拆解，含系统提示词
  与工具开销），但没有计价：全仓无 pricing / cost 表。
- **最小实现路径**：给 `PROVIDER_PRESETS` 加每百万 token 单价，按 `assistantMessage.usage`
  累计到会话与全局。

### 7. 只有 OpenAI 兼容协议　`未排期（按需）`

- **现状**：`src/agent/ai/stream.ts` 只实现 OpenAI 兼容的 `chat/completions` + SSE；
  `src/agent/compact/policy.ts` 会按模型名认上下文窗口（gemini / qwen-long 等），
  但不支持 Anthropic Messages / Gemini 原生协议。
- **影响**：用 OpenAI 兼容网关（预设里的 DeepSeek、百炼、Moonshot、Ollama 等）不受影响；
  要直连 Anthropic / Gemini 官方端点则需要中间网关。

### 8. 无 i18n　`未排期`

- **现状**：文案硬编码中文（部分中英混排）。`grep i18n|useTranslation` 零命中。

### 9. 子智能体的两条"续跑"入口绕过 profile 白名单与门禁　`待拍板`

- **现状（行号已按当前代码核准）**：
  - `store.turn()`（`src/agent/store.ts:3632`）里取的是**主会话工具表**
    （`defaultToolRegistry.getToolsForMode(...)`，:3669），`kind: 'main'` 是写死的——**没有
    `thread.isSubagent` 分支**；
  - `store.steerSubagentThread` 对**已停止**的子智能体走"重新排队 + `drain`"（:2751–2769），
    不过门禁；
  - 用户在**子智能体标签页里直接输入**走 `store.send → drain → turn`，同一条路。
- **已经修掉的那一半**：`resumeSubagentThread` 现在**先过门禁**再动手（:2806–2812，判定输入
  `resumeGateTask`）。所以门禁接入点已有四处：`startSubagentThread`、`resumeSubagentThread`、
  `subagents/runner.ts`，**但 `turn()` 本身仍然不认识子智能体身份**。
- **影响**：只读 / 白名单是子智能体的权限声明，走这两条路时会静默失效——这条路上的子智能体
  能拿到写工具，且没有任何提示。
- **最小实现路径（两件事必须一起做）**：让 `turn()` 认识 `thread.isSubagent`
  （用 `resolveSubagentTools` 取表、`hookContext.kind = 'subagent'`），并给
  `steerSubagentThread` 的续跑分支补门禁（判定输入用 `resumeGateTask`）。
  只补门禁而工具表仍走主会话，等于把"门禁过了"与"权限没生效"拼在一起，更难查。
- **需要拍板**：子智能体标签页是否允许用户直接对话？若允许，它应当按子智能体身份运行
  （白名单 + 门禁）；若不允许，就该在 `send` 入口挡掉并引导用 `send_subagent_message`。
  （本轮建议：**允许，但按子智能体身份跑**——禁掉输入是功能倒退，而按身份跑既保留功能又补上权限。）

---

## 二、明确不做的（设计上划出去了，别当缺口）

- **第三优先钩子点位**：模型选择、上下文超限前的主动裁剪、并发批次控制、错误 / 重试、
  用户输入改写（见 `docs/plugin-system-design.md` §6.6.2 第三优先与 §6.8）。
- **`casual` 惰性工具档位**（core 阶段 B）：契约里保留了该取值，运行层**明确拒绝并记 trace**
  （`src/agent/plugins/hook-runtime.ts`），不会假装降级成功。开发计划里标注为"不在 MVP"。
- **决策插件的引擎驱动"自动模式"**：每轮多一次引擎调用（成本与延迟翻倍），没有可用引擎时只能
  靠启发式——按本项目"拿不到真实判断就不假装有判断"的原则不做。已实现的是确定性工具路由。
- **`beforeTurn.replaceText` / `afterAgentEnd.appendNote`**：本项目没有"改写已渲染回复"与"向已
  结束会话追加旁注"的交付通道，声明它们只会变成静默失效，所以契约里没有这两个字段
  （原因写在 `AGENTS.md` §12）。

---

## 三、插件系统的小尾巴（不影响使用）

明细与理由见 `docs/plugin-system-dev-plan.md` 的 M3 完成记录，此处只列条目：

1. `Thread.pluginData` 没有分区大小上限（未决问题，超限行为需先定：截断 / 拒绝 / 诊断）。
2. 「能力开关」页只编辑全局那份；工作区级覆盖已经能读
   （`workspacePluginState[ws].capabilities`），界面上还没给"仅本项目"的选择。
3. 插件卡不列"这个插件注册了哪些钩子点位"——受限原因能说明"哪一步会被忽略"，
   但"它会动手做哪些事"目前只有 `getLoadedPlugins()` 能查到。

---

## 四、工程与交付注意

- **打包产物**：已随 M3 重建（`dist/a-da.exe` 与当前源码一致），并且
  `bun scripts/binary-check.ts` 现在验两关——UI 角色画出首帧且日志确认走 `ws`（自 spawn 主机）、
  主机角色 `--host` 报端口 + 握手 + 快照。**注意**：打包产物是 GUI 子系统（无控制台），
  自动化通道到不了管道，所以二进制检查只能用应用自己的启动日志当证据（详见
  `docs/agent-conventions.md` §17）。
- **拆分后的两项已知缺口**（M3 完成记录里如实标了，不是漏做）：
  1. **主机被杀时界面没有提示**：ws 客户端会退避重连、连上先要快照对齐，但"主机没了"
     目前只进控制台/日志，用户看不到一句人话（协议 §1.5 要求可理解 + 可重连）；
  2. **冷启动退化未测量**：打包形态多了"spawn 主机 + 握手 + 首帧快照"三步，只验过"能画出首帧"，
     没有与 M0–M2 的进程内形态对比过数字。
- **分支未合**：M0–M3 全部工作在一个特性分支 `feat/ui-host-split` 上（分阶段提交，可逐里程碑合并），
  未合回 `main`。
- **测试有偶发**：全量测试连跑 8 次出现过 1 次单条失败（每次失败的用例名不同），属既有的
  跨文件干扰类——`store` 是进程级单例、若干测试会临时改 `A_DA_HOME`，而 `bun` 把各测试文件
  放在同一进程里并发跑。不是新引入的，但新加会 mount 窗口或改环境变量的用例要格外小心
  （约束见 `AGENTS.md` §13 末尾）。
- **与设计文档的刻意偏差**：插件系统那三处见 `AGENTS.md` §12 / §13；拆分（M0–M3）期间的偏差
  逐条写在 `docs/ui-host-split-dev-plan.md` 的里程碑完成记录里（**合并前先读那两处**）。
