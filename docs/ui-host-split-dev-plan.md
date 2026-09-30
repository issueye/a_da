# UI ↔ Agent 主机拆分开发计划（含 MVP 里程碑）

> 依据：**`docs/jsonrpc-protocol.md`（协议草案 v0.1）**——本文把它切成可交付的里程碑，不再重复协议细节。
> 目标项目：a_da
> 状态：**待评审，未实现**
> 交付约束：**编译产物仍然只有一个 `dist/a-da.exe`**（协议 §0.2 第 4 条 / §1.8）；拆分是内部实现。
> 基线：见 §2，全部为本次实测（不是印象）。

---

## 1. 计划总览

```
M0 契约与测试替身 ──→ M1 复制视图与事件流 ──→ M2 文件/配置类 RPC 去耦 ──┐
   纯重构，行为不变      store 广播改成事件流       管理页不再碰文件系统   │
   （接口先立边界）      （客户端自持快照）          （UI 侧 import 清零） │
                                                                        ▼
                                        ★ MVP 达成：UI 只认协议，进程内跑，行为等价
                                                                        │
                    M3 真 WebSocket + 单文件双角色 ──→ M4 远端与多客户端（可选）
                        一个 exe 两个角色，用户无感         wss / 抢答 / 只读客户端
```

**MVP 的定义（重要）**：**M0–M2 完成即 MVP**。判据是"进程边界已经存在、UI 只认协议、行为与今天逐项等价"，
但传输仍是进程内——**用户可见行为零变化**。理由：

- 拆分的第一价值是**去单例耦合与可测性**（协议 §9 的 47 个进协议成员收敛到一个接口），这部分不依赖网络；
- 网络一旦提前进来，就会把"接口是否划对"和"进程/端口/生命周期是否搞对"两类问题混在一起查；
- M3 只换 `Transport` 的实现，**UI 与主机的业务代码在 M3 一行都不用改**——这正是 M0–M2 要买到的性质；
- M0 让 `client.state` 临时暴露 store 活对象，换来的是 **50 处测试 fixture 在 M0 一处都不用动**；
  真正的测试改动集中在 M1，且每处只加一次"提交"调用（§3 M1-7）。

**v1 = M3**（真 WS + 单文件双角色，用户可见行为才开始有变化：多一个后台主机进程，但交付物仍是同一个 exe）。
**v2 = M4**（远端/多客户端/Web 预留接口，协议 §12）。

**每个里程碑独立可交付、可回滚**：M0 纯机械重构（类型检查兜底）；M1/M2 行为等价（现有测试是回归线）；
M3 有明确开关（`A_DA_TRANSPORT=inprocess|ws` 可强制；`bun run dev` 与测试默认 `inprocess`，
打包 exe 无参数启动默认 `ws` + 自 spawn）。

---

## 2. 基线（本次实测）

| 门 | 命令 | 实测 |
|---|---|---|
| 类型检查（门一） | `bun run typecheck` | **exit 0** |
| 非 UI 测试（门二，真实回归线） | `bun test src/agent` | **569 pass / 0 fail**（64 个文件，~19s） |
| 全量测试 | `bun test` | **718 pass / 0 fail**（86 个文件，~47s） |

**耦合面实测**（协议 §0 的数字来自这里，命令见括注）：

| 指标 | 实测 | 口径 |
|---|---|---|
| UI 对 `store` 的引用 | **233 处 / 70 个不同成员** | `grep -o "store\.[A-Za-z_]\w*" src/ui/*.tsx`（排除 `*.test.tsx`） |
| UI 文件里直接 import agent 模块的 | **17 / 20 个文件**（13127 行） | `from '../agent/...'` |
| UI 直接调用的管理器函数 | **17 个** | `defaultExtensionLoader`/`Skill`/`Prompt`/`Subagent` + `config.*` |
| 单文件最重的是 | `Composer.tsx` 66 处 store 引用 / 6 个 agent import；`PluginsDialog.tsx` **11 个 agent import** / 33 处 | 同上 |
| agent 侧反向抓 store 单例 | **7 处**（`ask-user.ts` + `subagents.ts` ×6） | `await import('../../store')` |
| `store.ts` 规模 | **4409 行**（非空 4065）/ **73 个公开成员**（58 方法 + 15 getter/setter）、51 个 private；`notify()` 88、`notifySoon()` 14、`push()` 46 | 逐行清点（正则锚定 2 空格缩进，排除 `private`） |
| 挂真窗口的 UI 测试 | **14 个文件** | `createTestRoot` |
| UI 测试直接改 `store` 内部状态（fixture） | **50 处 / 7 个文件**（`Transcript` 16、`ChangesPanel` 12、`Composer` 12、`TodoFloatingPanel` 4、`TabStrip` 3、`Sidebar` 2、`shortcuts` 1） | 正则匹配 `thread.items =` / `store.threads =` / `store.activeId =` 这类直接赋值 |

**本计划的验收门（每个里程碑都跑，判定同 §5）**：
1. `bun run typecheck` exit 0；
2. `bun test src/agent` **0 fail**；
3. 全量 `bun test` **0 fail，且不新增失败用例名**；
4. **M0/M1/M2 额外一条**：`src/ui/**`（非测试）对 `../agent/store` 与 17 个管理器的 import **必须清零**（守门测试）。

---

## 3. 里程碑

### M0 — 契约与测试替身（纯重构，行为不变）

**目标**：把"UI 直接抓单例"变成"UI 只认一个客户端接口"，**实现仍是今天那个 store**。
这一步不改任何语义，只立边界——它是后面三步的地基，也是唯一"改动面很大但风险很低"的一步。

**交付物**

| # | 任务 | 涉及文件 |
|---|---|---|
| M0-1 | 新建 `src/shared/protocol/`：方法名常量、params/result DTO、错误码（协议 §3–§6）。**只放类型与常量**，不 import 任何 agent 实现 | 新建 3–4 个文件 |
| M0-2 | 新建 `src/ui/client/`：`AgentClient` 接口（`request(method, params)` / `on(evt, cb)` / `state`）+ `createInProcessClient(store)` 映射表 | 新建 2 个文件 |
| M0-3 | UI 迁移（**只读**）：`client.state.*` 替换 `store.*` 的读侧。`state` 在 M0 允许直接暴露 store 的活对象（M1 才换成复制视图） | `Sidebar/TabStrip/Transcript/Composer/ContextUsagePopover/TodoFloatingPanel/QuestionCard/DebugPanel/CommandPalette/EmptyConversationView` 等 10+ 个 |
| M0-4 | UI 迁移（**命令**）：`client.request('thread.send', …)` 等，替换写侧调用 | `Composer/Sidebar/WorkspaceSelector/Transcript/CommandPalette` |
| M0-5 | 守门测试：`src/ui/**`（非测试）不得 import `../agent/store`。放 `src/ui/protocol-boundary.test.ts`——**纯读文件断言，不挂窗口**，随全量 `bun test` 跑 | 新建 1 个测试 |
| M0-6 | `store` 侧补"协议方法 → 现有成员"的映射注释（一行一条），供 M1/M2 对照 | `src/agent/store.ts` 注释 |

**验收标准**

- [ ] 三条门全过（§2）；**没有任何一个 UI 测试需要改断言**（行为等价的直接证据）
- [ ] `src/ui/**`（非测试）里 `from '../agent/store'` 出现 **0 次**；`store.` 出现 **0 次**
- [ ] `client.state` 与 `client.request` 覆盖协议 §9.1 的 A 组（29 个命令）+ B 组（16 个快照字段）
- [ ] 4 个最重的文件（`Composer` 66 处、`Sidebar` 39、`PluginsDialog` 33、`Transcript` 14）全部迁移完毕

**明确不做**：不引入网络、不改事件语义（还是 `store.notify` 驱动重渲染）、不动 `store.ts` 的内部结构、
不让 UI 少 import 除 `store` 之外的东西（管理器的去耦是 M2）。

**实测改动面**：233 处引用 + 17 个文件的 import；**风险最低**（漏改会被 typecheck 直接抓住）。

---

### M1 — 复制视图与事件流（行为等价）

**目标**：`store` 的"原地改对象 + 广播重渲染"变成"**快照 + 事件**"，客户端自持一份只读复制视图。
M1 结束时仍然进程内跑，但**横跨进程的那套数据流已经成型**。

**交付物**

| # | 任务 | 说明 |
|---|---|---|
| M1-1 | `ViewStore`：客户端状态容器（`threads/activeId/queue/log/workspaceInfo/config/pending`…），由事件驱动更新 | 新建 `src/ui/client/view-store.ts` |
| M1-2 | 主机侧事件发射：88 处 `notify()` 逐条归类为 `evt.*`（协议 §4）或"客户端本地状态" | 改 `src/agent/store.ts`（**逐条清单**，见下） |
| M1-3 | 快照：`session.snapshot` 从 store 组装（threads + workspace + config + pending） | `src/agent/host/snapshot.ts` |
| M1-4 | **先粗后细**：起步允许整 thread 级事件（`evt.thread.upserted` + `evt.thread.items`），只在**高频路径**做增量（`evt.message.delta`、`evt.card.updated`、`evt.log.appended`） | 同 M1-2 |
| M1-5 | 合帧：同 item 的 delta 按 16–33ms 合并（**InProcess 下也走合帧**，提前暴露体验问题） | `src/ui/client/`、`src/agent/host/` |
| M1-6 | `client.state` 由"活对象"换成"复制视图"（M0 的临时妥协在此结束） | `src/ui/client/` |
| M1-7 | **测试 fixture 通道**：现有 UI 测试有 **50 处**直接改 `store` 内部状态（`thread.items = …` / `store.threads = …`）来造数据，复制视图看不到"没提交"的改动。为此提供一次"提交"入口（改完调一次 → 触发一次全量 `evt.thread.items`），50 处**只加一行，断言不动** | `src/ui/__fixtures__/store-fixture.ts`（新）+ 7 个测试文件各加若干行 |
| M1-8 | 粗粒度兜底事件：`notify()` 在拿不到精确事件时发 `evt.thread.items {reset:true}`——**先保证正确，再按测量省带宽** | 同 M1-2 |

**88 处 `notify()` 的归类规则**（M1-2 的清单口径）：

| 归类 | 去处 | 例子 |
|---|---|---|
| 高频数据流 | `evt.message.delta` / `evt.card.updated` | 流式文本、思考、工具输出增量 |
| 结构变化 | `evt.item.upserted` / `evt.thread.items` | 新消息、工具卡、通知行 |
| 会话级 | `evt.thread.upserted` / `evt.thread.removed` / `evt.thread.running` | 新建/删除/运行状态 |
| 运行环境 | `evt.workspace.scanned` / `evt.queue.updated` / `evt.stats.updated` | 扫描、队列、统计 |
| 纯 UI | **不发事件**，留客户端 | 弹窗开合、草稿、滚动 |

**验收标准**

- [ ] 三条门全过；**流式文本、工具卡、队列、审批卡行为与今天逐项等价**
- [ ] 合帧生效：单轮流式回复产生的 `evt.message.delta` 条数 **≪ token 数**（用测试断言上界）
- [ ] 88 处 `notify()` 每条都在清单里有归属（清单进代码注释，M1 的 review 材料）
- [ ] 断开客户端后重连：`session.snapshot` + `seq` 能恢复到一致视图（InProcess 下用两个 client 实例模拟）
- [ ] **50 处测试 fixture 只允许"追加一次提交调用"，渲染断言一律不改**（这条比 M0 松：M1 真的改了数据流，
      测试造数据的方式必须跟着走一步；见 M1-7。M0 阶段这 50 处一处都不用动——那正是 M0 让 `state` 暴露活对象的收益）

**明确不做**：不引入真网络（还是 InProcess）；不做 `seq` 环形缓冲（M3 才需要）；不改任何工具的语义。

**实测改动面**：`store.ts` 的 88 + 14 个广播点、UI 侧 12 个订阅组件、7 个测试文件的 50 处 fixture；
**风险中等**（漏一处 = 界面停在"执行中"）。

---

### M2 — 文件与配置类 RPC 去耦（MVP 收尾）

**目标**：把 UI 剩下那些**直接读写磁盘/配置**的调用搬到主机（协议 §3.6–§3.11），
`src/ui` 对 agent 实现的 import 清零。**M2 结束 = MVP 达成。**

**交付物**

| # | 任务 | 对应协议 | 主要文件 | UI 侧调用点 |
|---|---|---|---|---|
| M2-1 | 插件：`plugin.list/reload/setEnabled/delete/createTemplate/config.*/secret.*/capabilities.*/conflicts/builtinCatalog` | §3.8 | `tools/loader.ts`、`config.ts` | `PluginsDialog.tsx`（11 个 import、33 处 store 引用） |
| M2-2 | 配置：`config.get/set/checkProvider/presets` | §3.7 | `config.ts` | `SettingsDialog.tsx` |
| M2-3 | 技能/提示词/子智能体档案：`skill.*`、`prompt.*`、`subagentProfile.*` | §3.9 | `skills/`、`prompts/`、`subagents/manager.ts` | `SkillsPanel.tsx`、`PluginsDialog.tsx`、`Composer.tsx` |
| M2-4 | 改动审阅：`change.list/count/diff/revert*` | §3.10 | `checkpoint/`、`store.ts` | `ChangesPanel.tsx`、`TitleBar.tsx` |
| M2-5 | 工作区：`workspace.info/rescan/entries/add/remove/openPublic/projects` | §3.6 | `store.ts`、`session/manager.ts` | `Sidebar.tsx`、`WorkspaceSelector.tsx` |
| M2-6 | 删掉渲染期同步读盘：`Composer.tsx:153/233` 的 `getCompositeSystemPromptSync` → 用 `Thread.lastSystemPromptChars` 或 `prompt.composite` | §9.2 | `Composer.tsx` | 1 处（但它在渲染路径上） |
| M2-7 | 守门测试扩一条：17 个管理器函数在 `src/ui/**` 里出现 0 次 | — | `src/ui/protocol-boundary.test.ts` | — |

**验收标准**

- [ ] 三条门全过；4 个管理页（插件/设置/技能/改动）行为与今天等价（现有 UI 测试是回归线）
- [ ] `src/ui/**`（非测试）对 `../agent/**` 的 import **只剩 `types` 与纯函数**（协议 §9.2 最后一行的白名单）
- [ ] `readPluginSecret`/`readSavedConfig` 这类**读密钥/配置**的调用在 UI 侧归零；密钥只回"是否已设置"
- [ ] UI 侧不再有任何 `writeFileSync`/`scan*` 之类的文件系统动作（用守门测试断言 import 面）

**MVP 达成判据（Definition of Done）**

1. `src/ui/**` 只 import：`shared/protocol` 的类型、`ui/client` 的实现、纯展示函数（`patchStats`/`computeContextBreakdown` 等白名单）；
2. 协议 §9.1 的 A 组 29 个命令、B 组 16 个快照字段在 **InProcess** 下全部走通；
3. 三条门全过，14 个真窗口 UI 测试**一条断言都没改**；
4. `store.ts` 可以整体移到 `--host` 那一侧而 UI 不需要任何改动（结构上可验证：UI 侧 import 面为零）。

**明确不做**：真网络、多客户端、打包变更（都留给 M3/M4）。

---

### M3 — 真 WebSocket + 单文件双角色（v1）

**目标**：把 Transport 换成真 WebSocket，并让**同一个 exe** 内部起两个角色（协议 §1.8）。
从这一步起"拆分"才是运行期事实，但**交付物仍是一个二进制**。

**交付物**

| # | 任务 | 说明 |
|---|---|---|
| M3-1 | `app.tsx` 按 argv **动态分流**：`--host` → `import('./src/agent/host/main')`；否则 `import('./src/ui/main')`（`init` + `render` 都搬进 UI 分支，host 绝不碰渲染层） | 协议 §1.8 落地要验第 4 条 |
| M3-2 | UI 自 spawn：一次性令牌 + `--port 0` + 读就绪行 + 连回环 WS；主机随 UI 退出（父进程消失检测，Windows 用 Job Object 兜底） | 协议 §1.8 |
| M3-3 | `WebSocketTransport`（服务端 + 客户端两半）：握手、`session.initialize/snapshot/resync/ping`、`seq` 环形缓冲 + 补发 | 协议 §1.1–§1.5 |
| M3-4 | 断线重连：seq 缺口 → `session.resync` → 补不了就 `-32010 NeedResync` → 重新快照 | 协议 §1.3 |
| M3-5 | 打包与验收：`scripts/build.ts` 不变；`scripts/binary-check.ts` 加一条"自 spawn 主机 + `session.initialize` + 画出首帧" | 协议 §1.8 |
| M3-6 | 传输开关：`A_DA_TRANSPORT=inprocess\|ws` 可强制；默认规则——`bun run dev` 与全部测试走 `inprocess`，**打包后的 exe（无参数启动）走 `ws` + 自 spawn** | — |

**验收标准**

- [ ] `bun run build` **仍只产出 `dist/a-da.exe`**；双击仍能出窗口（用户可见形态不变）
- [ ] `scripts/binary-check.ts` 的新用例通过（证明"单文件 + 内部拆分"）
- [ ] 杀掉主机进程 → UI 显示可理解的错误并可重连；杀掉 UI → 主机**不残留**（任务管理器核对）
- [ ] 全部测试仍走 `inprocess`，因此**测试速度与真窗口约束不变**
- [ ] 冷启动到首帧的时间没有明显退化（与 M2 对比，记录数字）

**明确不做**：不做远端 wss、不做多客户端共享、不做 `--detach` 常驻。

**风险最高的一步**（进程生命周期），因此 M3 开工前先做 spike：协议 §1.8 的"落地前要实测的四件事"。

---

### M4 — 远端与多客户端（v2，可选，协议 §12 的前置）

**目标**：让"另一台机器/浏览器里的客户端"能连同一个主机。**这一步不做也不影响 MVP/v1 的价值。**

| # | 任务 | 协议 |
|---|---|---|
| M4-1 | `wss://` + 长期令牌/配对码 + `session.revoke` | §1.6 |
| M4-2 | `--detach` 常驻主机；多客户端接入；审批广播抢答 + `evt.approval.settled` 撤卡 | §1.7 |
| M4-3 | 只读客户端角色 `client.role: 'readonly'`（写命令一律拒） | §12.3 |
| M4-4 | 能力位细化（`native.dialog/notify/reveal/windowControls`、`clipboard.image`）+ `workspace.browse`（服务端列目录） | §12.2/§12.3 |
| M4-5 | Web 客户端最小验证页（不追求产品化，只为证明协议够用） | §12 |

**验收标准**

- [ ] 第二台机器上的客户端能完成一次完整对话（含审批应答）
- [ ] 两个客户端同时在线：审批先答者生效、另一方撤卡；写命令授权正确
- [ ] 只读客户端的所有写命令被拒（错误码与 `data` 明确）
- [ ] 目录浏览 `/` 与图片附件在 Web 客户端可用（走 `dataUrl`）

---

## 4. 必须同步修改的文件清单

| 文件 | 改动 | 里程碑 |
|---|---|---|
| `src/shared/protocol/*`（新） | 协议类型、方法名常量、错误码；只放类型 | M0 |
| `src/ui/client/*`（新） | `AgentClient` 接口、`InProcessTransport`、`WebSocketTransport`、`ViewStore` | M0/M1/M3 |
| `src/agent/host/*`（新） | 主机入口、快照组装、事件发射、WS 服务端 | M1/M3 |
| `app.tsx` | argv 动态分流（UI / `--host`），`init`+`render` 搬进 UI 分支 | M3 |
| `src/ui/*.tsx`（17 个文件） | `store.*` → `client.*`；管理器调用 → `client.request` | M0/M2 |
| `src/agent/store.ts` | 88 处 `notify()` 归类为事件；补协议方法映射注释；后半搬进 host | M1/M2/M3 |
| `src/agent/tools/loader.ts` | 插件管理能力暴露为 `plugin.*` | M2 |
| `src/agent/config.ts` | `config.*` / `plugin.capabilities.*` / `plugin.secret.*` | M2 |
| `src/agent/{skills,prompts,subagents}` | 管理器方法 → `skill.*` / `prompt.*` / `subagentProfile.*` | M2 |
| `src/agent/checkpoint/*` | → `change.*` | M2 |
| `src/agent/tools/builtins/subagent.ts`、`builtin-plugins/ask-user.ts` | 反向抓 store 的 7 处改成"向已连接客户端发 `req.*`" | M1/M3 |
| `scripts/binary-check.ts` | 加"自 spawn 主机 + initialize + 首帧"用例 | M3 |
| `scripts/build.ts` | **不改**（仍然一个 `outfile`；两个角色在同一 bundle） | — |
| `docs/jsonrpc-protocol.md` | 实现期发现的偏差回写 | 全程 |
| `AGENTS.md` / `docs/agent-conventions.md` | 新增约定：UI 不得 import agent 实现；协议改动要同步三处 | M0 起 |

---

## 5. 逐里程碑的验收门（统一执行）

```bash
bun run typecheck          # 门一：exit 0
bun test src/agent         # 门二：0 fail（真实回归线）
bun test                   # 参照：0 fail，且不新增失败用例名
bun test src/ui/protocol-boundary.test.ts   # M0 起：UI 不 import agent 实现
```

- **禁止**只跑 `typecheck` 就宣布完成（`AGENTS.md` 明确两个门独立）。
- M1 与 M3 结束后额外跑 `bun run build`：M1 确认打包未受影响，M3 确认仍只有一个产物。
- 每个里程碑单独 commit（M0 尤其要单独，便于回滚）。

---

## 6. 风险与对策（按严重度）

| 风险 | 里程碑 | 后果 | 对策 |
|---|---|---|---|
| **半拆状态长期存在**（有的页面走 client、有的还抓 store） | M0/M2 | "看起来拆了，其实还连着"——最难查的一类 | 守门测试从 M0 第一天就上（`src/ui/**` 不许 import `agent/store`），CI 拦住新增 |
| **88 处 `notify()` 漏归类** | M1 | 界面停在"执行中"、工具卡不刷新，且不报错 | 逐条清单进代码注释 + 端到端断言（"跑完一轮后卡片状态必须是 done"） |
| **流式体验回归**（同进程原地改 → 跨进程 delta） | M1 | 打字机效果抖动/延迟 | InProcess 阶段也走合帧，提前暴露；断言 delta 条数上界 |
| **进程生命周期**（自 spawn / 端口 / 孤儿进程） | M3 | 残留进程、启动失败、杀毒软件拦截 | M3 前先做 spike，验协议 §1.8 的四件事；就绪失败退路是"预选端口 + 重试连接" |
| **host 分支误碰渲染层** | M3 | `--host` 进程多一个空窗口或原生初始化失败 | `app.tsx` 改动态 import 分流（协议 §1.8 第 4 条），binary-check 守住 |
| **M1 触到测试 fixture**（50 处直接改 `store` 内部状态） | M1 | 换复制视图后这些用例集体红，容易被误判成"产品坏了" | 先上 M1-7 的提交入口 + M1-8 的粗粒度兜底事件，再动事件模型；**一次只改一个测试文件**，跑完再下一个 |
| **测试基础设施被拖慢** | M1/M3 | `bun test` 从 47s 涨到不可接受；真窗口约束被破坏 | 测试一律 `inprocess`；UI 测试继续用现有的 `createTestRoot` 单窗口模式 |
| **管理页迁移遗漏**（插件/技能/提示词/子智能体/配置四处） | M2 | 某个开关点了没反应 | 17 个管理器函数逐个对账（§3 M2 表）+ 守门测试断言 import 面为零 |
| **`seq`/重连语义做错** | M3 | 断线后状态错乱（比重连失败更糟） | 缺口 → `NeedResync` 强制重新快照；不做"尽力补"的模糊路径 |
| **协议与实现漂移** | 全程 | 文档承诺与行为不一致（本项目已有前科） | 实现期的偏差回写协议文档；方法表与 `shared/protocol` 的类型同源 |

---

## 7. 开工前要定的四件事

1. **`src/shared/protocol/` 的落点与依赖方向**：它被 UI 与 agent 同时 import，因此**只能是类型与常量**，
   不能 import 任何一侧实现（否则又成环）。是否认可这个约束？
2. **M0 的 `client.state` 允许临时暴露 store 活对象**：这是"小步走"的关键妥协（M1 才换复制视图）。
   好处是 M0 纯机械、类型检查兜底，而且**50 处测试 fixture 在 M0 完全不用改**；
   代价是 M0 结束时**语义上还没真拆**（数据流仍是 store 带头）。是否接受？
3. **守门测试的形式**：仓库没有 eslint，建议用一条读文件的 `bun test`（`src/ui/protocol-boundary.test.ts`），
   断言 `src/ui/**`（非测试）里不出现 `from '../agent/…'` 的**实现**导入。是否认可？
4. **开发期默认传输**：建议 `bun run dev` 与全部测试都用 `inprocess`，只有验收 exe 时才起 `--host`。
   （否则每次改代码要重启两个进程，开发体验下降。）

---

## 8. 立即开始：M0 执行清单

按顺序做，每步跑一次 `typecheck`：

1. **M0-1**：建 `src/shared/protocol/`（`methods.ts` 方法名常量、`dto.ts` 类型、`errors.ts` 错误码），
   从协议 §3–§6 **抄定义**，先只写类型（`import type` 指向 `src/agent/types.ts` 等现有类型）。
2. **M0-2**：建 `src/ui/client/`：`AgentClient` 接口 + `createInProcessClient(store)`；`request` 用一张
   `method → (params) => store.xxx(...)` 的映射表实现，映射表按协议 §9.1 的 A 组逐条写。
3. **M0-5（提前做）**：先写守门测试并**允许它暂时红**（列出待迁移文件），当作迁移进度表。
4. **M0-3**：从最重的 `Composer.tsx`（66 处）开始迁移读侧，再 `Sidebar` → `Transcript` → `TabStrip` → 其余。
5. **M0-4**：迁移写侧命令（`send`/`newThread`/`selectThread`/`stop`/`decide`/`answerQuestion`/`revert*`…）。
6. **M0-6**：在 `store.ts` 里给每个被协议用到的方法补一行"对应协议方法"注释。
7. 跑三条门 + 守门测试全绿；`git commit`（单独一笔，纯重构便于回滚）。

> MVP 的终点在 M2 末尾；M0 只是第一步。**M0 完成时仓库仍是可发布状态**（行为零变化）。
