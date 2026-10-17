# 代码冗余审计

> 审计日期：2026-10-17 ｜ 范围：`crates/`、`products/`、`desktop-ui/src/`、`electron/`、`tools/`、`docs/`、`archive/`
> 方法：全量扫描 + 8 行窗口归一化克隆检测 + 逐点人工核对；所有结论都附可复现的定位

---

## 一、结论摘要

**规模**

| 区域 | 文件 | 行数 | 备注 |
|---|---:|---:|---|
| `crates/`（Rust） | 214 | 65,399 | 内联测试 17,804 行（27%），逻辑约 47,595 行 |
| `crates/*/tests/`（集成测试） | 7 | 3,983 | 不含门禁脚本 |
| `products/`（产品二进制） | 2 | 873 | 两个 main.rs |
| `desktop-ui/src/`（前端） | 41 | 15,461 | |
| `electron/`（桌面壳） | 3 | 776 | |
| `tools/`（门禁与验收脚本） | 9 | 4,112 | 含 `xtask` 三个文件 |
| `archive/ts-legacy/`（TS 归档） | 272 | **68,033** | 主干 0 引用 |
| `docs/` | 24 篇 md | 约 1.1 MB 正文 | `du -sh` 3.9 MB（含图） |

**重复率**：在 30,513 行"非注释、非测试"的 Rust 代码中，**2,161 行（7.1%）** 落在 ≥8 行的逐字重复块里。
这个数字在同类工程里属于**偏低**水平；本仓库真正的冗余不在"复制粘贴"，而在 **"同一件事写了两三条并行实现路径"**。

**判定：三类冗余，性质完全不同，处理方式也不该一样**

1. **正面的单一真源**（值得保持）：工具描述符只有 `registry.rs` 一张表、`LONG_RUNNING_TOOLS` 有契约测试守着、前端 DTO 直接 `re-export` `crates/agent-proto/client-ts`。这些地方**没有**第二份手抄，是全仓最健康的部分。
2. **可机械消除的样板重复**（约 1,000～1,500 行，低风险）：时间戳、HTTP 客户端构造、日志/事件骨架、配置读改写。
3. **结构性"第二次实现"**（真问题）：初装 vs 重装两条装配路径、两份沙箱、三份协议名单、`EventSink` 三份骨架。这类重复**行数不多但漂移风险最高**——其中两处已经漂移了。

---

## 二、P0：可以现在就消除（低风险、纯收益）

### 2.1 两个产品把测试套件写进了生产依赖

`agent-conformance`（2,928 行 + 384 KB golden JSON）**只在 `#[cfg(test)]` 里被调用**：

- `crates/agent-node/src/scripts/mod.rs:610`（`test_passes_script_host_port_contract`）
- `products/ada-coding/src/main.rs:729`（`test_ada_coding_conformance`）
- `products/ada-skeleton/src/main.rs:136`

`agent-node` 的做法是对的（写在 `[dev-dependencies]`，`Cargo.toml:28-30`）；**但两个产品写在 `[dependencies]`**（`products/ada-coding/Cargo.toml:23`、`products/ada-skeleton/Cargo.toml:19`，两个文件都没有 `[dev-dependencies]` 段）。

后果：每次构建发布二进制都要编译 `agent-conformance` 及其 `golden/*.json`，并把 `agent-adapter` / `agent-runtime` / `agent-toolkit` 一起拉进产品依赖图（虽然它们本来也间接依赖）。

**动作**：两个产品各加一段 `[dev-dependencies]`，把行搬过去。零行为变化。

---

### 2.2 时间戳工具函数有 10 份逐字副本，其中一个还是 file-private

`fn now_ms()` 在 10 个文件里逐字重复（抽查 4 份，函数体完全一致）：

```
agent-toolkit/src/core/todo.rs:105          agent-node/src/checkpoint/manager.rs:400
agent-toolkit/src/plugin_tools.rs:279       agent-node/src/session/manager.rs:1216
agent-toolkit/src/background.rs:36          agent-node/src/subagents/tool.rs:94
agent-gateway/src/delegate.rs:654           agent-node/src/scripts/plugin_tool.rs:36
agent-gateway/src/registry.rs:455
agent-gateway/src/supervisor.rs:436
```

更值得注意的是：`agent-toolkit/src/background.rs:223` **已经有** `fn started() -> i64` 这个正确的辅助函数（本文件用了 13 次），但它是 file-private，于是其余 11 个工具文件继续手写 `SystemTime::now().duration_since(UNIX_EPOCH)`（共 26 处）：

```
core/finish.rs(1)  core/ask_user.rs(3)  core/todo.rs(1)  fs/batch.rs(7)
fs/edit.rs(2)  fs/list.rs(2)  fs/read.rs(2)  fs/search.rs(2)
fs/write.rs(2)  command/run.rs(2)  plugin_tools.rs(1)
```

**动作**：在 `agent-base`（零 IO 领域层，已有 `ports/clock.rs` 的 `Clock` 端口）或 `agent-adapter` 提一个 `pub fn now_ms()`；工具侧把 `started()` 上移。约省 40 行，更重要的是"时间语义"从此只有一处。

---

### 2.3 三个协议适配器各自重写 HTTP 客户端与 header 合并

`crates/agent-adapter/src/model/stream.rs`（1,170 行逻辑）里，`run_stream_openai_chat` / `run_stream_anthropic` / `run_stream_openai_responses` 三份实现重复了同一段请求准备：

| 重复内容 | 位置 |
|---|---|
| `reqwest::Client::builder().timeout(120s)` + proxy 解析（14 行） | `307`、`668`、`977` |
| 自定义 header 合并循环（11 行） | `329`、`691`、`999` |
| SSE 行解析与 `data:` 前缀处理 | 各自一份 |

差异只在鉴权头（`Authorization: Bearer` / `x-api-key` + `anthropic-version`）与 body 形状——**这三段本来就是同构的**。

**动作**：提 `fn build_http_client(cfg) -> Client`、`fn apply_custom_headers(&mut HeaderMap, cfg)`，鉴权头用一个 `match protocol` 的小函数。预计消掉 100～130 行，且"代理/超时"这类运维配置以后只改一处。

---

### 2.4 协议方法名有第三份名单，而它是死的

方法名在仓库里存在三份：

| 来源 | 形态 | 是否被校验 |
|---|---|---|
| `spec/proto/base.json` + `ada-coding.ext.json` | 72 个方法（声明） | `verify-spec` 校验产品声明，**不校验方法名与 Rust 常量一致** |
| `crates/agent-proto/src/methods.rs` | 常量 + `ALL_METHODS` | `verify-wiring` 校验它 ↔ `dispatch.rs` 的 match 臂 |
| `crates/agent-proto/client-ts/methods.ts` | 121 行常量 | **没有任何门禁，也没有任何 import** |

实测 `methods.ts` 在全仓**零引用**：`desktop-ui/src` 里没有一处 `from .../methods`，而 `ws-client.ts` 用的是 54 处**字符串字面量**（`this.request('session.initialize', ...)`、`'thread.send'`），全前端共 56 处。

也就是说：改一个方法名时，Rust 侧编译器会拦你，TS 侧**什么都不会拦**——`methods.ts` 那份常量既没被用，也没被比对。

**动作（二选一）**：
- **删** `client-ts/methods.ts`，承认前端用字面量（成本最低，但要接受 TS 无保护）；
- **接线**：`ws-client` 的 `request` 参数类型改成 `MethodName`（由 `ALL_METHODS` 派生），让 `methods.ts` 成为真源，并给 `verify-wiring` 加一条 "`methods.ts` 常量集合 ≡ `ALL_METHODS`" 的比对。

推荐后者：这正是"接线门"该管而没管的一格。

---

### 2.5 前端的工具中文标签表已经漂移

`desktop-ui/src/utils/tool-labels.ts` 的 `TOOL_LABEL` 有 25 条，其中 **3 条在 Rust 侧根本不存在**：

| 条目 | Rust 侧出现次数 |
|---|---|
| `find_symbol` | 0 |
| `check_subagent` | 0 |
| `read_url_content` | 0 |

而后端 `dispatch.rs:22-28` 明确写着"这里**不引入任何工具名清单**——一旦引入，界面就会再次广告不存在的工具（第 6 张名单的教训）"。结果第 6 张名单在**前端**又长出来了。

**动作**：删 3 条失效项；中期把标签改成由 `tool.declarations` 返回的 `summary` 派生（后端已有中文 `summary`），前端只保留"未知名回退"。

---

### 2.6 `EventSink` 的 INV-6 seq 校验被抄了三份

同一段 18 行（读 `last_seq` → 比较 → 违约记账 → 否则写回）逐字出现在：

- `crates/agent-rpc/src/server/events.rs:83`（`WsEventSink`）
- `crates/agent-rpc/src/runner/engine_bridge.rs:161`（`LoopEventBridge`）
- `crates/agent-rpc/src/runner/engine_bridge.rs:241`（`DirectEventSink`）

INV-6 是不变量。**不变量被复制三份，就等于没有不变量**——改判定规则（比如允许等于、或改为按 thread 分别计 seq）必然漏一处，而漏的那一处会静默放行违约。

**动作**：抽 `struct SeqGuard { last: AtomicU64, violations: Mutex<Vec<String>> }`，`fn check(&self, &AgentEvent)` 一处实现，三个 sink 各持一个 `SeqGuard` 字段。

---

## 三、P1：结构性"第二次实现"（需要小设计，风险高于 P0）

### 3.1 装配有两条路径：初装写一遍，重装再抄一遍 ⚠️ 最高优先级

| | 位置 | 规模 |
|---|---|---|
| 初装 | `crates/agent-host/src/lib.rs::run_from_spec`（`329-752`，约 420 行） | 读能力开关 → 建 `AssemblyBuilder` → 加内置工具 → 加插件工具 → 建技能/片段共享活体 |
| 重装 | `crates/agent-host/src/live.rs::LivePluginReassembler::reassemble`（`198-320`，约 120 行） | 同上，手抄 |

`live.rs` 的注释自己也承认这一点：

> 「P4：装配面的两位能力开关按**当前**配置来判……再启停插件，这时**重装配必须跟初装给同一个结论**（`ContributionPolicy` 是共用那一处）」

`ContributionPolicy` 确实共用了，但**装配流程**（收集插件工具 → 判内置影不影响 → `add_builtin_tool` → `add_plugin_tool` → `build()`）是两份独立代码。初装那边 `agent_toolkit::tools_for_toolkits(...).map_err(SpecError::Violation)?`，重装那边是 `match` + `tracing::warn!` + `return None`——**错误语义已经不一致了**。

漂移后果：用户装好插件、用着正常；关一次再开一次，某类工具（比如 `allowBuiltinShadow` 生效的那批）的归属就变了，而两次装配的"收据"对不上。

**动作**：把"收集 → 装配 → 摘要"提成一个 `fn assemble(spec, contributions, policy, workspace) -> Result<AssemblyPlan, AssemblyError>`，初装与重装都调它；重装只负责"把新 plan 通过 `LiveContributions::apply` 换进活体"。预计净减 80～100 行，并让"初装 = 重装"变成类型层面的保证而不是注释里的承诺。

---

### 3.2 路径沙箱有两份实现，而且**已经分叉** ⚠️ 安全相关

| 实现 | 规模 | `resolve_real_path` 行为 |
|---|---|---|
| `crates/agent-toolkit/src/sandbox.rs` | 164 行 | `canonicalize` 失败 → **回溯最深存在的祖先**（16 次循环）+ `strip_unc_prefix` |
| `crates/ts-engine/src/sandbox/mod.rs` | 100 行 | `canonicalize` 失败 → **原样返回**（无回溯） |

逐字重复的块约 **156 行**：`strip_unc_prefix`、`resolve_real_path`、`is_inside`、`realpath_deepest_existing`、`SKIP_DIRS`（后者只在 toolkit 侧有）。窗口扫描里这是全仓**单块最大的重复**。

危险点不是重复本身，而是**两份对"符号链接 + 不存在的目录"给出的答案不同**：一个会把 `a/../../.a-da/x` 归到保留目录并拒绝，另一个在 `canonicalize` 失败时直接放行原始路径。TS 插件运行时走的是第二份。

还有**第三份**：`crates/agent-gateway/src/fs_service.rs::resolve_path`（`31`）只做平台分隔符规范化，**完全不做沙箱判定**——它服务网关的 `fs.*` 方法，靠外层 `check_allowed_root` 兜。三份同名不同语义的 `resolve_path` 迟早会被误用。

**动作**：
- 短期：把 `ts-engine` 的 `resolve_real_path` 补成与 toolkit 一致（先消除行为差异，不动结构）；
- 中期：把这段路径逻辑抽成**零依赖小 crate**（`ada-path-safety`），`agent-toolkit` 与 `ts-engine` 都依赖它（`ts-engine` 目前刻意无内部依赖，所以只能走独立小 crate，不能反向依赖 toolkit）；
- 顺手把 `fs_service::resolve_path` 改名成 `normalize_separators`，名字与语义对齐。

---

### 3.3 配置有一个文件，四个写入者，各自 read-modify-write

`~/.a-da/config.json` 的"读 → parse → 改一个键 → `create_dir_all` → 整份写回"样板：

| 写入者 | 位置 |
|---|---|
| 插件启停 | `agent-node/src/plugins/manager.rs:119`（`toggle_plugin`，写点 `152`） |
| 插件能力位 | `agent-node/src/plugins/manager.rs:198`（`save_capabilities`，写点 `222`） |
| 插件专有配置 | `agent-node/src/plugins/manager.rs:227`（`save_config`，写点 `256`） |
| 应用配置 | `agent-rpc/src/server/dispatch.rs:76`（`save_app_config`） |

另外 `skills_state.json`（`skills/manager.rs`）、`gateway_workspaces.json`（`workspaces.rs`）各有一套自己的样板。

每个写入者都是"整份读改写 + 无锁"。两个并发写（用户在设置面板点保存、同时插件启停触发重装配）会丢更新。这也是 `AGENTS.md` 里"测试绝不能写用户真实 `~/.a-da`"那一串铁律的根因之一——多个写入者让隔离变得难以证明。

**动作**：提出 `AppConfigStore`（单锁 + `patch<F>(|v| ...)` + 原子写 `tmp → rename`），四个写入者改成调它。行数几乎不变，但把"共享可变文件"收敛成一个有并发语义的对象。

---

### 3.4 `ScriptHost` 的两个实现把 trait 骨架抄了两遍

`agent-node/src/scripts/mod.rs`（进程内 TS 宿主，563 行）与 `scripts/subprocess.rs`（子进程档，561 行）逐字重复约 **77 行**：

- `load()` 前导：取消检查 → `ScriptSource::Path` 解出 `(path, workspace)` → 拒绝 `Inline`（错误文案几乎相同）
- 5 个声明查询：`exports` / `tool_declarations` / `prompt_fragments` / `skills` / `hooks` —— 全都是 `Ok(self.lookup(handle)?.xxx)`

讽刺的是，这里**已经有**端口契约测试（`agent_conformance::verify_script_host_contract`），注释明确写着"通用断言写一次、两个实现都用它验；各写一遍的断言迟早会分叉"。行为层做到了单源，实现层没有。

**动作**：把 `lookup` 与 5 个声明查询提成 `trait ScriptRegistryAccess` 的默认方法（两个实现只需提供 `lookup`）；`load()` 的 workspace 推导提成 `fn resolve_load_target(req) -> Result<(PathBuf, PathBuf), ScriptError>`。约减 60 行。

---

### 3.5 网关的 `register` 处理写了两次

`crates/agent-gateway/src/relay.rs`：

- `516`（`serve_agent_control` / 匿名连接）：`register` + `info` + `listAgents` + `status`
- `888`（`relay` 的管理面）：`register` + `attach` + `detach` + `info` + `status` + …

两处 `register` 约 45 行，同构，差异只有变量名（`agent_id` / `id`、`gateway` / `gw`）和日志文案。**而且不等价**：`serve_agent_control` 那份会把注册结果记进 `registered_agent_id`（声明 `493`、赋值 `550`、断连时清理 `581`），`relay()` 那份**不记**——任何从这个入口注册的实例，连接断开后都没有清理路径，只能等 15 分钟空闲回收（`ReapPolicy`）。这是"复制时漏了一行"的典型后果。

**动作**：抽 `async fn handle_register(gw: &Gateway, params: &Value) -> Result<(String, Value), String>`，两处都调；清理注册交给调用侧统一的 `Drop`/守卫。

---

### 3.6 `AFTER_TURN` 钩子的 payload 写了两份

`crates/agent-base/src/engine/runtime.rs` 的 `run_turn` 有两条退出路径（"无工具调用自然结束" `957` 与"有工具调用后结束" `1484`），各自内联了一个 **12 字段**的 `serde_json::json!`：

```rust
"step", "threadId", "status", "assistantTextChars", "thinkingChars",
"requestedToolNames", "toolResults", "usage", "llmDurationMs", "toolsDurationMs"
```

这是给插件钩子的公开契约。加一个字段（比如加 `stopReason`）必须改两处，漏一处就变成"钩子在某些轮次里拿不到字段"——而且**不会有编译错误**。

**动作**：提 `fn after_turn_payload(...) -> serde_json::Value`，两处调用。约 18 行 × 2 → 1。

---

### 3.7 协议参数完全没有类型，"声明的契约"是第三份手写

`dispatch()` 一个函数 **2,414 行**、**76 个 match 臂**，所有参数靠手写取值：

- `params.get("x").and_then(|v| v.as_str())` —— **116 处**
- `.and_then(|v| v.as_u64())` —— 10 处
- `ok_or_else` 构造 `invalid_params` —— 39 处

而 `crates/agent-proto/src/dto.rs`（539 行）只定义了**一个** `...Result` 结构体（`InitializeResult`），没有任何 `...Params`。`spec/proto/*.json` 里写着的参数形状，**没有任何代码或门禁校验**。

这是本仓库最大的一处"隐性冗余"：同一份参数契约有三份表示（spec JSON / dispatch 手写解包 / 前端 TS 类型），三者之间零机械约束。

**动作（分两步，不要一次做完）**：
1. 给高频方法（`thread.send`、`config.setProvider`、`provider.save`、`change.revert*`、`plugin.*`）先补 `#[derive(Deserialize)]` 的 `...Params`，`dispatch` 改用 `serde_json::from_value` 一次性解包——错误处理顺带统一（现在 39 处 `ok_or_else` 文案各不相同）；
2. 给 `verify-wiring` 加一条：`spec/proto/*.json` 声明的方法必须能对上 `dto.rs` 里的 Params/Result 名（或显式标 `untyped: true` 白名单）。

---

## 四、P2：上帝文件（不是"重复"，但它是重复的温床）

先区分**逻辑行**与**内联测试行**（很多"几千行的文件"其实是测试撑起来的）：

| 文件 | 总行 | 内联测试 | 逻辑行 |
|---|---:|---:|---:|
| `agent-rpc/src/server/dispatch.rs` | 4,694 | 661 | **4,033** |
| `agent-base/src/engine/runtime.rs` | 3,295 | 1,738 | 1,557 |
| `agent-node/src/hooks/mod.rs` | 1,923 | 933 | 990 |
| `agent-node/src/session/manager.rs` | 1,775 | 553 | 1,222 |
| `agent-host/src/lib.rs` | 1,687 | 852 | 835 |
| `agent-rpc/src/state/store.rs` | 1,669 | 482 | 1,187 |
| `agent-adapter/src/model/stream.rs` | 1,480 | 310 | 1,170 |
| `agent-node/src/plugins/sandbox.rs` | 1,423 | 561 | 862 |
| `agent-gateway/src/relay.rs` | 1,004 | 0 | 1,004 |
| `agent-rpc/src/runner/engine_bridge.rs` | 1,003 | 626 | 377 |
| `desktop-ui/src/components/SettingsModal.tsx` | 1,769 | 0 | 1,769 |
| `desktop-ui/src/components/Transcript.tsx` | 1,724 | 0 | 1,724 |
| `desktop-ui/src/client/ws-client.ts` | 1,642 | 0 | 1,642 |

`dispatch()` 单个函数 2,414 行，是全仓最长的函数。它同时承担四件事：协议解包、状态变更、引擎编排、事件投影。这也是为什么 §3.7 的参数类型化迟迟做不动——**没有地方放**。

**建议（按性价比）**：
1. 先按"域"切 `dispatch` 的臂：`session/ui`、`thread`、`config/provider`、`change/checkpoint`、`plugin/skill`、`gateway`。每域一个 `fn dispatch_xxx(&self, params) -> Result<Value, ProtocolError>`，`match method` 只做转发。方法覆盖门禁（`check_dispatch_arms`）需要跟着改成正则扫多个文件——**这一步的收益是让 §3.7 变得可行**。
2. `relay.rs` 1,004 行无测试，却承担 §3.5 里已经漏掉的清理逻辑——建议补一条"注册 → 断连 → 实例被回收"的 e2e（`delegate_e2e.rs` 已有骨架）。
3. 前端 `SettingsModal` / `Transcript` 各 1,7xx 行：`SettingsModal` 里有 19 处同样的 `flex items-center justify-between` 行结构，可提 `<SettingsRow label desc>{control}</SettingsRow>`；`Transcript` 建议按卡片类型拆子组件（它已经在按 `ItemKind` 分支，边界现成）。

---

## 五、P3：仓库级死重量

### 5.1 `archive/ts-legacy`：68,033 行、36 MB、零引用

`verify:archive`（`tools/verify-archive.ts`）会**主动阻止**主干引用它——这本身是好设计。但 `archive/` 仍被 git 跟踪，于是：

- 每次 clone 多拉 36 MB；
- 全仓搜索（含 AI 助手、IDE 索引、`grep`）都会被 68k 行已作废的 TS 干扰；
- 它保留着"第二份引擎"的完整形态，任何人想找实现时都可能先读到它。

**建议**：迁到独立分支/tag（`git mv` + 归档 tag），主仓删除；`verify:archive` 改成"主干不存在 `archive/` 目录"的正向断言。若需要可查阅，README 给一条 `git checkout archive/ts-legacy` 的说明即可。

### 5.2 插件系统有 5 篇设计文档、两代并存，共 306 KB

| 文档 | 大小 |
|---|---:|
| `docs/plugin-system-design.md` | 91,635 B |
| `docs/plugin-system-dev-plan.md` | 50,554 B |
| `docs/plugin-system-optimization-plan.md` | 5,094 B |
| `docs/script-plugin-system-design.md` | 82,364 B |
| `docs/script-plugin-system-dev-plan.md` | 76,258 B |

`script-plugin-system-*` 是后一代（脚本插件），`plugin-system-*` 是前一代，但**没有任何一处标注"前者已被取代"**。而 `docs/` 的分类规则是"设计文档是唯一口径"（`AGENTS.md`），两份口径并存会直接导致实现分歧。

**建议**：在前一代文档顶部加 `> ⚠️ 已被 script-plugin-system-design.md 取代（保留作历史）`，或直接并入 `docs/archive/`。

### 5.3 `AGENTS.md` 25 KB 自己就是重复源

它的开篇写着"深度约定全文已迁至 `docs/agent-conventions.md`，只留接手就必须知道的事"，但实际仍保留了大量与 `agent-conventions.md` 重叠的段落（并且长到会被系统提示词截断——本次会话里它就被截断了）。被截断意味着**后半部分等于不存在**，而作者以为它在。

**建议**：按"必须注入"与"查阅即可"切两段，把查阅部分全部推到 `agent-conventions.md`，`AGENTS.md` 控制在 8 KB 内。

### 5.4 本地产物

`dist/`（165 文件）、`dist-electron/`、`tmp/` 均已被 `.gitignore` 排除（`git ls-files` 计数为 0，干净）。但 `tmp/` 里已积累 197 个文件（124 个 `.log`、28 个 `.png`），包含历史诊断产物。

**建议**：无需入库动作，定期清理即可（本次审计新增的 4 个扫描脚本也在 `tmp/`，可随手删）。

---

## 六、看起来像冗余、但**不建议动**的部分

这部分要明确写出来，否则下一次"去重"会误伤设计：

| 现象 | 为什么必须保留 |
|---|---|
| 8 个端口 trait 各有 2～3 个实现（`Scope`、`SessionStore`、`ModelClient`、`EventSink`…） | 端口/实现分离是 INV-2 的基石；`agent-conformance` 就是为"多个实现必须同一套契约"而存在的 |
| `agent-rpc/src/state/store.rs`（内存卡片）与 `agent-node/src/session/manager.rs`（JSONL 磁盘）两份 item 列表 | 这是"界面即时性"与"落盘可恢复"的分离；`AGENTS.md` 已把 `at` 严格递增写成两处都必须守的规则，属于**受控**的重复 |
| Rust DTO 与 `client-ts/dto.ts` 两份类型 | 跨语言无法避免。当前 `desktop-ui/src/types/index.ts` 直接 `export * from '../../../crates/agent-proto/client-ts'`，**没有**第二份手抄——这是正确做法，保持即可 |
| `LONG_RUNNING_TOOLS` 与各工具的 `long_running()` | 看似两份，但有 `test_long_running_set_matches_tool_declarations` 机械守门（`registry.rs:651`）。这是"用测试守住单一真源"的正面样板 |
| 3 个 `run_stream_*` 存在（而非合成一个） | 三家协议 SSE 语义差异真实存在；要抽的是**请求准备**（§2.3），不是把三份解析器合成一个巨大状态机 |

---

## 七、建议执行顺序

**第 1 批（半天，零风险，建议立刻做）**
1. 两个产品 `agent-conformance` → `[dev-dependencies]`（§2.1）
2. 删 `TOOL_LABEL` 的 3 条失效项（§2.5）
3. `now_ms` / `started()` 收敛到一个 `pub` 辅助（§2.2）
4. `EventSink` 的 seq 校验抽 `SeqGuard`（§2.6）

**第 2 批（1～2 天，需要跑门禁）**
5. `stream.rs` 抽 `build_http_client` / `apply_custom_headers`（§2.3）
6. 装配流程合成 `fn assemble()`（§3.1）—— **收益最大**
7. 网关 `register` 合并且修正断连清理（§3.5）
8. `AFTER_TURN` payload 抽函数（§3.6）

**第 3 批（需设计，单独排期）**
9. 沙箱路径逻辑收敛为小 crate（§3.2）
10. `AppConfigStore`（§3.3）
11. `dispatch` 按域拆分 + 参数类型化（§4 + §3.7）
12. `methods.ts` 接线或删除 + 门禁补一格（§2.4）

**第 4 批（决策问题，不是技术问题）**
13. `archive/` 迁出主仓（§5.1）
14. 两代插件设计文档标注作废（§5.2）
15. `AGENTS.md` 瘦身（§5.3）

---

## 八、执行记录（2026-10-17）

第 1、2 批已落地；第 3、4 批未动（需要设计决策，见上方排期）。逐项如下。

| # | 项 | 状态 | 落地形态 |
|---|---|---|---|
| 1 | §2.1 产品依赖 | ✅ | 两个 products 各加 `[dev-dependencies]`：`conformance` 移入；`ada-coding` 的 `agent-toolkit` 也只在测试里用，一并移入；`ada-skeleton` 的 `agent-toolkit` 全无引用，直接删除 |
| 2 | §2.5 前端标签 | ✅ | 删 `find_symbol` / `check_subagent` / `read_url_content`；补上真存在但漏登记的 `finish`、`inspect_project`（别名）；模块注释改为"界面文案，不是工具清单" |
| 3 | §2.2 时间戳 | ✅ | 新增 `agent-toolkit/src/time.rs::now_ms()`（放在依赖链最下层，见下方说明）。10 份定义 → 2 份：工具/适配/节点三层共用一份；网关因"只依赖 agent-proto"的硬边界保留 `supervisor::now_ms`（已在文档注释里写明是**受控**重复）。`background.rs` 的 file-private `started()` 随之删除，13 处调用改 `crate::now_ms()`。工具包内 26 处手写 `SystemTime::now()` 全部消除；其中 11 处原本是 `.unwrap_or(started_at)`，改写为 `now_ms().max(started_at)` 以保持"回执的 finished_at 不得早于 started_at" |
| 4 | §2.6 SeqGuard | ✅ | 新增 `agent-rpc/src/seq_guard.rs`，三个 sink 各持一个 `SeqGuard` 字段，`seq_violations()` 委托给它；附 2 条单测。**行为不变**：违约只记账不丢事件 |
| 5 | §2.3 stream.rs | ✅ | 新增 `build_http_client()` / `build_headers()` + `AuthStyle` 枚举，三处 `run_stream_*` 各剩两行。净减 10 行，但**代理/超时/header 合并只剩一处**（这才是目的） |
| 6 | §3.1 装配双路径 | ✅ | 新增 `agent-host/src/lib.rs::assemble(spec, workspace, allow_builtin_shadow, plugin_tools) -> Result<AssemblyPlan, SpecError>`；`run_from_spec` 用 `?`（启动期必须炸），`LivePluginReassembler::reassemble` 用 `match` + `warn` + `None`（运行期只能放弃并记账）。**流程不会再分叉，处置差异留在调用侧且各有理由** |
| 7 | §3.5 网关 register | ✅ | 抽出 `handle_register(&Gateway, &params) -> Result<(String, Value), String>`；两处调用；新增 `RegisteredAgentGuard`（`Drop` 时 `set_status(Stopped)`），`relay()` 那份补上了原本缺失的断连清理 |
| 8 | §3.6 AFTER_TURN | ✅ | 新增 `after_turn_payload(...)`，两条退出路径共用；`status` 作为参数显式传入（当前两处都是 `"completed"`） |
| 12 | §2.4 methods.ts | 🟡 部分 | **没有接线**，但补了门禁：`verify-wiring` 新增 `check_ts_methods_mirror`，比对 `methods.ts` ↔ `methods.rs` 的常量名与字面量（当前 82 ↔ 82，差异 0）。从此漏改一边就会红 |

**验证**（全部通过）

```bash
cargo check --workspace --all-targets          # 0 error，无新增 warning
cargo test --workspace -- --test-threads=1     # 全绿
cargo run -p xtask -- verify-wiring            # ✔ 无违约（含新增的 TS 方法常量比对）
npm --prefix desktop-ui run typecheck          # tsc --noEmit 通过
```

**一个与本次改动无关的既有现象**：`cargo test --workspace` **不加** `--test-threads=1` 时，
`agent-node` 的 4 条 `scripts::tests::*`（预算/needs 相关）会随机失败；串行跑 128 条全过。
`package.json` 的 `test` 脚本本来就带 `--test-threads=1`——说明这是已知的并行测试隔离问题
（这些用例共享临时插件目录），不在本次改动范围内，但值得单独修。

**下一步（第 3 批）建议顺序**：先 §3.2 沙箱（安全相关，且是全仓单块最大重复）→ §3.3
`AppConfigStore`（并发丢更新）→ §4 拆 `dispatch` + §3.7 参数类型化（先拆才有地方放类型）。

---

## 附录：复现方式

```bash
# 规模
for d in crates/*/; do echo "$d $(find "$d" -name '*.rs' | wc -l) files"; done

# 逐字重复块（8 行窗口，去注释/去测试/归一化空白）——本次口径
#   逻辑行 30,513；落入重复块 2,161 行（7.1%）
#   （脚本见 tmp/dup_ratio.py，可按需调整 WINDOW）

# 具体证据
grep -rn "fn now_ms" --include=*.rs crates                 # 10 份
grep -rn "reqwest::Client::builder()" crates/agent-adapter/src/model/stream.rs   # 3 处
grep -n '"register" =>' crates/agent-gateway/src/relay.rs   # 2 处
grep -c "and_then(|v| v.as_str())" crates/agent-rpc/src/server/dispatch.rs       # 116
grep -rn "from '.*methods'" desktop-ui/src                  # 0 处（methods.ts 零引用）
```
