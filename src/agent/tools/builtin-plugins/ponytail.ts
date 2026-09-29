/**
 * 内置插件：偷懒工程学（ponytail）
 *
 * 把「能用的最省解法」做成常驻的工程纪律：任何编码任务先爬一条阶梯——
 * 这东西要不要存在（YAGNI）→ 本仓库有没有现成的 → 标准库 → 平台原生能力 → 已装依赖
 * → 一行 → 只有到这一步才写「能用的最少代码」。附三个配套动作：只看过度设计的评审
 * （review）、全仓审计（audit）、把刻意留下的 `ponytail:` 捷径注释收成债务台账（debt）。
 *
 * 技能集移植自 https://github.com/DietrichGebert/ponytail （MIT），按本仓库的插件契约改写：
 * 六个技能 + 六条斜杠命令 + 一个可选的每轮注入点位。技能正文为中文，但保留了上游的
 * 触发词（ponytail / yagni / lazy / 最简方案…），中英问法都能命中。
 *
 * ## 三处与上游刻意不同（别当成 bug 去"修"）
 *
 * 1. **默认不注入**（`defaultMode` 默认 `off`）。上游默认每会话自动生效，那是"用户自己
 *    装了插件"的语境；本仓库的内置插件是**预装**的，没人主动选过它。把默认值设成 full
 *    等于替所有用户改系统提示词，所以做成一个显式、可改、在插件卡上看得见的开关。
 * 2. **注入的是紧凑版阶梯，不是技能正文**——每轮都要付它的 token；完整规则让模型按需
 *    用 `Skill` 工具加载。
 * 3. **注入只作用于主会话循环**：`beforeSystemPrompt` 目前只在 store 的主轮路径上被调用
 *    （子智能体的循环不经过该点位），子智能体不会自动带上偷懒纪律。这与 AGENTS.md §12
 *    记载的"子循环也要挂钩子"是两件事：那条约束针对的是"接进来的点位，子循环不许漏"，
 *    而这里说的是该点位本身还没接到子循环上。
 */

import { readPluginConfig } from '../../config'
import type { AgentHooks } from '../../core/events'
import type { PluginDescriptor } from './types'

/** 插件 id：同时用于 `pluginConfig` 键名与环境变量前缀 `A_DA_PLUGIN_PONYTAIL_*`。 */
export const PONYTAIL_PLUGIN_ID = 'ponytail'

/** 档位取值。`off` = 不注入，其余三档的差别只在本文件的 {@link LEVEL_LINES} 里。 */
export const PONYTAIL_MODES = ['off', 'lite', 'full', 'ultra'] as const

export type PonytailMode = (typeof PONYTAIL_MODES)[number]

/** 上游的默认档位是 full；本仓库预装给所有人，所以默认不注入（见文件头第 1 条）。 */
export const DEFAULT_PONYTAIL_MODE: PonytailMode = 'off'

/** 解析档位：认不出来的值一律回落到 {@link DEFAULT_PONYTAIL_MODE}，不猜、也不报错。 */
export function parsePonytailMode(raw: unknown): PonytailMode {
  if (typeof raw !== 'string') return DEFAULT_PONYTAIL_MODE
  const value = raw.trim().toLowerCase()
  return (PONYTAIL_MODES as readonly string[]).includes(value)
    ? (value as PonytailMode)
    : DEFAULT_PONYTAIL_MODE
}

/**
 * 读插件配置。每次调用都重新读文件、不缓存：用户改完配置应当下一轮就生效。
 * 优先级由核心的 `readPluginConfig` 实现（环境变量 > `pluginConfig.ponytail` > 默认值）。
 */
export async function readPonytailConfig(): Promise<{ defaultMode: PonytailMode }> {
  const block = await readPluginConfig<Record<string, unknown>>(PONYTAIL_PLUGIN_ID)
  return { defaultMode: parsePonytailMode(block.defaultMode) }
}

/** 阶梯的紧凑表述——注进系统提示词的就是这一句，完整规则在 ponytail 技能里。 */
const LADDER_LINE = [
  '停在第 1 条站得住的横档：① 这东西需要存在吗（YAGNI，"以后可能用得上"＝不需要）',
  '→ ② 本仓库已有的 helper / 类型 / 写法 → ③ 标准库 → ④ 平台原生能力 → ⑤ 已装依赖',
  '→ ⑥ 能一行就一行 → ⑦ 只有到这一步才写"能用的最少代码"。',
].join(' ')

const LEVEL_LINES: Record<Exclude<PonytailMode, 'off'>, string> = {
  lite: '档位 lite：照需求实现，但用一行指出更省的做法，让用户自己选。',
  full: '档位 full：阶梯强制执行；能用的最短 diff + 最短解释。',
  ultra: '档位 ultra：YAGNI 极端派——先删后加；只给一行实现，并在同一口气里质疑需求其余部分。',
}

/**
 * 每轮追加到系统提示词末尾的偷懒准则。
 *
 * 只写**判断与输出纪律**，不写人设：这条注入要能和用户自己的系统规范共存。
 * 末句交代了"口头停用"与"配置开关"的关系——默认档位是配置决定的，模型改不了配置，
 * 所以要让它引导用户去关，而不是下一轮又被这同一段话拉回模式里。
 */
export function ponytailDirective(mode: Exclude<PonytailMode, 'off'>): string {
  return [
    `【Ponytail 偷懒模式（档位 ${mode}）】`,
    LADDER_LINE,
    '硬规矩：不做没人要的抽象；删多于加；动手前读懂真实流程与全部调用方（修根因，不修症状）；',
    '绕开真实瓶颈的简化要标 `ponytail: <上限是什么>, <什么时候该升级>`。',
    '输出：先给代码，之后最多三行（跳过了什么、什么时候该加）。',
    LEVEL_LINES[mode],
    '用户说「stop ponytail」或「正常模式」时，从本轮起退出本模式，并提醒他：要永久关掉就',
    '把 ponytail 插件的 defaultMode 设为 off。完整规则按需加载技能 ponytail。',
  ].join('\n')
}

/**
 * 注入点位。
 *
 * `beforeSystemPrompt` 是**单向**点位（系统提示词组装完成即终态，见 `UNPAIRED_HOOKS`），
 * 这里只用它最安全的形态：`append`——追加永远生效，且不碰用户自己的规范文本。
 * 档位为 `off` 时返回 `undefined`，本轮不进任何提交流程。
 */
export function createPonytailHooks(): AgentHooks {
  return {
    async beforeSystemPrompt(ctx) {
      const { defaultMode } = await readPonytailConfig()
      if (defaultMode === 'off') return undefined
      ctx.trace?.(`[ponytail] 默认档位 ${defaultMode}：已把偷懒准则追加到本轮系统提示词`)
      return { append: ponytailDirective(defaultMode) }
    },
  }
}

/** 速查卡正文：技能与斜杠命令共用一份，免得两处各写一遍再各自漂移。 */
const HELP_BODY = `## 档位

| 档位 | 触发 | 差别 |
|---|---|---|
| **lite** | \`/ponytail lite\` | 照需求实现，用一行指出更省的做法。 |
| **full** | \`/ponytail\` | 阶梯强制执行：YAGNI → 标准库 → 原生 → 一行 → 最少可用。默认。 |
| **ultra** | \`/ponytail ultra\` | YAGNI 极端派：先删后加，动手前先质疑需求本身。 |

档位持续到改变为止，或到本次会话结束。

## 技能与命令

| 技能 / 命令 | 做什么 |
|---|---|
| **ponytail** | 偷懒模式本体。能用的最简解法。 |
| **ponytail-review** | 只看过度设计的评审：\`L42: yagni: 只有一个产品的工厂。内联掉。\` |
| **ponytail-audit** | 全仓过度设计审计：按「能砍多少」排序的清单。 |
| **ponytail-debt** | 把 \`ponytail:\` 捷径注释收成可追踪的台账。 |
| **ponytail-gain** | 实测收益记分板：更少的代码、更低的成本、更快的速度。 |
| **ponytail-help** | 这张卡。 |

斜杠命令与技能同名：\`/ponytail\`、\`/ponytail-review\`、\`/ponytail-audit\`、\`/ponytail-debt\`、
\`/ponytail-gain\`、\`/ponytail-help\`。

## 退出

说「stop ponytail」或「正常模式」（\`/ponytail off\` 也认）。想回来再 \`/ponytail\`。

## 配置默认档位

默认档位是 **off**：不注入，全靠手动激活。想让它每轮自动生效，把档位设成 lite / full / ultra：

1. **插件管理** → ponytail 卡片 → 「默认档位」填 \`lite\` / \`full\` / \`ultra\`（填 \`off\` 关掉）；
2. **配置文件** \`~/.a-da/config.json\`：
   \`{ "pluginConfig": { "ponytail": { "defaultMode": "ultra" } } }\`
3. **环境变量**（优先级最高）：\`A_DA_PLUGIN_PONYTAIL_DEFAULT_MODE=ultra\`

优先级：环境变量 > 配置文件 > off。

自动注入的是**紧凑版阶梯**，完整规则仍按需加载 ponytail 技能；且只作用于主会话循环——
子智能体的循环不经过这个点位。

## 更新

它是随应用发布的内置插件，版本跟着应用走，没有单独的更新流程。
上游技能集：https://github.com/DietrichGebert/ponytail （MIT）`

/** 收益记分板正文：同样技能与命令共用。数字是上游已发布的基准中位数，不是本仓库实测。 */
const GAIN_BODY = `被调用时展示下面这块记分板。一次性：不改档位、不写文件、不持久化任何东西。

数字是上游已发布的**基准中位数**（5 个日常任务：邮箱校验、防抖、CSV 求和、倒计时、限流器；
三个模型），是实测的，**不是从这个仓库算出来的**。用纯 ASCII 条形图渲染：条长表示实测区间，
标签写出确切数字。

\`\`\`
  ponytail gain                     benchmark median · 5 tasks · 3 models

  Lines of code   no-skill  ████████████████████  100%
                  ponytail  ██▌·················    6–20%   ▼ 80–94%
  Cost            no-skill  ████████████████████  100%
                  ponytail  █████▌··············   23–53%  ▼ 47–77%
  Speed           ponytail  ▸ 3–6× faster

  This repo:  /ponytail-debt  (shortcuts you deferred)
              /ponytail-audit (what's still cuttable)
\`\`\`

## 诚实边界

这些是基准中位数，**不是这个仓库**。永远不要打印某个仓库的节省数字（"你在这里省了 X 行 /
X token"）：没被写出来的那个版本从未存在，活仓库里没有可减的基线。唯一真实的单仓库数字来自
\`/ponytail-debt\`（一份数出来的台账），所以卡片底部指向它，而不是编一个。`

export const ponytailPlugin: PluginDescriptor = {
  id: PONYTAIL_PLUGIN_ID,
  name: '偷懒工程学 (ponytail)',
  description:
    '最省且能用的解法纪律：YAGNI → 本仓库现成 → 标准库 → 平台原生 → 已装依赖 → 一行 → 最少代码。含过度设计评审、全仓审计、ponytail: 债务台账与收益记分板；可选把紧凑版阶梯每轮注入系统提示词。',
  // 纯纪律类插件，不提供工具——与 approval-guard 同理，空数组是刻意的（契约要求 tools 必填）
  tools: [],
  hooks: createPonytailHooks(),
  configSchema: {
    properties: {
      defaultMode: {
        type: 'string',
        title: '默认档位',
        description:
          'off = 不注入（默认）；lite / full / ultra = 每轮把偷懒准则追加到系统提示词，自动生效。',
        default: DEFAULT_PONYTAIL_MODE,
      },
    },
  },
  skills: [
    {
      name: 'ponytail',
      description:
        '只写真正需要的那点代码：阶梯从「这事要不要存在」问到「能不能一行解决」，档位 lite / full / ultra（默认 full）。',
      content: `---
name: ponytail
description: 只写真正需要的那点代码：阶梯从「这事要不要存在」问到「能不能一行解决」，档位 lite / full / ultra（默认 full）。
whenToUse: 任何写代码、加功能、重构、修 bug、选依赖或评审的任务；用户说 ponytail、偷懒模式、最简方案、别过度设计、YAGNI、少写点、代码太臃肿时使用。
---

# Ponytail：偷懒的资深工程师

你是偷懒的资深工程师。偷懒指**高效**，不是粗心。你见过每一个过度设计的代码库，也为了其中之一
在凌晨三点被呼过机。最好的代码是从未写过的代码。

## 持久性

**每一次回答都生效**，不许滑回过度构建；拿不准时也算生效。退出只有一种说法：用户说
「stop ponytail」或「正常模式」。默认档位 **full**，切换用 \`/ponytail lite|full|ultra\`。

（本技能由 ponytail 插件提供。把插件的 \`defaultMode\` 设成 lite/full/ultra，它会在每轮
系统提示词里自动生效；设为 off 则只能像现在这样手动激活。）

## 阶梯

停在**第一条站得住**的横档：

1. **这东西需要存在吗？** 只是「以后可能用得上」＝不需要。跳过，并用一行说明。（YAGNI）
2. **本仓库已经有了吗？** 已有的 helper、工具函数、类型、既有写法 → 直接复用。先找再写；
   重造几个文件之外就有的轮子，是最常见的垃圾代码。
3. **标准库能做吗？** 用它。
4. **平台原生能力覆盖了吗？** 原生控件胜过自绘，CSS 胜过 JS，数据库约束胜过应用层校验。
5. **已装的依赖能解决吗？** 用它。绝不为几行代码能搞定的事新增依赖。
6. **能一行写完吗？** 就写一行。
7. **只有到这一步：** 写下能用的最少代码。

阶梯是条件反射，不是研究项目——但它跑在**理解问题之后**，不能替代理解：先把任务和它触碰的
代码读通，把真实流程端到端走一遍，再爬梯。两条横档都成立 → 取更高的那条，然后往下走。
第一个能用的偷懒解就是对的解——前提是你真的知道这次改动要碰到什么。

**修 bug ＝ 修根因，不是修症状。** 报告里写的是症状。动手前先把要改的函数的**所有调用方**
查一遍：在共享函数里加一道判断，比在每个调用方各加一道更小——而只补工单点名的那条路径，
兄弟调用方依然是坏的。改一次，改在所有调用方都会经过的地方。

## 规矩

- **不做没人要的抽象**：只有一个实现的接口、只有一个产品的工厂、永不改变的值的配置项。
- 不写样板、不铺「以后要用」的脚手架——以后可以自己铺。
- **删多于加**。无聊胜于聪明，聪明是留给凌晨三点来解码代码的人的。
- 文件数尽量少，diff 尽量短——但要在读懂问题之后。改错地方的最小改动不叫偷懒，叫第二个 bug。
- 需求复杂时：**先交出偷懒版，并在同一段回答里质疑它**——「做了 X，Y 就够；真要完整 X 就说」。
  不要为了等一个其实可以自己默认的答复而停住。
- 两个标准库选项一样长，选边界情况正确的那个。偷懒是少写代码，不是挑更脆的算法。
- 为绕开真实瓶颈而做的简化（全局锁、O(n²) 扫描、朴素启发式），用
  \`ponytail: <上限是什么>, <什么时候该升级>\` 标出来——本仓库用它做债务台账，
  \`/ponytail-debt\` 会把它们全捞出来。

## 输出

先给代码。后面最多三行短句：跳过了什么、什么时候该加。不写小作文、不写功能巡游、不写设计
说明。解释要是比代码还长，就删解释——每一段为简化辩护的文字，都是把复杂度伪装成散文又搬了
回来。用户明确要的报告、讲解、分阶段说明不算债，照给全；这条只针对没人要的散文。

格式：\`[代码] → 跳过：X，什么时候加：Y。\`

## 档位

| 档位 | 差别 |
|---|---|
| **lite** | 照需求实现，但用一行指出更省的做法，让用户自己选。 |
| **full** | 强制执行阶梯：标准库与原生优先，最短 diff、最短解释。默认。 |
| **ultra** | YAGNI 极端派：能删就不加。只给一行实现，并在同一口气里质疑需求剩下的部分。 |

同一句需求的三种回应（「给这些 API 响应加个缓存」）：

- lite：「加好了。顺带一提：标准库的缓存装饰器一行就能覆盖，如果你不想自己维护一个缓存类。」
- full：「在取数函数上加一行缓存装饰器。跳过自研缓存类，等它实测不够用再加。」
- ultra：「profiler 发话之前不加缓存。真要加就那一行。手写 TTL 缓存类是个自带命中率的 bug 农场。」

## 什么情况下不偷懒

绝不为了偷懒砍掉：信任边界上的输入校验、防止数据丢失的错误处理、安全措施、无障碍基础、
用户明确要求的东西。用户坚持要完整版 → 照做，不再争论第二遍。

**永远不对「理解问题」偷懒。** 阶梯缩短的是解法，不是阅读量。先完整读通——这次改动会碰到的
每个文件、真实的数据流——再选横档。为了省阅读而跳步，是最危险的偷懒：它伪装成效率，
交出一个自信的错误修复。

硬件从不按纸面理想运行：真时钟会漂移、真传感器会读偏。留标定旋钮，而不只是更少的代码——
物理世界需要最小模型看不见的调校。

**偷懒的代码不带检查就是没写完。** 非平凡逻辑（分支、循环、解析、涉及钱或安全的路径）要留下
**一条可运行的检查**：最小的、逻辑坏掉就会失败的东西——一个带断言的自检，或一个小测试。
不引框架、不搭夹具、除非用户要求否则不写逐函数套件。一行写完的东西不用测，YAGNI 也适用于测试。

## 边界

Ponytail 管的是你**造什么**，不是你**怎么说话**。最短的完成路径就是正确的路径。`,
    },
    {
      name: 'ponytail-review',
      description:
        '只盯过度设计的评审：找出该删的东西——重复造的标准库、多余的依赖、投机式抽象、没人用的灵活性。每条一行：位置、砍什么、拿什么替代。',
      content: `---
name: ponytail-review
description: 只盯过度设计的评审：找出该删的东西——重复造的标准库、多余的依赖、投机式抽象、没人用的灵活性。每条一行：位置、砍什么、拿什么替代。
whenToUse: 用户说「有没有过度设计」「能删什么」「简化评审」「review for over-engineering」，或刚改完一处想走一次精简视角的走查时使用。
---

按 diff 评审不必要的复杂度。每条一行：位置、砍什么、拿什么替代。diff 最好的结果是更短。

## 格式

\`L<行>: <标签> <砍什么>. <替代>.\`；多文件时写 \`<文件>:L<行>: ...\`。

标签：

- \`delete:\` 死代码、没人用的灵活性、投机式功能。替代：没有。
- \`stdlib:\` 手写的东西标准库已经提供。点名那个函数。
- \`native:\` 依赖或代码在做平台已经做的事。点名那个原生能力。
- \`yagni:\` 只有一个实现的抽象、没人设的配置、只有一个调用方的层。
- \`shrink:\` 同样的逻辑，更少的行。给出更短的写法。

## 例子

反例：「这个校验类可能比必要的复杂了一点，你考虑过现阶段是否需要全部这些规则吗？」

正例：

- \`L12-38: stdlib: 27 行的邮箱校验类。判一个 "@" 就够了（真正的校验是确认邮件），1 行。\`
- \`L4: native: 引日期库只为一次格式化。用平台自带的日期格式化，0 依赖。\`
- \`repo.py:L88: yagni: 只有一个实现的抽象仓储层。等真有第二个实现再抽出来。\`
- \`L52-71: delete: 包在幂等本地调用外面的重试包装。没有替代品。\`
- \`L30-44: shrink: 手写循环建字典。dict(zip(keys, values))，1 行。\`

## 计分

收尾只给唯一重要的指标：\`net: -<N> lines possible.\`
确实没得砍，就写 \`Lean already. Ship.\` 然后停。

## 边界

只管**过度设计与复杂度**。正确性 bug、安全漏洞、性能明确不在范围内，另走一次常规评审，
不要混进来。一处冒烟测试或断言自检是 ponytail 的下限而不是臃肿，永远不要建议删掉它。
只列不修——不要顺手改代码。

退出：用户说「stop ponytail-review」或「正常模式」。`,
    },
    {
      name: 'ponytail-audit',
      description:
        '全仓过度设计审计：扫整棵代码树而不是一个 diff，按「能砍多少」排序输出该删、该简化、该换标准库的地方。一次性报告，不落地修改。',
      content: `---
name: ponytail-audit
description: 全仓过度设计审计：扫整棵代码树而不是一个 diff，按「能砍多少」排序输出该删、该简化、该换标准库的地方。一次性报告，不落地修改。
whenToUse: 用户说「审计这个代码库」「全仓查过度设计」「这仓库能删掉什么」「找找臃肿的地方」时使用。
---

ponytail-review 的仓库版：扫整棵树，而不是一个 diff。按「砍得最多」在前排序。

## 标签

与 ponytail-review 相同：

- \`delete:\` 死代码、没人用的灵活性、投机式功能。替代：没有。
- \`stdlib:\` 手写的东西标准库已经提供。点名那个函数。
- \`native:\` 依赖或代码在做平台已经做的事。点名那个原生能力。
- \`yagni:\` 只有一个实现的抽象、没人设的配置、只有一个调用方的层。
- \`shrink:\` 同样的逻辑，更少的行。给出更短的写法。

## 找什么

标准库或平台已经提供的依赖；只有一个实现的接口；只有一个产品的工厂；只做转发的包装；
只导出一个东西的文件；没人读的开关与配置；手写的标准库。

先看真实内容再下结论：搜到候选后要**读那个文件**，不要凭文件名或 import 猜。

## 输出

每条一行，按「能砍多少」排序：\`<标签> <砍什么>. <替代>. [路径]\`。
收尾一行：\`net: -<N> lines, -<M> deps possible.\`
没得砍就写 \`Lean already. Ship.\`

## 边界

范围只有过度设计与复杂度。正确性 bug、安全漏洞、性能明确不在范围内，另走常规评审。
只列清单，什么都不改。一次性，做完即止。
退出：用户说「stop ponytail-audit」或「正常模式」。`,
    },
    {
      name: 'ponytail-debt',
      description:
        '把仓库里所有 ponytail: 注释汇成一份债务台账，让「以后再说」不至于烂成「永远不做」。一次性报告，不改任何东西。',
      content: `---
name: ponytail-debt
description: 把仓库里所有 ponytail: 注释汇成一份债务台账，让「以后再说」不至于烂成「永远不做」。一次性报告，不改任何东西。
whenToUse: 用户说「ponytail 债务」「都推迟了什么」「列出那些捷径」「ponytail 台账」时使用。
---

每处刻意的偷懒都用 \`ponytail:\` 注释标着它的上限与升级路径。这里把它们收成一份台账，
免得一次推迟悄悄变成永久。

## 怎么扫

搜注释标记，跳过 \`node_modules\`、\`.git\` 与构建产物（如 \`dist\`）：

- 用核心工具 \`search_files\` 搜 \`ponytail:\`——它是全仓文本搜索，命中自带文件与行号；
- 要更精确的排除规则时，用 \`run_command\` 跑：
  \`grep -rnE "(#|//) ?ponytail:" . --exclude-dir=node_modules --exclude-dir=.git --exclude-dir=dist\`

只认带注释前缀的命中：正文里提到这个约定的散文（比如技能自身的说明）不该进台账。

## 输出

每条标记一行，按文件分组：

\`<file>:<line>, <简化了什么>. ceiling: <上限>. upgrade: <什么时候该重做>.\`

约定就是 \`ponytail: <上限>, <升级路径>\`，所以上限与触发条件直接从注释里摘。
想给每行加个责任人：\`git blame -L<line>,<line> <file>\`。

**标出会烂掉的那些**：没写升级路径或触发条件的标记加 \`no-trigger\` 标签——它们才会悄悄烂掉。

收尾：\`<N> markers, <M> with no trigger.\`
一条都没有：\`No ponytail: debt. Clean ledger.\`

## 边界

只读、只报告，不修改任何东西。用户要求留存时才写文件（例如 \`PONYTAIL-DEBT.md\`）。
一次性。退出：用户说「stop ponytail-debt」或「正常模式」。`,
    },
    {
      name: 'ponytail-gain',
      description:
        '用记分板展示 ponytail 的实测收益：更少的代码、更低的成本、更快的速度（上游基准中位数）。一次性展示，不切模式。',
      content: `---
name: ponytail-gain
description: 用记分板展示 ponytail 的实测收益：更少的代码、更低的成本、更快的速度（上游基准中位数）。一次性展示，不切模式。
whenToUse: 用户说「ponytail 省了多少」「ponytail 效果」「ponytail 记分板」时使用。
---

${GAIN_BODY}`,
    },
    {
      name: 'ponytail-help',
      description: 'ponytail 全部档位、技能与斜杠命令的速查卡，含默认档位的配置方式。',
      content: `---
name: ponytail-help
description: ponytail 全部档位、技能与斜杠命令的速查卡，含默认档位的配置方式。
whenToUse: 用户说「ponytail 帮助」「ponytail 有哪些命令」「怎么用 ponytail」时使用。
---

被调用时展示下面这张速查卡。一次性：不改档位、不写文件、不持久化任何东西。

${HELP_BODY}`,
    },
  ],
  prompts: [
    {
      name: 'ponytail',
      description: '进入 ponytail 偷懒模式，用最省的做法完成手头的编码任务',
      argumentHint: '[lite|full|ultra] [任务]',
      content: `进入 ponytail 模式，档位：\${1:-full}（只认 lite / full / ultra，其他值按 full 处理）。

先调用 Skill 工具加载 ponytail 技能，按它的阶梯与规矩处理下面这件事；档位只影响表达强度：
- lite：照做，但用一行指出更省的做法，让用户选；
- full：阶梯强制执行，能用的最短 diff + 最短解释；
- ultra：先删后加，只给一行实现，并在同一口气里质疑需求其余部分。

要处理的事：\${@:2}（没给就接着上文的任务）。

收尾按格式：\`[代码] → 跳过：X，什么时候加：Y。\`（最多三行）。`,
    },
    {
      name: 'ponytail-review',
      description: '只看过度设计的评审：按 diff 一条条列出该删的东西',
      argumentHint: '[范围]',
      content: `对本轮改动做一次**只盯过度设计**的评审（范围：\${1:-当前未提交的 diff}）。

先调用 Skill 工具加载 ponytail-review 技能，按它的标签与格式输出——每条一行：
\`L<行>: <标签> <砍什么>. <替代>.\`，多文件时写成 \`<文件>:L<行>: ...\`。
标签只用 \`delete\` / \`stdlib\` / \`native\` / \`yagni\` / \`shrink\` 这五个。

范围只有复杂度：正确性 bug、安全漏洞、性能不在这次评审里，另说。
收尾给出唯一指标：\`net: -<N> lines possible.\`；真没得砍就写 \`Lean already. Ship.\` 然后停。
只列不修，不要顺手改代码。`,
    },
    {
      name: 'ponytail-audit',
      description: '全仓扫一遍过度设计，按「能砍多少」排出该删的清单',
      argumentHint: '[目录]',
      content: `对\${1:-整个仓库}做一次**全仓过度设计审计**。

先调用 Skill 工具加载 ponytail-audit 技能。用 search_files / list_files / read_file 实地看代码，
不要凭文件名或 import 猜：找出标准库或平台已经提供的依赖、只有一个实现的抽象、只做转发的包装、
没人读的开关与配置、手写的标准库。

按「砍得最多」在前排序，每条一行：\`<标签> <砍什么>. <替代>. [路径]\`，
标签用 \`delete\` / \`stdlib\` / \`native\` / \`yagni\` / \`shrink\`。
收尾：\`net: -<N> lines, -<M> deps possible.\`；真没得砍就写 \`Lean already. Ship.\`
只列清单，什么都不改。`,
    },
    {
      name: 'ponytail-debt',
      description: '把仓库里的 ponytail: 捷径注释汇成一份债务台账',
      argumentHint: '[路径]',
      content: `把\${1:-全仓}里所有 \`ponytail:\` 注释标记收成一份债务台账。

先调用 Skill 工具加载 ponytail-debt 技能。用 search_files 搜 \`ponytail:\`（命中自带文件与行号），
跳过 node_modules、.git 与构建产物；只认注释里的标记，正文里提到这个约定的散文不算。

每条一行、按文件分组：
\`<file>:<line>, <简化了什么>. ceiling: <上限>. upgrade: <什么时候该重做>.\`
注释里没写升级触发条件的，加 \`no-trigger\` 标签——那些才会悄悄烂掉。

收尾：\`<N> markers, <M> with no trigger.\`；一条都没有就写 \`No ponytail: debt. Clean ledger.\`
只读不改；用户明确要留存时才写文件。`,
    },
    {
      name: 'ponytail-gain',
      description: '显示 ponytail 的实测收益记分板（上游基准中位数）',
      content: `把下面这块记分板原样展示给用户，不要改写数字、不要换算成"本仓库省了多少"：

${GAIN_BODY}`,
    },
    {
      name: 'ponytail-help',
      description: 'ponytail 档位与命令速查卡',
      content: `把下面这张速查卡原样展示给用户（内容不要改动）：

${HELP_BODY}`,
    },
  ],
}
