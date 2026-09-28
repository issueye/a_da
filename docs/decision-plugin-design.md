# 决策插件（decision）设计文档

> 参考项目：[`E:\code\github\pi-jev`](E:/code/github/pi-jev)（pi-jev v0.6.0，2304 行）
> 目标项目：a_da
> 状态：**设计待评审**，尚未实现

---

## 1. 背景与目标

### 1.1 pi-jev 做了什么

pi-jev 是 Pi coding agent 的扩展，核心价值是**给智能体补上「结构化判断」这个能力**：

它把 TypeSafe Jev（System One）模型包装成一个决策原语，支持三种类型化问题：

| 类型 | 语义 | criteria 形态 |
|---|---|---|
| `choice` | 从若干选项中选一个 | `{ 选项key: 描述 }` |
| `noul` | 是/否概率（0–1） | 无 |
| `score` | 按评分标准打分 | `["最高档", ..., "最低档"]` |

围绕这个原语，pi-jev 铺了七层能力：工具路由、技能发现、类型化决策、决策设计器、决策门禁、
工具调用守卫（防幻觉）、压缩时保留关键历史、子智能体 RPC。

它有一条很克制的设计原则，值得原样继承：

> **On-Demand & Safe**: Runs when called. No unsolicited per-turn API token costs.
> Fails closed safely: if Jev is unreachable or unconfigured, tool routing does not
> blindly activate unjudged tools and reports zero confidence on keyword fallbacks.

以及「成本透明」：每个会消耗 Jev 请求的入口都在 README 里被明确列出。

### 1.2 a_da 为什么要它

a_da 现在的模型侧完全是「生成文本」范式。当任务需要的是**判断**而不是**生成**时，会出问题：

- **验收判定**：「这次改动是否满足验收标准？」——让模型生成一段评价，不如要一个概率 + 阈值。
- **严重度分级**：代码审查产出的缺陷要分 `[严重]/[隐患]/[建议]`，现在靠自然语言描述，
  下一轮推理要重新解析一遍文本。
- **归因分类**：测试失败原因（路径不存在 / 语法错误 / 权限 / 真实逻辑失败）是四选一，
  适合 `choice` 而不是一段散文。
- **决策留痕**：判断结论应当可比较、可累计、可设阈值，而不是淹没在长回复里。

**关键约束**：a_da 没有 Jev 那样的专用 System One 后端，`streamModelChat`
是纯 OpenAI 兼容接口（**不支持 JSON mode，也不返回 logprobs**，已确认）。
所以「校准概率」必须有明确的来源与诚实的能力边界——这是本设计最需要小心的地方（见 §4）。

### 1.3 本次范围（已确认）

- ✅ **决策核心**：`choice` / `noul` / `score` 三种类型化决策
- ✅ **决策设计器**：自由提示词 → 自动设计决策 schema → 立即评估
- ✅ **决策门禁**：对 git diff / 文件 / 文本做「是否满足验收标准」判定
- ✅ **可插拔双引擎**：远端 Jev 兼容端点 + 本地模型自评，均不可用时退回确定性启发式
- ✅ **按需触发**：只在模型主动调工具时发生，不产生擅自的每轮开销
- ❌ 工具路由、技能发现、工具调用守卫、压缩集成、自动模式 → 见 §9「后续演进」

---

## 2. 关键设计原则

这四条决定了实现里的每一个取舍，先立在这里：

1. **绝不捏造确定性。** 引擎不可用时返回 `ok: false`，不返回编造的数字。
   这是全插件最重要的一条安全属性。
2. **概率必须标注来源。** 每条答附 `engine` 与 `calibrated` 字段。
   远端 Jev 是校准的；本地自评**不是**，必须如实标注（见 §4.3）。
3. **失败安全，方向要分场景。** 判断类（`decide`）失败即失败，不猜；
   门禁类（`check_gate`）提供 `fail_open`，由调用方显式选择放宽。
4. **单一阈值常量。** 像 pi-jev 的 `JEV_THRESHOLD` 一样，所有路径读同一个
   `DEFAULT_DECISION_THRESHOLD`，避免各处理解不一致。

---

## 3. 与 pi-jev 的差异对照（这些差异决定了移植不能照搬）

| 能力面 | pi-jev | a_da 对应物 | 影响 |
|---|---|---|---|
| 决策后端 | `@typesafe-ai/sdk` 的 `systemOne()` | 无 | 必须自建引擎抽象（§4） |
| 模型调用 | `ctx.modelRegistry.complete(model, ...)` | `readLlmConfig()` + `streamModelChat()` | 设计器照搬逻辑、换调用方式（§6） |
| 插件契约 | `ExtensionAPI`（registerTool/registerFlag/on） | `BuiltinPluginPackage`（tools/skills/prompts） | 交付形态改为「三位一体」（§7） |
| 工具激活 | `getActiveTools()/setActiveTools()` | **无此概念**，工具始终全量下发 | 工具路由在 a_da 需反向收窄，v1 不做（§9） |
| 轮次拦截 | `before_agent_start` 等事件 | **扩展无法拦截轮次开始** | 自动模式需改核心，v1 不做（§9） |
| 子智能体门禁 | pi-subagents 的 `gate` 参数 | a_da 子智能体无 gate 参数 | v1 只提供工具；CLI 见 §8.4 |
| 密钥来源 | `TYPESAFE_API_KEY` / secret 文件 | `A_DA_*` 环境变量 + `config.json` | 兼容读取旧变量名（§5.3） |

两个 a_da 特有的**必须遵守**的约定（来自 `AGENTS.md`）：

- **新增面向子智能体的能力必须同步更新白名单**，否则子智能体拿不到（踩过这个坑）。
- **只读工具必须登记进 `READ_ONLY`**，否则会被 `isWriteTool` 的失败安全判定当成写工具，
  只读子智能体与 plan 模式都用不了。

---

## 4. 引擎抽象（核心设计）

### 4.1 统一接口

```ts
// src/agent/tools/builtin-plugins/decision/types.ts

export type QuestionType = 'choice' | 'noul' | 'score'

export interface ChoiceQuestion {
  type: 'choice'
  instructions: string
  criteria: Record<string, string | null>
}
export interface NoulQuestion {
  type: 'noul'
  instructions: string
}
export interface ScoreQuestion {
  type: 'score'
  instructions: string
  criteria: string[]          // 评分档位，最高档在前
}
export type DecisionQuestion = ChoiceQuestion | NoulQuestion | ScoreQuestion

export interface DecisionRequest {
  /** 被判断的材料：字符串或结构化对象 */
  state: string | Record<string, unknown>
  /** 问题 id → 问题定义 */
  questions: Record<string, DecisionQuestion>
  /** 单题覆盖阈值（不传用 DEFAULT_DECISION_THRESHOLD） */
  threshold?: number
}

export interface DecisionAnswer {
  type: QuestionType
  /** choice=选项key；noul=概率 0–1；score=档位名 */
  value: string | number
  /** 引擎自报置信度（可能缺省） */
  confidence?: number
  /** choice/score 的分布：选项 → 占比 */
  distribution?: Record<string, number>
  /** 是否经过校准。远端 Jev=true，本地自评/启发式=false */
  calibrated: boolean
}

export interface DecisionResponse {
  answers: Record<string, DecisionAnswer>
  engine: EngineId
  model?: string
  elapsedMs: number
  /** 采样次数（本地引擎 >1；远端/启发式为 1） */
  samples: number
  /** 非致命问题，例如"本地自评概率未经校准"、"state 被截断" */
  notes: string[]
}

export type EngineId = 'jev' | 'local' | 'heuristic'

export interface DecisionEngine {
  readonly id: EngineId
  /** 是否可用（配置齐备/依赖就绪） */
  isAvailable(): Promise<boolean>
  evaluate(req: DecisionRequest, signal?: AbortSignal): Promise<DecisionResponse>
}
```

### 4.2 引擎解析顺序（`engine: 'auto'`）

```
jev       —— 配了 Jev 兼容端点（§5.3）时首选，忠实还原 pi-jev 语义
  ↓ 不可用
local     —— readLlmConfig() 非 null 时可用（复用用户已配置的模型）
  ↓ 不可用
heuristic —— 确定性规则，永远可用，但校准度为 0，只做兜底
```

解析结果会被缓存并在 `decide` 的返回 `notes` 中说明实际用了哪个引擎。

### 4.3 本地自评引擎：怎么从聊天模型里拿到「概率」

这是本设计最需要诚实处理的部分。聊天模型自报的概率**系统性偏乐观**（动辄给 0.9），
直接采信等于制造虚假确定性。所以本地引擎**不采信单次自报值，而用多样本投票**：

- **`noul`（是/否概率）**：以 temperature > 0 采样 N 次（默认 3），每次要求模型输出
  `{"answer": "yes"|"no", "probability": 0.xx}`。
  **主值取「yes 票数 / N」（投票占比）**，而非自报概率的均值——投票占比在经验上比
  自报数值稳定得多。自报均值作为参考放进 `notes`。
  同时输出 `distribution: { yes: p, no: 1-p }`。
- **`choice`**：采样 N 次，统计各选项票数 → 天然得到 `distribution`，
  主值取票数最高者；**并列时按 criteria 的 key 顺序取第一个**（确定性打破平局）。
- **`score`**：采样 N 次，统计各档位分布；主值取**加权期望档位**（按 criteria 顺序
  映射到数值），比众数更能反映"倾向"。

`DecisionAnswer.calibrated` 一律为 `false`，且 `notes` 里必须写明：

> 本地自评：概率来自 N 次采样的投票占比，**未经校准**，不宜直接当概率解释。

N 次采样用 `Promise.all` 并发（同一请求的多次采样彼此独立）。
每次采样带超时（默认 20s）与整体预算（默认 60s），避免挂死；`signal` 全程贯通。

### 4.4 启发式兜底：只说「不知道」

启发式**不做任何真实判断**，只保证工具可用且不误导：

- 恒定产出 `calibrated: false`、`confidence: 0`，`notes` 明确写出
  「无可用决策引擎，以下为占位结果，请勿据此决策」。
- 对 `choice` 返回 criteria 的第一个 key，对 `noul` 返回 `0.5`，对 `score` 返回中间档位——
  即刻意选择**中性的、不指引任何方向**的值。
- **`check_gate` 在启发式下默认 fail-close**（视为未通过），除非显式传 `fail_open: true`。

理由：pi-jev 的失败哲学是"reports zero confidence on keyword fallbacks"而不是猜。
门禁场景里"猜通过"的危害远大于"要求人工复核"。

---

## 5. 配置

### 5.1 config.json 新增块

沿用 a_da 既有做法（`readSavedConfig()` 的开放索引签名，未知键必须原样保留）：

```json
{
  "decision": {
    "engine": "auto",
    "baseUrl": "http://localhost:8000",
    "apiKey": "",
    "threshold": 0.65,
    "samples": 3,
    "timeoutMs": 20000
  }
}
```

- `engine`: `"auto" | "jev" | "local" | "heuristic"`，默认 `auto`
- `threshold`: 默认 `0.65`（对齐 pi-jev 的 `JEV_THRESHOLD`）
- `samples`: 本地引擎采样次数，默认 3；设为 1 可省开销（但会失去投票稳定性）

### 5.2 环境变量

| 变量 | 用途 |
|---|---|
| `A_DA_DECISION_ENGINE` | 覆盖 engine |
| `A_DA_DECISION_BASE_URL` | Jev 兼容端点 |
| `A_DA_DECISION_API_KEY` | 端点密钥 |

### 5.3 兼容读取（降低迁移成本）

按以下顺序解析 Jev 端点与密钥，先命中先用：

```
baseUrl: A_DA_DECISION_BASE_URL → PI_JEV_BASE_URL → TYPESAFE_BASE_URL
apiKey:  A_DA_DECISION_API_KEY  → TYPESAFE_API_KEY
         → ~/.a-da/secrets/decision_api_key
```

`~/.a-da/secrets/` 是 a_da 数据目录（`getAppHome()`）下的新约定，
对齐 pi-jev 的 `~/.pi/agent/secrets/typesafe_api_key`，避免把密钥写进 `config.json`。

> 注意：环境变量优先级高于 `config.json`，与 `readLlmConfig()` 的既有约定一致。

---

## 6. 三个工具

### 6.1 `decide` —— 类型化决策

```ts
parameters: {
  state: string | object        // 被判断的材料
  questions: Record<string, {
    type: 'choice' | 'noul' | 'score'
    instructions: string
    criteria?: object | string[]
  }>
  threshold?: number
}
```

返回：每个问题的 `value` / `confidence` / `distribution` / `calibrated`，
以及 `engine`、`samples`、`elapsedMs`、`notes`。
`details` 里带完整结构化结果，供卡片与后续推理消费。

执行模式 `sequential`（要读配置、可能发多次请求）。

### 6.2 `design_decision` —— 自由提示词 → 决策 schema → 评估

对齐 pi-jev 的 `/jev test <prompt>`。两步：

1. **设计**：用 a_da 自己的模型（`readLlmConfig()` + `streamModelChat`，
   `tools: undefined`，专用系统提示词）产出 `{ state, questions }`。
   系统提示词基本照搬 pi-jev 的 `DESIGN_SYSTEM_PROMPT`（含 `MAX_DESIGNED_QUESTIONS = 6`
   上限与三条类型规则）。
2. **评估**：把设计结果交给引擎，返回结论。

**校验必须严格**——`validateDesign()` 直接复用 pi-jev 的防御逻辑（这是它写得最好的部分之一）：

- 丢弃缺 `instructions` 的问题
- `choice` 的 `criteria` 必须是**非空对象**（不是数组）
- `score` 的 `criteria` 必须是**非空数组**
- `noul` **不得**带 `criteria`
- 一个问题都没通过校验 → 整体失败，报「模型未返回可用的决策 schema，请换个说法」

`extractJson()` 也要照搬（容忍 ``` 围栏与前后散文，取第一个 `{...}`）。

参数：`prompt: string`、可选 `max_questions`（≤6）、`model`。

### 6.3 `check_gate` —— 验收标准门禁

对齐 `pi-jev-gate`，判定「这份产出是否满足验收标准」，返回概率与是否通过。

```ts
parameters: {
  criteria: string                    // 验收标准（自然语言）
  source?: 'diff' | 'file' | 'text'   // 默认 diff
  file?: string                       // source=file 时必填
  text?: string                       // source=text 时必填
  threshold?: number                  // 默认 0.70（对齐 pi-jev）
  fail_open?: boolean                 // 引擎不可用时是否放行，默认 false
}
```

`source=diff` 通过 `execFileAsync('git', ['diff', 'HEAD'])` 取（失败则退 `git diff --cached`），
与 `git-tools.ts` 的执行方式一致。返回 `passed` / `probability` / `threshold` /
`engine` / `calibrated` / `elapsedMs`。

**在启发式引擎下默认 fail-close**（`passed: false`），除非 `fail_open: true`。
这是刻意与 `decide` 不同的方向：门禁猜错的代价不对称。

---

## 7. 插件交付形态：三位一体

对齐 `batch-ops` / `test-runner` 的既有形态（`BuiltinPluginPackage`）：

```ts
export const decisionPlugin: BuiltinPluginPackage = {
  id: 'decision',
  name: '决策与判定 (decision)',
  description: '类型化决策（choice/noul/score）、自由提示词决策设计器与验收门禁。',
  tools: [createDecideTool, createDesignDecisionTool, createCheckGateTool],
  skills: [/* decision-discipline */],
  prompts: [/* decide, gate */],
}
```

- **技能 `decision-discipline`**：教「什么时候该用结构化判断而不是生成一段话」，
  以及 `calibrated: false` 的正确解读方式。
- **提示词 `decide`**：对当前讨论的问题做一次类型化判定。
- **提示词 `gate`**：对当前 git diff 跑一次验收门禁。

注册点：`src/agent/tools/builtin-plugins/index.ts` 的 `BUILTIN_PLUGINS` 数组。
loader / skills manager / prompts manager 会自动接管工具注册与技能、提示词的归集，
**无需改这三个 manager**。

---

## 8. 文件布局与必改清单

### 8.1 新增文件

```
src/agent/tools/builtin-plugins/decision/
├── index.ts      # decisionPlugin 包定义（tools + skills + prompts）
├── types.ts      # §4.1 的类型契约
├── config.ts     # §5 配置解析（config.json + env + secrets + 兼容旧变量名）
├── engine.ts     # 引擎解析 + 三个引擎实现（jev / local / heuristic）
├── designer.ts   # §6.2 的 extractJson / validateDesign / designDecision
├── gate.ts       # §6.3 的状态解析（diff/file/text）与判定
└── tools.ts      # 三个 AgentTool 实现

src/agent/tools/builtin-plugins/decision.test.ts   # §10 测试
```

### 8.2 必须同步修改的既有文件（漏一个就出问题）

| 文件 | 改动 | 为什么 |
|---|---|---|
| `builtin-plugins/index.ts` | 导入并加入 `BUILTIN_PLUGINS` | 否则插件不加载 |
| `tools/registry.ts` | `READ_ONLY` 加 `decide`/`design_decision`/`check_gate` | 否则只读子智能体与 plan 模式被 `isWriteTool` 挡掉 |
| `tools/registry.ts` | `BUILTIN_TOOLS_METADATA` 加三条 | 插件管理页要诚实展示 |
| `tools.ts` | `describeTool` 加三个 case | 工具卡头部摘要 |
| `subagents/builtins.ts` | 子智能体白名单补决策工具（见下） | **`AGENTS.md` 第 1 条**：白名单与插件注册表脱钩 |
| `subagents/builtins.test.ts` | 守门测试加决策工具的断言 | 防止再次漂移 |
| `README.md` | 内置插件清单加一行 + 决策插件小节 | 文档一致性 |

### 8.3 子智能体白名单分配

依据各角色实际需要，按最小授权给：

| 子智能体 | `decide` | `design_decision` | `check_gate` | 理由 |
|---|---|---|---|---|
| `researcher` | ✅ | — | — | 可做归属/分类判定 |
| `code_reviewer` | ✅ | ✅ | — | 严重度分级、自定义评分维度 |
| `tester` | ✅ | — | ✅ | 失败归因分类 + 收尾自检门禁 |
| `general_purpose` | ✅ | ✅ | ✅ | 通配 `*` 自动覆盖，无需改 |

三个工具全部**无工作区副作用**（只发模型请求 / 只读 `git diff`），
因此都进 `READ_ONLY`。注意与 `run_test_focused` 的区别：后者执行测试命令、
可能产生构建产物，所以**算写工具**——`check_gate` 不执行测试，只读 diff，是只读的。

### 8.4 可选：CLI 门禁（阶段二）

pi-jev 提供 `bin/jev-gate.js` 供 CI 与子智能体 `gate` 参数使用。
a_da 对应物可按既有的 `scripts/*-check.ts` 约定加 `scripts/decision-gate.ts`，
支持 `--diff` / `--file` / stdin 与 `0/1/2` 退出码，供 CI 使用。
v1 不做（a_da 子智能体没有 `gate` 参数，挂不进去）。

---

## 9. 明确不做的事，以及将来怎么做

| pi-jev 能力 | v1 | 将来怎么加 |
|---|---|---|
| 工具路由 `jev_find_tools` | ❌ | a_da 工具**始终全量下发**，没有「激活/未激活」。意义应改为**反向收窄**：用决策判定本轮真正需要的工具以省 token。需要改 `registry.getToolsForMode` + `store.turn` 的工具表构建，属于核心改动 |
| 技能发现 `jev_find_skill` | ❌ | a_da 已有 `Skill` 工具按名加载；语义发现可作为该工具的增强，不必新工具 |
| 自动模式（每轮跑一次） | ❌ | a_da 的扩展系统**无法拦截轮次开始**（`ExtensionContext` 只有 `registerTool` / `onEvent`）。需在 `store.turn` 加 `beforeTurn` 钩子，属于核心改动。且每轮一次决策 = 每轮额外 token，须明示 |
| 模型自动选择 | ❌ | a_da 的模型配置是单例 `ProviderConfig`，没有多模型池；需要先有多模型配置能力 |
| 工具调用守卫（防幻觉） | ❌ | 可在 `store.gate()`（已有 `beforeToolCall`）里挂决策判定。但会增加每次工具调用的延迟与开销，且误拦的代价高，需要独立评估 |
| 压缩时保留关键历史 | ❌ | a_da 的 `executeCompaction` 已经做得不错；可在 `selectCompactSelection` 里用决策给消息打分。属于优化，收益需实测 |
| 子智能体 RPC `agent: "jev"` | ❌ | a_da 的子智能体是真实进程内会话，不是 RPC；直接让子智能体调 `decide` 即可覆盖该用途 |

---

## 10. 测试计划

`decision.test.ts` 覆盖，重点在**边界与诚实性**而非happy path：

**引擎解析**
- `auto` 在只配了模型时选 `local`；只配了端点时选 `jev`；都没配时选 `heuristic`
- 环境变量优先于 `config.json`
- 兼容读取 `PI_JEV_BASE_URL` / `TYPESAFE_API_KEY`

**本地引擎数学**（用假模型响应驱动，不发真实请求）
- `noul`：3 次采样中 2 次 yes → 主值 ≈ 0.667（**投票占比**，不是自报均值）
- `choice`：票数分布正确；**平票时取 criteria 首个 key**（确定性打破平局）
- `score`：加权期望档位计算正确
- 采样超时 → 用已到的样本，不死等
- 解析失败 → 重试一次；仍失败则如实报错，**不猜值**

**设计器校验**（照搬 pi-jev 的防御规则，逐条钉住）
- `choice` 的 `criteria` 是数组 → 拒绝
- `score` 的 `criteria` 是空数组 → 拒绝
- `noul` 带 `criteria` → 拒绝
- 问题缺 `instructions` → 丢弃该问题
- 全部被丢弃 → 整体失败
- 带 ``` 围栏与前后散文的模型输出 → 仍能抽出 JSON
- 问题数超 `max_questions` → 截断到上限

**门禁**
- `passed` 随阈值变化正确翻转
- `source=file` 读文件；`source=text` 直接用文本；`source=diff` 调 git
- 引擎不可用 + `fail_open: false` → `passed: false`（fail-close）
- 引擎不可用 + `fail_open: true` → `passed: true`
- `state` 超大 → 截断且 `notes` 有说明

**诚实性（最重要的一组）**
- 无任何引擎可用时，`decide` 返回 `ok: false`，**绝不返回编造的概率**
- 本地引擎结果 `calibrated === false` 且 `notes` 含未校准说明
- 启发式结果 `calibrated === false`、`confidence === 0`

**a_da 集成守护**（对齐 `AGENTS.md` 的两条硬约定）
- 三个工具都被 `isWriteTool` 判为**只读**
- 子智能体实际工具表中，各角色的决策工具可见性与 §8.3 一致
- 三个工具都在主工具表里
- `describeTool` 对三个工具都给出非空摘要

---

## 11. 成本透明（对齐 pi-jev 的 Cost Clarity）

实现后需在 README 明确列出开销，避免用户意外消耗：

| 入口 | 模型请求数 |
|---|---|
| `decide`（本地引擎） | N = `samples`（默认 3） |
| `decide`（Jev 引擎） | 1 次 Jev 请求 |
| `design_decision` | 1 次设计请求 + N 次评估 |
| `check_gate`（本地引擎） | N 次 |
| `check_gate`（Jev 引擎） | 1 次 Jev 请求 |
| 启发式 | 0 |

默认按需触发，**不产生每轮固定开销**。

---

## 12. 实施顺序

1. `types.ts` + `config.ts`（配置与兼容读取，附测试）
2. `engine.ts`：先做 `heuristic`（永远可用，解锁端到端）→ 再做 `local` → 最后 `jev`
3. `tools.ts` 的 `decide`，接进 `registry` 与 `READ_ONLY`
4. `designer.ts` + `design_decision`
5. `gate.ts` + `check_gate`
6. `index.ts` 包定义 + skills/prompts
7. 同步 §8.2 的六个文件，跑白名单守门测试
8. README 补插件清单与成本表

每步跑 `bun run typecheck` 与 `bun test`（两个独立门），最后 `bun run build` 重建二进制。

---

## 13. 待确认问题

1. **插件 id 与工具名是否合适？**
   提议 id=`decision`；工具 `decide` / `design_decision` / `check_gate`。
2. **本地引擎默认采样数 3 是否可接受？** 每次 `decide` 就是 3 次模型请求。
   降到 1 更省，但会失去投票稳定性。
3. **`check_gate` 在启发式下默认 fail-close 是否合理？**
   这会表现为"没配引擎时门禁一律不通过"。
4. **是否需要 `scripts/decision-gate.ts` CLI？**（§8.4，为 CI 与未来的子智能体 gate）
5. **子智能体白名单分配（§8.3）是否符合你的预期？**
   尤其是要不要给 `researcher` 也开 `design_decision`。

---

## 14. 实施记录

**已完成**（按 §12 顺序）：`types` / `config` / `engine` / `designer` / `gate` / `tools` / `index`
七个文件全部落地，§8.2 的必改文件同步完毕（`BUILTIN_PLUGINS`、`READ_ONLY`、
`BUILTIN_TOOLS_METADATA`、`describeTool`、子智能体白名单、守门测试），README 与 `AGENTS.md`
补完。`typecheck` 干净；插件自身 **74 个测试全绿**（decision 55 + 子智能体守门 8 + 相关）。
§10 测试计划里的每一项都有对应用例，包括最重要的诚实性一组。

与设计的偏差：

- `parseNoulAnswer` 的中文解析比设计时预想的复杂。CJK 不能用 `\b` 单词边界
  （`\b是\b` 对汉字永不匹配），且「是否」是疑问词、其后常跟复述问题的「成立」。
  实现改为：把每个「是否」起到下一个标点为止视作**问句片段**，片段内的肯定/否定词不计入。
  这样「是否成立？我认为成立」仍能取到后半句的真表态。
- 门禁的「平均情况」`noul` 概率贴近阈值（±0.15）时，`decide` 会在 notes 里提示人工复核。

**待解决（阻塞验证，不阻塞功能）**：注册本插件会让 `bun test src/ui/` 成组测试退化
（`98 pass / 1 fail / 30s` → `81 pass / 18 fail / 110s`）。已定位到触发条件是
「存在一个新增的、未被去重的插件技能」，与技能内容/长度、工具数量、重依赖均无关
（逐项实测排除）；把技能名改成已存在的名字触发去重即可恢复。根因指向测试基建
（GPU 测试渲染器在多技能下被拖住），**不在插件本身**。详细证据与修的方向记在
`AGENTS.md` 的「已知问题 2」。在修好之前，往 `BUILTIN_PLUGINS` 加带技能的新插件都会踩到它。

