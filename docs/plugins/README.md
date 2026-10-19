# 插件平台（现状）

> 本文描述 **当前实现**的插件平台：插件怎么被发现、能贡献什么、能干预到哪里、
> 用户能管到什么程度、出问题去哪儿看。
> 精确的 API 形状见 [extension-factory.md](extension-factory.md) 与
> [extension-api.d.ts](extension-api.d.ts)；本文只讲"怎么用"与"边界在哪"。

插件是 a_da 唯一的**扩展点**：它跑在 `crates/ts-engine`（Boa + oxc）里，
但**贡献在装配期结算**——插件不能中途改动工具表，也不能在轮内改自己的权限。

---

## 1. 包形态与发现规则

「一个插件」= **一个包目录**里的 `index.ts`（或 `index.js`），且必须
`export default` 一个**工厂函数**。

```
<workspace>/.ada/agent-coding/extensions/<id>/index.ts    工作区插件（scope = workspace）
~/.a-da/agent-coding/extensions/<id>/index.ts              全局插件（scope = global）
```

| 规则 | 说明 |
|---|---|
| 只认**包根** | 扫的是 `<root>/<id>/index.ts|index.js`；更深的 `src/index.ts`、散落的 `foo.ts` 都不会被发现 |
| 插件标识 = 目录名 | 改目录名 = 换一个插件；线上标识为 `workspace:<id>` / `global:<id>` |
| 单文件不支持 | 旧时代的"单文件插件 / 对象式默认导出"已下线，`plugin.validate` 会如实拒绝 |
| 没有模块解析器 | 沙箱**不支持**相对路径 import，插件必须自包含 |
| 能力声明 | 源码前 40 行内的注释 `// @ada-needs fs,net`；识别 `fs` / `fs=read` / `fs=write` / `net`(=`fetch`) / `none`。**没有指令时按地板给权限（fs 开、net 关）**；认不出的 token 不授予，并在装配期报 `unknown_need:<token>` |

生成一个正确的骨架用内置工具 [`create_plugin`](../quality/feature-catalog.md)（插件 `plugin-forge` 提供）。
它的 `scope` **必填**（`workspace` / `global`）：用户没说清装在哪时，工具**拒绝执行**并让模型
先用 `ask_user` 问用户——插件装在哪决定了它的生命周期（工作区插件随项目走、全局插件对所有工程生效），
猜错的代价是"同一个行为在一个工程里有、另一个工程里没有"，而用户不知道是哪次操作造成的。
（界面上「新建插件」弹窗里选作用域，同一个决定。）

**联网与代理**：声明 `net` 之后沙箱里才有 `fetch`（否则连 `globalThis.__native_fetch_async`
都不注册）。它支持 HTTP / SOCKS5 代理，两种写法：请求级
`fetch(url, { proxy: 'http://127.0.0.1:7897' })`（`127.0.0.1:7897` 这种不带 scheme 的也认），
或宿主进程的 `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`（大小写都认）。
🔴 **代理配错会直接报错，不会静默直连**——设了代理就意味着流量必须走代理，
悄悄发到目标站比请求失败严重得多。域名白名单（`A_DA_PLUGIN_NET_ALLOW`）在**建连之前**生效，
代理绕不过去。这些说明也会由 `create_plugin` 写进生成的技能正文与 `README.md`。

---

## 2. 三类脚本贡献 + 一类磁盘贡献（技能）

工厂函数里注册的贡献**在装配期结算**；技能则来自磁盘目录：

| 贡献 | 注册方式 | 落地位置 |
|---|---|---|
| 工具 | `ada.registerTool({ name, description, parameters, execute })` | 工具目录（`ToolCatalog`） |
| 系统提示词片段 | `ada.registerSystemPrompt({ id, content })` | 系统提示词，带 `plugin:<id>` 归因 |
| 技能 | **磁盘目录** `skills/<name>/SKILL.md`（见 §2.1） | 技能三来源之一（目录 / 插件 / 内置） |
| 钩子 | `ada.on(point, handler, { actions, order })` | 扩展点运行时（见 §3） |

⚠️ **必须写 `ada.metadata`**（工厂里**第一句**）：宿主在装配后读它，且
`tools` / `systemPrompt` / `hooks` 的 `count` 与 `names` 会**逐项、按顺序**
与实际注册比对——多注册一条忘了同步，装载期直接拒。
`skills` 是**声明性**字段（宿主以目录扫描为准），不参与这条校验。

工具的写入性**不由插件自述**：内置工具名走描述符（`registry.rs`），
第三方新工具名一律**按写操作 + 需要审批**处理（失败安全，`AGENTS.md` §2）。

### 2.1 技能：目录 + `SKILL.md`（Agent Skills 规范）

技能**不是**一个散装 `.md` 文件，而是一个目录：

```
skills/
└── <name>/
    ├── SKILL.md          # 必需：YAML frontmatter + Markdown 正文
    ├── references/       # 可选：按需读取的详细资料
    ├── scripts/          # 可选：可执行脚本
    └── assets/           # 可选：模板 / 图片 / 数据
```

Frontmatter 必填：

```yaml
---
name: deploy-runbook          # 必须等于目录名；1-64，仅 a-z / 0-9 / -，不以 - 起止，无 --
description: 发布流程：先跑测试再打标签。Use when the user asks to release.
---
```

- `description` 1-1024 字符，写清"做什么 + 什么时候用"——模型据此决定要不要激活它；
- 可选 `license`、`compatibility`（≤500）、`metadata`（字符串键值映射）、`allowed-tools`；
- 正文在技能激活时**整份加载**（建议 < 500 行）；更长的参考资料放 `references/`，按需读取；
- frontmatter 里的**未定义字段会报错**（拼错的键不会静默变成"已声明"）。

🔴 **2026-10-19 起不兼容旧形态**：`skills/<name>.md` 裸文件与脚本里
`ada.registerSkill({ body })` 都不再生效。旧形态以 `skill_spec_violation:*` 进插件诊断
（面板可见），不会静默消失。完整规范见 [`docs/SKILL_SPEC.md`](../SKILL_SPEC.md)。

工具的写入性**不由插件自述**：内置工具名走描述符（`registry.rs`），
第三方新工具名一律**按写操作 + 需要审批**处理（失败安全，`AGENTS.md` §2）。

---

## 3. 钩子点位与动作分级

点位单一真源：`agent_base::ports::hooks::points::IMPLEMENTED`（**33 个，全部可注册、全部有调用方**）。
声明即授权：`actions` 里没有的动作，运行期即使插件返回了也不生效（并记理由）。

**动作**：`observe`（只进诊断面）、`note`（往结果追一句旁注，**模型看得到**）、
`replace`（改写载荷，改完**重新校验**）、`deny`（拦下这一次行为）、`abort`（结束整轮）。

**分级**（`HookLevel` + `allows`，回答的是"谁能拦"）：

| 级别 | 第三方插件 | 内置插件 |
|---|---|---|
| **数据面** `beforeToolCall` / `afterToolCall` / `beforeTodoUpdate` / `afterTodoUpdate` | 可 `deny` / `replace` | 同左 |
| **控制面**（审批、轮次、会话、Agent、压缩、模型、子智能体、提示词、上下文…） | 只能 `observe` / `note` | 可 `deny` / `replace` |
| **只读面** `beforePersist` / `afterCheckpoint` / `beforeSkillLoad` / `afterSkillLoad` | 只能 `observe` / `note` | 同左 |

`abort` **只给内置**（它等于"拒绝服务"）。`replace` 只开放给 `REPLACE_SUPPORTED`
（`beforeToolCall` / `beforeTodoUpdate`）——载荷必须是"一份完整、可重新校验的数据"。

**成对是硬约束**：每个点位都是 `before` / `after` 一对，运行期发成对诊断
（`hook.entered` ↔ `hook.finished` / `hook.skipped`）；没有订阅者时如实报
`no_subscriber`，而不是静默跳过。

> 点位清单（按族）见 [quality/unfinished-features.md](../quality/unfinished-features.md) §一.9，
> 那里同时记着"哪些曾经声明了却没人调用"的历史教训。

---

## 4. 用户能决定什么（能力开关）

`PluginCapabilities`（全局配置，`plugin.capabilities.set` 可改）。
**改完立刻重装配**（2026-10-19）：保存能力开关后宿主会重建贡献集合，不需要重启产品。
（插件包本身的增删与 `index.ts` 改动同样即时生效——新建/删除/`plugin.reload` 都会触发重装配。）

| 开关 | 关掉之后 |
|---|---|
| `allowThirdPartyHooks` | 第三方钩子**装配期即拒**（`third_party_hooks_disabled`），内置不受影响 |
| `allowBuiltinShadow` | 插件工具不再顶掉同名内置工具（`builtin_shadow_disabled`） |
| `allowSystemPromptReplace` | 插件提示词片段不进系统提示词 |
| `allowTextRewrite` | `note` 旁注不再注入 |
| `allowThreadDeleteBlock` | `beforeThreadDelete` 的拦下不生效 |
| `allowCompactionReplace` | `beforeCompaction` 的拦下 / 改写不生效 |
| `hookTimeoutMs`（默认 500） | 单点位时间预算；**超时一律视为"无意见"** |

每一位都必须有生产消费者（`cargo xtask verify-wiring` 的 check L2 逐个核对）——
界面承诺了"关掉会发生什么"，就必须真的发生。

---

## 5. 沙箱与运行时

| 档位 | 启动方式 | 隔离性 |
|---|---|---|
| **进程内**（默认） | 无参数 | 进程内 Boa VM；**没有内存上限**（Boa 无堆限额 API） |
| **子进程** | `ada-coding --script-sandbox subprocess` | 独立 `runner` 进程 + 超时 `kill`；**同样没有内存上限** |

- `net` 策略**先于请求**判定：超时与域名白名单已生效，来源是环境变量
  `A_DA_PLUGIN_NET_TIMEOUT_SECS` / `A_DA_PLUGIN_NET_ALLOW`（配置文件与界面还没做）。
- 转译走 oxc 类型擦除；**运行时错误的行号是转译后 JS 的座标**（source map 未做）。
- 失败安全口径：脚本抛错 = 无意见；钩子超时 = 无意见。**没有**对反复失败的插件做熔断。

---

## 6. 管理面（协议方法）

| 方法 | 用途 |
|---|---|
| `plugin.builtinCatalog` | 内置插件目录（由工具注册表派生，不是写死清单） |
| `plugin.list` / `plugin.load` | 列表与详情；贡献字段来自**装配期活体视图**（真的注册上的才算） |
| `plugin.validate` | 只读自检：语法、`metadata` 契约、能力声明 |
| `plugin.setEnabled` | 启停；回执里带"本次实际改变了哪些贡献" |
| `plugin.reload` | 重新装配 |
| `plugin.delete` / `plugin.createTemplate` | 删除与生成骨架 |
| `plugin.capabilities.set` / `plugin.config.set` / `plugin.secret.set` | 能力开关 / 配置 / 密钥 |

**内置插件**（10 个，编译进产品）：`git-tools`、`code-outline`、`project-inspector`、
`test-runner`、`batch-ops`、`plugin-forge`、`todo-guard`、`approval-guard`、`ask-user`、`ponytail`。
其中 `approval-guard` 是**审批策略**的唯一归属（执行归核心，`AGENTS.md` §14）。

---

## 7. 诊断与排错

诊断点位词汇表在 `agent-base/src/domain/diag.rs`（**只登记真有发射方的点位**，
`verify-wiring` check K 盯着）。与插件相关的：

| 点位 | 什么时候发 |
|---|---|
| `plugin.discovered` / `plugin.skipped` | 扫到 / 跳过（`disabled`、`tool_not_registered`…）。**一个都没有也会发**（缺席可见） |
| `plugin.replaced_builtin` | 插件工具顶掉了同名内置工具——用户的信任工具被换了，必须报 |
| `script.invoked` / `script.finished` | 每次脚本调用（耗时、结果） |
| `hook.entered` / `hook.finished` / `hook.skipped` | 成对；`skipped` = **没有任何插件订阅这个点位** |
| `hook.replace` / `hook.replace_rejected` | 改写生效 / 改写被重新校验拒掉（形状、schema、路径越界） |

**常见失败对照表**：

| 现象 | 原因 |
|---|---|
| 装载期报"缺少 metadata 对象" | 没在工厂里写 `ada.metadata = ...`（写成 `export const metadata` 无效：默认导出会被换掉） |
| 装载期报 `metadata tools.count=N，实际为 M` | 注册项与 `metadata` 不同步 |
| 插件在、模型看不到工具 | 插件被停用，或贡献还没重载（贡献是装配期结算的，改完要重新加载 / 新开一轮） |
| 工具永远调不到 | 工具名含空格或中文（`create_plugin` 会当场拒），或 `execute` 形参写错（一个形参 = `(args)`，两个 = `(callId, args)`） |
| 钩子"声明了却没人调用" | 点位不在 `IMPLEMENTED` 里，或动作越权（控制面 `deny` 归内置），或能力开关关了——三种都会记理由，去诊断面板看 |

**已知小尾巴**（详见 [quality/unfinished-features.md](../quality/unfinished-features.md) §三）：
钩子内动态注册（`ctx.registerTool` / `registerSkill` / `registerSystemPrompt`）目前
**只收集、不生效**；工作区级能力开关覆盖不存在；运行时错误无 source map。

---

## 相关文档

| 文档 | 用途 |
|---|---|
| [extension-factory.md](extension-factory.md) | 工厂契约与 API 精确形状 |
| [extension-api.d.ts](extension-api.d.ts) | 插件作者的类型声明 |
| [../quality/unfinished-features.md](../quality/unfinished-features.md) | 插件相关的缺口与尾巴 |
| [../architecture/conventions.md](../architecture/conventions.md) | §12（钩子成对、工具集只能收窄）、§14（审批策略与执行分离）、§15（别断言钩子被调用，断言副作用） |
