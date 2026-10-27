# 插件化改造与安全下线：设计与开发计划

> 状态：**执行中**（2026-10-27 起）
> 范围：全仓（`crates/`、`plugins/`、`products/`）
> 相关：[kernel-design.md](kernel-design.md)、[conventions.md](conventions.md)、
> [../plugins/README.md](../plugins/README.md)、[../quality/unfinished-features.md](../quality/unfinished-features.md)

---

## 0. 一页结论

**目标**：让 `agent-base` 只保留内核态（领域类型、端口契约、唯一多轮循环、安全策略的
**执行语义**），其余能力全部以**插件贡献**的方式接入；产品差异只写在声明与插件清单里。

**两条并行推进的线**：

1. **插件化**：先修掉"假插件"，再把叶子能力、中台能力、协议面逐层迁到插件。
2. **安全下线**：本阶段审批、路径沙箱、插件能力门、危险命令扫描**一律不阻塞功能**，
   用一个总开关统一控制；代码保留、可一键恢复，物理删除列为独立的 P3 可选项。

**为什么安全下线不是"把安全代码删了"**：审批闸门是 `agent-base` 的端口契约，路径沙箱
被工具、子智能体、命令执行、插件运行时共同依赖，事件与回执里也带着审批状态。物理删除会
同时破坏引擎契约、协议、UI 与大量测试，属于一次大重构；而"行为下线"能在**不改契约**的
前提下立刻解除功能阻塞，并且随时能恢复。两者的取舍见 §4。

---

## 1. 现状与问题

### 1.1 分层现状

| 层 | crate | LOC（src） | 职责 |
|---|---|---|---|
| 内核 | `agent-base` | 8,184 | 领域类型、13 个端口、`run_turn`、钩子分级 |
| 端口实现 | `agent-adapter` | 4,707 | 模型 SSE、JSONL 会话、路径沙箱、提示词、时钟 |
| 工具集 | `agent-toolkit` | 7,521 | 20 个内置工具描述符、6 个工具包 |
| 节点能力 | `agent-node` | 18,533 | 会话、审批、检查点、子智能体、插件、技能、钩子运行时 |
| 桥接 | `agent-rpc` | 13,287 | 86 个 RPC 方法、WS 宿主、UI 投影 |
| 组合根 | `agent-runtime` + `agent-host` | 4,383 | 按 spec 装配 |
| 平台 | `agent-gateway` + `agent-web` | 30,061 | 网关 + SSR 界面 |
| 插件运行时 | `ts-engine` | 9,709 | Boa + oxc 沙箱 |

依赖方向由 `verify-wiring` 守着：`agent-rpc → agent-node`、网关/界面只依赖 `agent-proto`。

### 1.2 插件系统现状

第三方 TS 插件（`~/.a-da/...` 或 `<ws>/.ada/...` 的包目录）在**装配期**结算贡献：

- 工具（`ada.registerTool`）、提示词片段（`ada.registerSystemPrompt`）、
  技能（磁盘 `skills/<name>/SKILL.md`）、钩子（`ada.on`）、界面动作（`ada.registerAction`）、
  HTTP 路由（`ada.route`）、前端插槽/页面/仪表盘（`plugin.json`）。

它**不能**提供端口实现、工具包、中台服务、RPC 方法。这是本计划要补的部分。

### 1.3 三个关键问题

1. **9 个"内置插件"是卡片，不是插件**
   `crates/agent-node/src/plugins/builtins.rs` 的 `BUILTIN_PLUGINS` 只是元数据；
   工具实际编译在 `agent-toolkit`。`collect_plugin_tools` 明确跳过 `scope == "builtin"`
   （`scripts/assembly.rs`），装配走 `spec.toolkits`（`agent-host/src/lib.rs`）。
   结果是：**关掉 `builtin:git-tools` 不会移除 `git` 工具**。
2. **能力选择是"工具包"，不是"插件"**
   产品只能声明 `toolkits`（`toolkits.rs` 的 `TOOLKIT_NAMES`），新增/裁剪能力都要改 Rust。
3. **内核里残留协议形状**
   `engine/prompt_format.rs` 直接产出 `ChatCompletionMessage`，
   `CompletionRequest.messages` 也是 OpenAI 形状；模型供应商插件会继承这个约束。

---

## 2. 目标与非目标

### 2.1 目标

- **G1 内核最小**：`agent-base` 不新增任何产品能力；所有扩展都通过端口或贡献点。
- **G2 清单驱动**：组合根只读产品声明 + 插件清单，不再有"再加一个 `with_xxx`"的硬编码装配。
- **G3 真启停**：插件/工具包关闭后，工具、提示词、技能、钩子、UI 同时下线，并给收据。
- **G4 两层插件**：第一方 Rust 插件（受信、编译期注册）承载中台能力；
  第三方 TS 插件（沙箱）承载可隔离的扩展。
- **G5 功能优先**：安全机制默认不阻塞；恢复只改一个开关。

### 2.2 非目标（本阶段）

- 不做动态 Rust `dylib` 插件（ABI 不稳定、失败模式差）。
- 不引入第二套引擎或第二套协议。
- 不重写 `agent-base` 的领域模型。
- 不删除第三方 TS 插件的沙箱（它仍然是隔离边界，只是能力门暂时放开）。

---

## 3. 目标架构

### 3.1 内核边界

`agent-base` 只保留：

- `domain/`：消息、工具描述符与回执、事件、错误与失败方向、诊断点位；
- `ports/`：13 个端口 trait（无默认实现）；
- `engine/`：`run_turn`、`RunPolicy`、钩子编排、消息格式化；
- `model/`：模型面类型（后续可把协议形状下沉到适配器，见 P2）。

**不进内核**：具体工具、工具包、审批策略、检查点、子智能体、后台任务、模型供应商、
会话存储实现、UI、协议分发、网关。

### 3.2 插件 SDK

新增 `agent-sdk`（名称暂定，也可放在 `agent-runtime` 内）：

```rust
pub trait CapabilityPlugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn contribute(&self, cx: &PluginContext, out: &mut ContributionSink)
        -> Result<(), PluginError>;
}

pub struct PluginManifest {
    pub id: &'static str,
    pub version: &'static str,
    pub provides: &'static [ContributionKind],
    pub requires: &'static [Requirement],   // 端口/能力/工具包依赖
    pub default_enabled: bool,
    pub tier: PluginTier,                   // FirstParty | ThirdParty
}
```

`ContributionSink` 提供与贡献类型一一对应的方法：

```rust
add_toolkit / add_tool / add_prompt_fragment / add_skill
add_hook / add_model_provider / add_session_store / add_scope
add_approval_policy / add_rollback_store / add_background_service
add_subagent_backend / add_prompt_pack / add_skill_source
add_protocol_namespace / add_ui_extension / add_event_subscriber
```

### 3.3 贡献类型

把现有 `AssemblyBuilder` 的 `ContributionKind`（Tool / PromptFragment / Skill / Hook）
扩展为：

| 类别 | 贡献 | 装载期校验 |
|---|---|---|
| 能力面 | `Tool`、`Toolkit`、`SubagentBackend`、`BackgroundService` | 描述符唯一、依赖工具包存在 |
| 上下文面 | `PromptFragment`、`PromptPack`、`Skill`、`SkillSource` | 排序稳定、重名去重 |
| 策略面 | `ApprovalPolicy`、`Scope`、`RollbackStore`、`CompactionStrategy` | 只能提供判定，不能接管执行 |
| 端口面 | `ModelProvider`、`SessionStore`、`EventSink` | 端口契约测试（conformance） |
| 交互面 | `Hook`、`ProtocolNamespace`、`UiExtension`、`EventSubscriber` | 分级校验、命名空间隔离 |

### 3.4 装配管线

```
spec.plugins ──► PluginRegistry::resolve ──► plugin.contribute(sink)
                                              │
                                              ▼
                                   AssemblyBuilder（收集→校验→排序）
                                              │
                                              ▼
                     AssemblyPlan { tools, catalog, prompt, skills, hooks, services }
                                              │
                                              ▼
                     AgentRuntime（引擎只拿端口，不认识插件）
```

`spec.toolkits` 保留为语法糖：等价于启用一组提供 `Toolkit` 的插件，避免破坏现有产品。

### 3.5 两层插件

| | 第一方 Rust 插件 | 第三方 TS 插件 |
|---|---|---|
| 形态 | 编译进产品的 crate，静态注册 | Boa 沙箱，`index.ts` |
| 受信 | 是 | 否 |
| 可提供 | 端口实现、工具包、中台服务、协议命名空间 | 工具、提示词、技能、钩子、动作、HTTP 路由、UI |
| 失败策略 | fail-fast | fail-safe（跳过并记录） |
| 启停 | 清单/配置 | 目录 + 配置 |
| 分发 | 随产品编译 | 插件市场/目录 |

**第一方插件也不许接管执行**：审批的"问用户"、路径沙箱的最终拒绝、取消贯穿、
事件 `seq` 仍然在 host/engine；插件只能提供策略与数据。

---

## 4. 安全与审批下线（本阶段执行）

### 4.1 决策

采用 **"行为下线 + 代码封存"**，而不是物理删除：

- 生产路径上不再出现审批弹窗、沙箱拒绝、插件能力门、危险命令二次确认；
- 端口、策略函数、协议方法、UI、测试全部保留；
- 一个常量控制是否执行：`agent_toolkit::security::SECURITY_ENFORCED`；
- 恢复 = 把常量改回 `true` + 跑全量测试，不改任何调用点。

物理删除列为 §4.6 的可选 P3，理由是它会同时破坏引擎契约、协议与 UI，
且当前分支有未提交改动，混在一起无法安全回滚。

### 4.2 开关

```rust
// crates/agent-toolkit/src/security.rs
pub const SECURITY_ENFORCED: bool = false;   // 本阶段：安全下线
pub const fn deferred() -> bool { !SECURITY_ENFORCED }
```

只在这几个**执行边界**读取它：

| 边界 | 位置 | 下线后行为 |
|---|---|---|
| 审批闸门 | `agent-node::approval::HostApprovalGate` | `needs_approval` 恒 false，`decide` 直接 allowed |
| 路径作用域 | `agent-adapter::scope::ApprovalAwareScope` | 视作完全访问（保留 `~/.a-da` 凭据黑名单） |
| 工具路径门 | `agent-toolkit::fs_tools::PathGate::Scoped` + `cmd_tools` / `background` | 作用域视作完全访问；命令 `cwd` 走直通解析 |
| 插件能力门 | `agent-node::scripts::to_engine_needs` / `subprocess::needs_json` | 授予 fs/net/sqlite/exec 全部能力 |
| 危险命令扫描 | 由审批闸门短路，策略函数不改 | 不再二次确认 |

**刻意不改的地方**（改了会让测试与契约失真，且不是"阻塞功能"的来源）：

- `should_ask_approval_*` 纯函数：保持原语义，单元测试继续有效；
- `check_workspace_sandbox` 纯函数：保持原语义，由调用方选择是否走直通档；
- `WorkspaceScope`：保持原语义，由 `ApprovalAwareScope` 决定是否启用。

### 4.3 影响矩阵

| 场景 | 下线前 | 下线后 |
|---|---|---|
| `write_file` 写工作区外 | 拒绝 | 允许 |
| `run_command` 的 `cwd` 在工作区外 | 拒绝 | 允许 |
| 第三方插件工具（`Access::External`） | 每次都问 | 不问，直接执行 |
| 插件 `@ada-needs` 没声明 `net` | 无 `fetch` | 有 `fetch` |
| 插件 `@ada-needs` 没声明 `exec` | 无 `child_process` | 有 `child_process` |
| 危险命令（`rm -rf` / `git push`） | Ask/Readonly 档二次确认 | 不问 |
| 只读档（Readonly） | 写工具一律问 | 不问 |
| `~/.a-da/config.json` 写入 | 任何档位拒绝 | **仍然拒绝**（见 §4.4） |

### 4.4 仍然保留的底线（不是审批/安全闸门）

1. **`~/.a-da` 凭据与配置不可写（经 `Scope` 的文件工具）**：`ApprovalAwareScope` 的
   `deny_prefixes` 不受开关影响。理由不是"要不要问用户"，而是**数据完整性**——一次误写会把
   API Key、供应商配置、网关发现文件改坏，事故成本远高于"少一次功能验证"。
   例外（插件目录、技能目录）继续按白名单放行。
   ⚠️ 本阶段 `run_command` / `run_background` 的 `cwd` 直通**不保证**这一点；
   但原实现也只校验 `cwd`、不解析命令内容，因此这不是本次新增的暴露面。
2. **工具回执结构化**：`status` / `started_at` / `finished_at` 非 `Option`，否则界面静默变空。
3. **事件顺序与 `seq`**：`ToolCallStarted` 先于 `ToolCallFinished`，每轮恰好一个 `TurnFinished`。
4. **取消贯穿**：`CancelToken` 仍传进模型流与工具执行。
5. **描述符单一真源**：工具读写性/审批/回滚仍来自 `registry.rs`；插件不能自述只读。
6. **失败方向**：端口/策略的 `FailDirection` 类型保留，等安全恢复时直接生效。

### 4.5 恢复步骤

1. `crates/agent-toolkit/src/security.rs`：`SECURITY_ENFORCED = true`；
2. `cargo test --workspace -- --test-threads=1`；
3. 如需逐项恢复，按 §4.2 的表逐个把调用方改回强制实现（开关支持分批：
   可在 `security.rs` 增加 `APPROVAL_ENFORCED` / `SCOPE_ENFORCED` 等细分常量）。

### 4.6 物理删除清单（P3，可选）

只有确认"不再需要这些能力"时才做，且必须单独分支、单独 PR：

| 模块 | 删除内容 | 连带改动 |
|---|---|---|
| 审批 | `agent-base/ports/approval.rs`、`agent-node/approval/`、`agent-adapter/approval/` | 引擎 `needs_approval_for`、事件 `ApprovalRequested`、协议 `approval.decide`、UI 审批托盘、`approval-guard` 插件 |
| 路径沙箱 | `agent-adapter/scope/`、`agent-toolkit/sandbox.rs` 的判定 | `Scope` 端口、`PathGate`、`fs`/`command`/`background`/插件工具 |
| 插件能力门 | `@ada-needs`、`ScriptNeeds`、`ts-engine/env/{fs,net,sqlite,child_process}` 的注入开关 | 插件文档、`create_plugin` 生成物、验证脚本 |
| 危险扫描 | `ApprovalGuardConfig.confirm_commands`、`is_destructive_command` | git/command 工具测试 |
| 协议/UI | `ApprovalMode`、`approval.decide`、审批托盘、`plugins/approval` | `methods.rs`、dispatch、client-ts、SSR、`verify-wiring` |

预计影响：13 个 crate 中的 9 个、约 40–60 个测试文件、4 份协议文档。**不建议与插件化改造同批做。**

---

## 5. 工作分解

### P0：修掉假插件（1–3 天）

**目标**：插件开关第一次真正生效。

- [ ] `BUILTIN_PLUGINS` 的 `tools` 字段作为"插件 id → 工具名"映射，装配时按
      `disabled_ids` 过滤；或删除 9 个假插件，直接展示 6 个工具包。
- [ ] 删除空壳 `ponytail`；`approval-guard` 要么接真实策略，要么改成说明性条目
      （不可启停）。
- [ ] 新增守门测试：关闭 `builtin:git-tools` 后，`ToolCatalog::descriptors()` 不含 `git`；
      `plugin.setEnabled` 的收据里工具 delta 非空。
- 验收：`cargo test -p agent-node -p agent-rpc` + 手动点一次插件开关。

### P1：叶子能力插件化（1–2 周）

**目标**：删掉一个插件目录，产品仍能编译，只是少一项能力。

- [ ] 新建 `plugins/` 第一方 crate：`git`、`project`、`forge`、`batch`。
- [ ] `TOOLKIT_NAMES` 改为从插件清单派生；`verify-spec` 继续守"声明 ↔ 实现"。
- [ ] 内置技能（`agent-node/skills/builtins.rs`）与人格提示词改为插件包。
- [ ] `plugins/` 目录与 `agent-toolkit` 的边界：工具实现依赖 `agent-base` + `agent-sdk`，
      不再依赖 host。
- 验收：`cargo xtask verify-wiring` + 删除任一插件后 `cargo build --workspace` 仍过。

### P2：中台能力端口化（3–4 周）

**目标**：审批策略、检查点、后台任务、委派、模型供应商都能被插件替换。

- [ ] 新增端口：`ApprovalPolicyProvider`、`BackgroundService`、`QuestionChannel`、
      `SubagentBackend`、`ModelProvider`、`CompactionStrategy`。
- [ ] 迁移：`checkpoint` → `RollbackStore` 插件；`background` → `BackgroundService` 插件；
      `approval policy` → `ApprovalPolicyProvider` 插件；`subagents` → `SubagentBackend` 插件。
- [ ] 模型端口去协议化：`CompletionRequest.messages` 改为协议中立的 `Message`，
      `format_messages_for_model` 下沉到 `agent-adapter`。
- [ ] 每个端口补 conformance 用例（沿用 `agent-conformance`）。
- 验收：两个不同插件实现同一端口，业务代码零改动。

### P3：协议面收窄与物理删除（可选，风险高）

- [ ] 核心 RPC 只保留 `session.*` / `thread.*` / `approval.*` / `question.*` / 事件；
- [ ] `market.*` / `provider.*` / `prompt.*` / `skill.*` / `subagentProfile.*` /
      `change.*` / `task.*` 迁到 `x.<plugin>.*`，由插件路由分发；
- [ ] 如确认不要安全能力，按 §4.6 执行物理删除。

---

## 6. 验收与门禁

### 6.1 现有门禁

```bash
cargo build --workspace
cargo test --workspace -- --test-threads=1
cargo xtask verify --fast
cargo xtask verify-wiring
cargo xtask verify-spec
bun run typecheck
```

### 6.2 新增门禁

| 门 | 断言 |
|---|---|
| `verify-plugin-manifest` | 清单里的每个 `provides` 都有实现；每个实现都有声明 |
| `verify-plugin-receipt` | `plugin.setEnabled` 收据里的增删与实际 catalog/prompt/skills/hooks 一致 |
| `verify-plugin-tiers` | 第三方插件不得提供端口实现/协议命名空间 |
| `verify-security-switch` | `SECURITY_ENFORCED` 只有一个定义处；生产边界不得绕过它直接判安全 |

---

## 7. 风险与回滚

| 风险 | 影响 | 处置 |
|---|---|---|
| 安全下线后误写用户目录 | 配置/密钥损坏 | 保留 `~/.a-da` 黑名单；测试用 `TempAppHome` |
| 插件化后工具表漂移 | 模型看到不存在的工具 | 描述符单一真源 + 装配收据门 |
| 第一方插件依赖 host | 分层回退 | `verify-wiring` 依赖方向检查扩展到 `plugins/` |
| 物理删除安全代码 | 引擎/协议/UI 大改 | 独立分支、独立 PR，不与 P0–P2 混做 |
| 未提交改动与重构混在一起 | 无法回滚 | 每阶段一个 commit；当前分支先单独提交安全下线 |

**回滚**：`SECURITY_ENFORCED = true` 即恢复安全行为；插件化按阶段独立 commit，
每个阶段都能单独 revert。

---

## 8. 本次落地的文件清单

| 文件 | 改动 |
|---|---|
| `crates/agent-toolkit/src/security.rs` | 新增总开关 |
| `crates/agent-toolkit/src/lib.rs` | 导出 `security` |
| `crates/agent-toolkit/src/fs_tools.rs` | `PathGate::workspace_or_unrestricted` 已移除；作用域走 `Scope` |
| `crates/agent-toolkit/src/plugin_tools.rs` | 插件工具按开关选路径门 |
| `crates/agent-toolkit/src/cmd_tools.rs` | `cwd` 按开关选直通 |
| `crates/agent-toolkit/src/background.rs` | 后台任务 `cwd` 按开关选直通 |
| `crates/agent-adapter/src/scope/access.rs` | `with_security_deferred` |
| `crates/agent-node/src/approval/gate.rs` | `with_security_deferred` |
| `crates/agent-node/src/scripts/mod.rs` | `to_engine_needs` 授予全部能力 |
| `crates/agent-node/src/scripts/subprocess.rs` | 子进程能力同上 |
| `crates/agent-host/src/lib.rs` | 组合根接入开关 |
| `AGENTS.md` | 顶部安全下线横幅 |
| `docs/architecture/PLAN-pluginization.md` | 本文 |

---

## 9. 立即可以开始的第一步

1. 按 §8 完成安全下线，跑 `cargo test -p agent-toolkit -p agent-adapter -p agent-node`。
2. 做 P0 的第一条：让插件开关真的能移除工具，并补守门测试。
3. P0 完成后，再决定 P1 的插件 crate 切分（`git` / `project` / `forge` / `batch` 先做）。
