# 未完成功能清单

> **维护约定**：每条写清「现状证据（`file:line` 或 grep 判据）／影响／最小实现路径／状态」。
> 状态取值：`未排期`（没人做）、`待拍板`（缺一个产品决定）、`明确不做`（设计上划出去了）。
>
> **最后核对**：2026-10-09。核对方式＝全仓 grep + 跑六条门禁，**不是凭印象**。
> 改动功能后请顺手更新本文件。
>
> **注意**：本文件只写"**功能缺口**"。接线类问题（"看起来装上了其实没接线"）
> 的唯一口径在 [`docs/agent-base-wiring-plan.md`](agent-base-wiring-plan.md)——
> 那里有逐条证据与处置状态，不要在这里重复。

## 门禁基线（核对时）

| 命令 | 结果 |
|---|---|
| `cargo test --workspace -- --test-threads=1` | **302 passed / 0 failed** |
| `cargo xtask verify` | ✔ 全仓 Verify 通过 |
| `cargo xtask verify-wiring` | ✔ 无违约（协议方法 72 / match 臂 72；端口 11 个全有生产实现） |
| `cargo xtask verify-spec` | ✔ 无违约 |
| `cargo xtask compat` | ✔ 无违约 |
| `cargo xtask verify-archive` | ✔ 全绿（8 条断言） |
| `bun run typecheck` | ✔ exit 0 |

> `cargo xtask verify-docs` 的**红例基线已清零**（W6-T7）：README / 本文件 / `feature-catalog.md`
> 不再把归档布局当现行路径描述。

---

## 一、产品功能缺口（按价值排序）

### 1. MCP client　`待拍板`

- **现状**：全仓 `grep mcp` **零命中**（`crates/` 与 `tauri-ui/` 都没有），设计文档也从未提及。
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
- **最小实现路径**：给 `auto_approve` 接上消费者（按「工具名 + 工作区」匹配），
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
  工具 schema 字符数之和），但前端 `tauri-ui/src/utils/context-breakdown.ts`
  **从未被调用**，`systemChars` 输入也没人提供——界面仍在客户端估算。
- **最小实现路径**：在上下文用量 Popover 打开时调 `stats.promptChars`，
  把它作为 `buildContextBreakdown` 的输入。

### 8. 图片输入从未送进模型　`未排期（当前产品都声明 images=false）`

- **现状**：`TurnRequest::with_images` **全仓零消费者**；`thread.start` 把 images 存进
  UI store（`add_user_message_with_images`）后就**没有下文**——模型永远收不到图片。
- **已就位的部分**：`capabilities.images` 的**门禁**是真的（W5-T4：
  声明 `false` 的产品会在 `run_turn_with_images` 里被拒）。
- **最小实现路径**：`dispatch.rs` 的 `run_agent_turn` 调用点把 images 透传进 `TurnRequest`；
  并在模型侧按 `ModelCapabilities.images` 决定是否携带。

### 9. 钩子点位 = 0　`未排期`

- **现状**：插件契约承诺"机制 + 3 个点位"，实际**一个点位都没接**——
  `grep HookPoint|beforeTurn|afterAgentEnd` 在 `crates/` 零命中。
- **口径**：要么接上点位（至少 1 个真被调用的），要么把契约里的点位声明删掉（R3）。
  **不能保持"契约里有、实现里没有"**——那正是 AGENTS.md §15 警告的"静默失效"。

### 10. i18n　`未排期`

- **现状**：文案硬编码中文（部分中英混排）。`grep i18n|useTranslation` 零命中。
- **注**：产品声明里有 `identity.locale`（W5-T4 已接进系统提示词），
  但**界面文案**仍与 locale 无关。

---

## 二、明确不做的（设计上划出去了，别当缺口）

- **第三优先钩子点位**：模型选择、上下文超限前的主动裁剪、并发批次控制、错误 / 重试、
  用户输入改写（见 `docs/plugin-system-design.md` §6.6.2 与 §6.8）。
- **`casual` 惰性工具档位**：契约里保留了该取值，运行层**明确拒绝并记 trace**，
  不会假装降级成功。开发计划里标注为"不在 MVP"。
- **决策插件的引擎驱动"自动模式"**：每轮多一次引擎调用（成本与延迟翻倍），
  没有可用引擎时只能靠启发式——按本项目"拿不到真实判断就不假装有判断"的原则不做。
  已实现的是确定性工具路由。
- **`beforeTurn.replaceText` / `afterAgentEnd.appendNote`**：本项目没有"改写已渲染回复"与
  "向已结束会话追加旁注"的交付通道，声明它们只会变成静默失效，所以契约里没有这两个字段
  （原因写在 `AGENTS.md` §12）。
- **`cargo xtask gen`（协议四产物生成）**：W5-T1 判定不划算，**正式降级**为
  "手写 + 三对副本双向校验"（决策与依据见 `docs/agent-base-design.md` §6.2）。
- **`enum` 化协议方法常量 + 编译器穷尽性**：同上，降级为散装 `&str` + 机械校验。

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

- **构建**：本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；
  加 `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。
- **Rust 测试必须串行**：`cargo test --workspace -- --test-threads=1`。
  根因是共享全局态（`AppHome` 单例、若干测试读 `~/.a-da`）；
  测试默认 home 已指向临时目录，但仍有个别用例共享进程级状态。
- **两个独立的门**：`bun run typecheck` 与 `cargo test` **都要过**，别只跑一个。
- **TS 测试已整体冻结**：`bunfig.toml` 已把旧实现排除在测试发现之外，
  跑 TS 测试只会得到 "No tests found"。**门就是 `cargo test` 与 `bun run typecheck`。**
- **打包产物检查**：产物是 GUI 子系统（无控制台），自动化通道到不了管道，
  所以二进制检查只能用应用自己的启动日志当证据（详见 `docs/agent-conventions.md` §17）。
- **冷启动退化未测量**：打包形态多了"spawn 主机 + 握手 + 首帧快照"三步，
  只验过"能画出首帧"，没有与进程内形态对比过数字。
- **前端重连**：W6-T6 已改为指数退避（500ms 起 / 30s 封顶 / ±20% 抖动）+ 代次保护 +
  断线立刻失败在途请求；策略在 `tauri-ui/src/client/reconnect-policy.ts`，
  由 `cargo xtask verify-wiring` 的结构性断言守住。
- **CLI `run`**：W6-T3 已从"只建会话就退出"改为真执行（装配 → 跑一轮 → 落盘），
  退出码 0/1/2 分明；`--dry-run` 可离线验证装配。

---

## 相关文档

| 文档 | 用途 |
|---|---|
| [`docs/agent-base-wiring-plan.md`](agent-base-wiring-plan.md) | **接线类问题的唯一口径**：缺口台账、任务表、执行记录、断言变更 |
| [`docs/agent-base-design.md`](agent-base-design.md) | 设计口径（分层、端口、协议单源、装配） |
| [`AGENTS.md`](../AGENTS.md) | 接手须知与 § 索引 |
| [`docs/protocol/README.md`](protocol/README.md) | 线协议总览 |
| [`docs/feature-catalog.md`](feature-catalog.md) | 已实现功能清单 |
