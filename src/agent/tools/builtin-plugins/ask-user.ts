/**
 * 内置插件：向用户提问（ask-user）
 *
 * 给智能体一个**向用户提问并等待回答**的能力。此前它只有一条与用户交互的通道——
 * 审批闸门——而那是"要不要执行"的是非题，不是"你需要什么"的开放问答。
 *
 * ## 为什么值得单独做
 *
 * 编码任务里最常见的返工不是做错了，而是**做之前没问清楚**：改哪个模块、保留还是
 * 替换旧接口、兼容到什么程度。模型在没有信息时会自行假设，然后一路假设到底。
 * 给它一个能停下来问的通道，比让它猜完再改要省得多。
 *
 * ## 边界
 *
 * - **子智能体不能用**：并发派发时多个子智能体同时提问，用户不知道在回答谁。
 *   它们在 store 层被如实拒绝，并被告知改用 `notify_parent` 把问题交给主智能体。
 * - **不设超时**：与审批一致——用户没答就是没答；会话中止会收尾，不会留下悬挂的
 *   promise。要让运行停下来，用停止按钮。
 * - **只读**：它不碰工作区，登记在 `READ_ONLY` 里，plan 模式与只读子智能体都能用
 *   （计划阶段正是最需要澄清的时候）。
 */

import type { AgentTool, AgentToolResult } from '../../core/types'
import type { AgentQuestion } from '../../types'
import type { PluginDescriptor } from './types'

interface AskUserArgs {
  question?: unknown
  choices?: unknown
  allow_text?: unknown
}

/** 选项上限：选项一多就变成菜单而不是提问，用户反而更难答。 */
const MAX_CHOICES = 6
/** 问题长度上限：一段话该用普通消息说，不是塞进问题里。 */
const MAX_QUESTION_CHARS = 600

interface NormalizedChoice {
  id: string
  label: string
  description?: string
}

/**
 * 把模型给的选项规整成受控形状。
 *
 * 收到的是**模型生成的 JSON**，必须严格校验（与决策插件的 `validateDesign` 同源理由）：
 * 畸形输入不能进到界面里让用户面对一个点不动的按钮。
 * 返回 `undefined` 表示"没有可用选项"，此时强制允许自由输入。
 */
function normalizeChoices(raw: unknown): NormalizedChoice[] | undefined {
  if (!Array.isArray(raw)) return undefined
  const seen = new Set<string>()
  const choices: NormalizedChoice[] = []
  for (const item of raw.slice(0, MAX_CHOICES)) {
    if (typeof item !== 'object' || item === null) continue
    const record = item as Record<string, unknown>
    const label = typeof record.label === 'string' ? record.label.trim() : ''
    if (!label) continue
    // 没给 id 就用序号：id 是回传用的标识，不该让模型为它操心
    const id = typeof record.id === 'string' && record.id.trim() ? record.id.trim() : `c${choices.length + 1}`
    if (seen.has(id)) continue
    seen.add(id)
    const description = typeof record.description === 'string' ? record.description.trim() : ''
    choices.push({ id, label, ...(description ? { description } : {}) })
  }
  return choices.length > 0 ? choices : undefined
}

/** 把答案渲染成给模型看的一句话（模型据此继续，措辞要无歧义）。 */
function describeAnswer(
  answer: { answeredBy: 'user' | 'aborted'; choice?: string; text?: string },
  choices: NormalizedChoice[] | undefined
): string {
  if (answer.answeredBy === 'aborted') {
    return '用户中止了这次运行，问题没有得到回答。不要再假设一个答案继续做——把需要确认的点说清楚，等用户回来再问一次。'
  }
  const picked = answer.choice ? choices?.find((choice) => choice.id === answer.choice) : undefined
  if (picked && answer.text) return `用户选择了「${picked.label}」，并补充：${answer.text}`
  if (picked) return `用户选择了「${picked.label}」`
  if (answer.text) return `用户回答：${answer.text}`
  return '用户没有给出内容就提交了。请把问题问得更具体一些。'
}

export function createAskUserTool(): AgentTool<AskUserArgs, { question: AgentQuestion }> {
  return {
    name: 'ask_user',
    label: '向用户提问',
    description:
      '任务中途向用户提问并等待回答。需要用户拍板才能继续时用它：在多个方案间取舍、确认要改哪个模块、补齐缺失的需求信息。' +
      '给 choices 就是选择题（用户点一下即可），不给就是自由问答。' +
      '不要用它问"我可以继续吗"这类空泛的确认——那是在推卸判断；只在答案会实质改变你接下来做什么时才问。',
    parameters: {
      type: 'object',
      properties: {
        question: {
          type: 'string',
          description: '要问用户的问题。把背景与影响说清楚，让用户不必回看历史就能答。',
        },
        choices: {
          type: 'array',
          description:
            '可选。给 2–6 个选项让用户点选（每个 { label, description?, id? }）。不给则用户自由输入。',
          items: {
            type: 'object',
            properties: {
              id: { type: 'string', description: '选项标识，不传则自动编号' },
              label: { type: 'string', description: '选项短标签（必填）' },
              description: { type: 'string', description: '选项的补充说明' },
            },
            required: ['label'],
          },
        },
        allow_text: {
          type: 'boolean',
          description: '有选项时是否仍允许用户自由输入补充。默认 true。',
        },
      },
      required: ['question'],
    },

    async execute(callId, args, signal): Promise<AgentToolResult<{ question: AgentQuestion }>> {
      const question = typeof args?.question === 'string' ? args.question.trim() : ''
      if (!question) {
        return { output: '缺少 question 参数：要问用户什么？', ok: false }
      }
      if (question.length > MAX_QUESTION_CHARS) {
        return {
          output: `问题太长（${question.length} 字，上限 ${MAX_QUESTION_CHARS}）。把背景压缩成几句，或改用普通消息说明后再问。`,
          ok: false,
        }
      }

      const choices = normalizeChoices(args.choices)
      if (Array.isArray(args?.choices) && args.choices.length > 0 && !choices) {
        return {
          output: 'choices 里没有可用的选项（每项都需要非空的 label）。去掉 choices 即可改为自由问答。',
          ok: false,
        }
      }
      // 没有可用选项时必须允许自由输入，否则用户无从作答
      const allowText = choices ? args?.allow_text !== false : true

      // 工具内部动态 import 的是模块级 store 单例（生产环境就是它）
      const { store } = await import('../../store')

      let answer: { answeredBy: 'user' | 'aborted'; choice?: string; text?: string }
      try {
        answer = await store.requestUserAnswer({ callId, question, choices, allowText, signal })
      } catch (error) {
        // 子智能体路径在这里：如实说明并给出正确做法，而不是让它以为"问过了没回答"
        return { output: (error as Error).message, ok: false }
      }

      const record: AgentQuestion = {
        question,
        choices,
        allowText,
        status: answer.answeredBy === 'user' ? 'answered' : 'aborted',
        askedAt: Date.now(),
        answer,
      }

      return {
        output: describeAnswer(answer, choices),
        // 中止时标 isError：这一轮的结果不是"用户答了"，模型不该当成有效输入
        ok: answer.answeredBy === 'user',
        details: { question: record },
      }
    },
  }
}

export const askUserPlugin: PluginDescriptor = {
  id: 'ask-user',
  name: '向用户提问 (ask-user)',
  description:
    '让智能体在任务中途向用户提问并等待回答：在方案间取舍、确认改动范围、补齐需求信息。可选择项，也可自由作答。',
  tools: [createAskUserTool],
  skills: [
    {
      name: 'asking-discipline',
      description: '什么时候该停下来问用户、怎么问，以及为什么"能自己查就别问"。',
      content: `---
name: asking-discipline
description: 什么时候该停下来问用户、怎么问，以及为什么"能自己查就别问"。
whenToUse: 当需要用户拍板、或准备用 ask_user 提问时使用。
---

# 提问的纪律

## 先分清三种"不知道"

1. **查得到**：代码库里有答案（改哪个文件、现有接口长什么样）→ **自己查**，
   不要问用户。用户在等你干活，不是等你提问。
2. **查不到但影响大**：答案会实质改变你接下来做什么（改哪个模块、要不要兼容旧接口、
   破坏性迁移能否接受）→ **该问**。
3. **查不到但影响小**：选哪个都行、事后能改（变量命名、日志措辞）→ **自己定**，
   在结论里说一句你的选择即可。

只有第 2 种值得打断用户。

## 不要问的几种

- **"我可以继续吗？"**——这是在推卸判断。要么继续，要么把风险说清楚。
- **"你希望怎么做？"** 而不给背景——用户得先回看历史才知道你在纠结什么。
- **拿自己查得到的东西问**——先 search/read 一轮再决定要不要问。

## 怎么问

- **带上背景与影响**：让用户不必回看历史就能答。
- **能选就别用问答题**：给出 2–6 个具体选项，每个说清代价（"先补测试再重构" vs
  "一次性重构"），用户点一下就好。
- **一次问全**：同一个决策点相关的几件事一次问完，不要分三轮打断。
- **拿到答案就照做**：不要因为用户的回答与你的倾向不同就再问一遍。
`,
    },
  ],
  prompts: [
    {
      name: 'ask-first',
      description: '先向我确认关键前提，再动手，避免做完才发现方向不对。',
      argumentHint: '[要做的任务]',
      content: `在动手之前，先用 ask_user 向我确认这次任务里**会影响做法**的关键前提。

要求：
- 只问那些答案会实质改变你接下来做什么的问题（改哪个模块、要不要兼容旧接口、能否接受破坏性变更、优先正确性还是最小改动）。
- 每个问题给出 2–6 个具体选项，把代价写进选项里，让我点一下就够。
- 一次问完，不要分多轮打断。
- 我能自己查到的（代码结构、现有实现）说明你已经查过并给出你的判断，不要拿它们来问我。

任务：$1`,
    },
  ],
}
