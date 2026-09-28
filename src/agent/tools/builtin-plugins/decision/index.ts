/**
 * 内置辅助插件：决策与判定 (decision)
 *
 * 给智能体补上「结构化判断」这个能力：需要的是判断而不是生成一段话时，
 * 产出可比、可设阈值、可累计的结构化结论。
 *
 * 三个引擎按 jev → local → heuristic 回退，且**绝不捏造确定性**：
 * 拿不到真实判断就失败，而不是编一个看起来合理的概率。
 */

import { createCheckGateTool, createDecideTool, createDesignDecisionTool } from './tools'
import type { BuiltinPluginPackage } from '../types'

export * from './types'
export { createDecideTool, createDesignDecisionTool, createCheckGateTool } from './tools'
export {
  readDecisionConfig,
  DEFAULT_DECISION_THRESHOLD,
  DEFAULT_GATE_THRESHOLD,
  DEFAULT_SAMPLES,
  type DecisionConfig,
} from './config'
export {
  HeuristicEngine,
  LocalEngine,
  JevEngine,
  resolveEngine,
  parseNoulAnswer,
  parseChoiceKey,
  type ResolvedEngine,
  type SampleRunner,
} from './engine'
export {
  designDecision,
  extractJson,
  validateDesign,
  MAX_DESIGNED_QUESTIONS,
  DESIGN_SYSTEM_PROMPT,
  type DesignOutcome,
  type DesignRunner,
} from './designer'
export { runGate, readGitDiff, resolveGateState, type GateOptions, type GateSource } from './gate'

export const decisionPlugin: BuiltinPluginPackage = {
  id: 'decision',
  name: '决策与判定 (decision)',
  description:
    '类型化决策（choice / noul / score）、自由提示词决策设计器与验收门禁。支持 Jev 兼容端点、本地模型自评与确定性兜底三级引擎，概率均标注是否已校准。',
  tools: [createDecideTool, createDesignDecisionTool, createCheckGateTool],
  skills: [
    {
      name: 'decision-discipline',
      description: '什么时候该用结构化判断而不是生成一段话，以及如何正确解读未校准的概率。',
      content: `---
name: decision-discipline
description: 什么时候该用结构化判断而不是生成一段话，以及如何正确解读未校准的概率。
whenToUse: 当需要分级、分类、验收判定、二选一取舍或按评分标准打分时使用。
---

# 结构化判断的使用法则

## 什么时候该用 decide 而不是让模型写一段评价

需要**判断**而不是**生成**时，用结构化决策：

- **分级**：缺陷严重度、风险等级、优先级
- **分类**：失败归因（路径不存在 / 语法 / 权限 / 真实逻辑失败）
- **验收**：这次改动是否满足某条验收标准
- **取舍**：两个方案选一个，并说明倾向强度
- **打分**：按既定评分标准给产出评分

好处是结论**可比、可设阈值、可累计**，而不是埋在散文里等下一轮重新解析。

反过来，需要解释、推演、给建议时，就该正常写文字——不要硬塞进 choice 里。

## 正确解读 calibrated 字段

每条答案都带 \`calibrated\`，这是最重要的字段：

- \`calibrated: true\`（Jev 引擎）：专用 System One 模型给出的概率，**可以当概率用**。
- \`calibrated: false\`（本地引擎 / 启发式）：**不能当概率用**。
  - 本地引擎的值是多次采样的**投票占比**，比模型自报值稳，但仍是启发式的。
  - 启发式的值是刻意保持中性的占位（confidence 为 0），**没有任何判断依据**。

看到 \`engine: heuristic\` 时，务必如实告诉用户「没有可用的决策引擎，这个结论不算数」，
不要说成「倾向于……」。这是本插件最重要的一条纪律。

## 阈值与复核

- 默认阈值 0.65；门禁默认 0.70（更严）。
- 概率落在阈值 ±0.15 内时工具会提示复核——**别把模糊当确定**，这种情况值得把判断依据
  摆给用户看，而不是直接下结论。

## 写法要点

- \`state\` 必须**自包含**：引擎看不到你的对话历史，不要写「上文那段代码」。
- \`noul\` 不要带 criteria；\`choice\` 的 criteria 是对象；\`score\` 的是数组且**最高档在前**。
- 问题要能**仅凭 state** 回答，否则拿到的概率没有意义。
`,
    },
  ],
  prompts: [
    {
      name: 'decide',
      description: '对当前讨论的问题做一次类型化判定',
      argumentHint: '[要判断什么]',
      content: `请针对 \${1:要判断的问题} 做一次类型化决策：

1. 先把需要判断的材料整理进 state（务必自包含，不要把「上文」之类的引用留给引擎）；
2. 选择合适的问题类型：分级/分类用 choice，是否判断用 noul，按标准打用 score；
3. 调用 decide，并在给出结论时**同时说明 engine 与 calibrated**——
   如果是本地自评或启发式，明确告诉用户这个概率未经校准。`,
    },
    {
      name: 'gate',
      description: '对当前 git 改动跑一次验收门禁',
      argumentHint: '[验收标准]',
      content: `请以「\${1:验收标准}」为验收标准，对当前工作区的未提交改动跑一次门禁：

调用 check_gate（source 用默认的 diff）。拿到结果后：
- 若通过，说明概率与引擎；
- 若未通过，指出概率落点，并结合具体 diff 说明可能的原因；
- 若引擎是 heuristic，直接说明「没有可用的决策引擎，此结果无判定依据」。`,
    },
  ],
}
