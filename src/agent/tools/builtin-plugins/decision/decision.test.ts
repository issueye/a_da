/**
 * 决策插件测试。
 *
 * 重点不在 happy path，而在三处最容易出错也最要紧的地方：
 *
 * 1. **投票数学**：本地引擎不采信模型自报概率，而用多样本投票占比。这套算术必须钉死，
 *    否则「校准」就成了嘴上说说。
 * 2. **不可信输入的校验**：设计器/工具收到的是模型生成的 JSON，畸形输入必须被挡住，
 *    而不是把半残 schema 放进去评估。
 * 3. **诚实性**：这是整个插件的立身之本——没有引擎时绝不编造概率；本地自评与启发式
 *    必须标注 calibrated=false；门禁在无引擎时默认 fail-close。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  DESIGN_SYSTEM_PROMPT,
  LocalEngine,
  HeuristicEngine,
  JevEngine,
  MAX_DESIGNED_QUESTIONS,
  createCheckGateTool,
  createDecideTool,
  designDecision,
  extractJson,
  parseChoiceKey,
  parseNoulAnswer,
  readDecisionConfig,
  resolveEngine,
  runGate,
  validateDesign,
  type DecisionConfig,
  type SampleRunner,
} from './index'
import { DEFAULT_DECISION_THRESHOLD, DEFAULT_GATE_THRESHOLD, MAX_STATE_CHARS } from './config'
import type { DecisionEngine, DecisionRequest } from './types'
import { defaultToolRegistry } from '../../registry'
import { BUILTIN_PLUGINS } from '..'

let home = ''
let oldHome: string | undefined

beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'ada-decision-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = home
  // 清掉可能影响解析的环境变量，测试自己按需设置
  delete process.env.A_DA_DECISION_ENGINE
  delete process.env.A_DA_DECISION_BASE_URL
  delete process.env.A_DA_DECISION_API_KEY
  delete process.env.PI_JEV_BASE_URL
  delete process.env.TYPESAFE_BASE_URL
  delete process.env.TYPESAFE_API_KEY
})

afterEach(async () => {
  delete process.env.A_DA_DECISION_ENGINE
  delete process.env.A_DA_DECISION_BASE_URL
  delete process.env.A_DA_DECISION_API_KEY
  delete process.env.PI_JEV_BASE_URL
  delete process.env.TYPESAFE_BASE_URL
  delete process.env.TYPESAFE_API_KEY
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  await rm(home, { recursive: true, force: true })
})

/** 造一个假采样器：按顺序返回预设文本，用光后重复最后一个。 */
function fakeRunner(outputs: string[]): SampleRunner {
  let index = 0
  return async () => {
    const text = outputs[Math.min(index, outputs.length - 1)] ?? ''
    index += 1
    return { text }
  }
}

const modelConfig = { baseUrl: 'http://localhost/v1', apiKey: 'k', model: 'test' }

// ---------------------------------------------------------------- 配置

describe('决策配置解析', () => {
  test('默认值：auto 引擎、0.65 阈值、3 次采样', async () => {
    const config = await readDecisionConfig()
    expect(config.engine).toBe('auto')
    expect(config.threshold).toBe(DEFAULT_DECISION_THRESHOLD)
    expect(config.samples).toBe(3)
    expect(config.baseUrl).toBe('')
  })

  test('环境变量优先于 config.json', async () => {
    await writeFile(
      join(home, 'config.json'),
      JSON.stringify({ decision: { engine: 'local', threshold: 0.9, baseUrl: 'http://from-file' } }),
      'utf8',
    )
    process.env.A_DA_DECISION_ENGINE = 'heuristic'
    process.env.A_DA_DECISION_BASE_URL = 'http://from-env'

    const config = await readDecisionConfig()
    expect(config.engine).toBe('heuristic')
    expect(config.baseUrl).toBe('http://from-env')
    // 阈值没被环境变量覆盖，仍取文件里的
    expect(config.threshold).toBe(0.9)
  })

  test('兼容 pi-jev 的旧变量名（PI_JEV_BASE_URL / TYPESAFE_*）', async () => {
    process.env.PI_JEV_BASE_URL = 'http://jev-local:8000'
    process.env.TYPESAFE_API_KEY = 'ts_from_old_env'
    const config = await readDecisionConfig()
    expect(config.baseUrl).toBe('http://jev-local:8000')
    expect(config.apiKey).toBe('ts_from_old_env')
  })

  test('A_DA_DECISION_BASE_URL 优先于 PI_JEV_BASE_URL', async () => {
    process.env.A_DA_DECISION_BASE_URL = 'http://new'
    process.env.PI_JEV_BASE_URL = 'http://old'
    expect((await readDecisionConfig()).baseUrl).toBe('http://new')
  })

  test('采样次数被夹在 1..9', async () => {
    await writeFile(join(home, 'config.json'), JSON.stringify({ decision: { samples: 99 } }), 'utf8')
    expect((await readDecisionConfig()).samples).toBe(9)
  })
})

// ---------------------------------------------------------------- 引擎解析

describe('引擎解析与回退', () => {
  const base: DecisionConfig = {
    engine: 'auto',
    baseUrl: '',
    apiKey: '',
    threshold: 0.65,
    samples: 1,
    sampleTimeoutMs: 1000,
  }

  test('都没配置时回退到 heuristic，并给出说明', async () => {
    const resolved = await resolveEngine(base)
    expect(resolved.engine.id).toBe('heuristic')
    expect(resolved.note).toContain('回退至确定性占位结果')
  })

  test('只配了 Jev 端点时选 jev', async () => {
    const resolved = await resolveEngine({ ...base, baseUrl: 'http://jev:8000' })
    expect(resolved.engine.id).toBe('jev')
  })

  test('显式指定 local 但没配模型时报错，而不是悄悄降级', async () => {
    // 用户明确要求某个引擎时，静默换掉比失败更糟
    await expect(resolveEngine({ ...base, engine: 'local' })).rejects.toThrow('未配置模型')
  })

  test('显式指定 jev 但没配端点时报错', async () => {
    await expect(resolveEngine({ ...base, engine: 'jev' })).rejects.toThrow('未配置端点')
  })
})

// ---------------------------------------------------------------- 解析辅助

describe('模型输出的解析', () => {
  test('parseNoulAnswer 认多种写法', () => {
    expect(parseNoulAnswer('{"answer": "yes", "probability": 0.8}')).toBe('yes')
    expect(parseNoulAnswer('{"answer": "no"}')).toBe('no')
    expect(parseNoulAnswer('答案是 是')).toBe('yes')
    expect(parseNoulAnswer('我认为不成立')).toBe('no')
    expect(parseNoulAnswer('无法判断')).toBeNull()
  })

  test('parseNoulAnswer 不把 not / nothing 误读成 no（单词边界必须生效）', () => {
    expect(parseNoulAnswer('nothing here')).toBeNull()
    expect(parseNoulAnswer('I cannot decide')).toBeNull()
    expect(parseNoulAnswer('not sure')).toBeNull()
  })

  test('parseNoulAnswer 中文里「是否」是疑问词，不算否定', () => {
    // 「是否成立？我认为成立」既有疑问词「是否」又有肯定「成立」，应判 yes
    expect(parseNoulAnswer('是否成立？我认为成立')).toBe('yes')
    // 「不成立」是真正的否定
    expect(parseNoulAnswer('这个说法不成立')).toBe('no')
    // 只有疑问词、没有明确表态 → 判不出来
    expect(parseNoulAnswer('是否成立？')).toBeNull()
  })

  test('parseNoulAnswer 同时出现肯定与否定时取更早的那个', () => {
    expect(parseNoulAnswer('yes, not no')).toBe('yes')
    expect(parseNoulAnswer('no, but yes')).toBe('no')
  })

  test('parseChoiceKey 长 key 优先，避免短 key 误命中', () => {
    // "a" 是 "alpha" 的前缀，若按长度升序匹配会选错
    expect(parseChoiceKey('I choose alpha', ['a', 'alpha'])).toBe('alpha')
    expect(parseChoiceKey('{"answer": "b"}', ['a', 'b'])).toBe('b')
    expect(parseChoiceKey('完全无关', ['a', 'b'])).toBeNull()
  })

  test('extractJson 容忍 ``` 围栏与前后散文', () => {
    expect(extractJson('```json\n{"a":1}\n```')).toEqual({ a: 1 })
    expect(extractJson('好的，这是设计：\n{"a":1}\n希望有帮助')).toEqual({ a: 1 })
    expect(extractJson('没有 JSON')).toBeNull()
    expect(extractJson('{坏掉的')).toBeNull()
  })
})

// ---------------------------------------------------------------- 本地引擎数学

describe('本地引擎：多样本投票', () => {
  const q: DecisionRequest = {
    state: '一段材料',
    questions: { gate: { type: 'noul', instructions: '是否成立' } },
  }

  test('noul 取投票占比，而不是自报概率的均值', async () => {
    // 3 次采样里 2 次 yes → 0.667。自报值都虚高到 0.9，故意验证不被采信
    const engine = new LocalEngine({
      samples: 3,
      config: modelConfig,
      runner: fakeRunner([
        '{"answer":"yes","probability":0.9}',
        '{"answer":"yes","probability":0.9}',
        '{"answer":"no","probability":0.1}',
      ]),
    })
    const response = await engine.evaluate(q)
    const answer = response.answers.gate!
    expect(answer.value).toBeCloseTo(2 / 3, 3)
    expect(answer.distribution).toEqual({ yes: 0.6667, no: 0.3333 })
    // 自报均值只作参考放进 confidence
    expect(answer.confidence).toBeCloseTo((0.9 + 0.9 + 0.1) / 3, 3)
    expect(answer.calibrated).toBe(false)
  })

  test('choice 取众数并给出分布；平票时取 criteria 里靠前的 key', async () => {
    const request: DecisionRequest = {
      state: 'x',
      questions: {
        severity: {
          type: 'choice',
          instructions: '严重度',
          criteria: { critical: '严重', minor: '轻微' },
        },
      },
    }

    // 2:1 → critical
    const majority = await new LocalEngine({
      samples: 3,
      config: modelConfig,
      runner: fakeRunner(['critical', 'critical', 'minor']),
    }).evaluate(request)
    expect(majority.answers.severity!.value).toBe('critical')
    expect(majority.answers.severity!.distribution).toEqual({ critical: 0.6667, minor: 0.3333 })

    // 1:1 平票 → 取 criteria 首个 key（critical）
    const tie = await new LocalEngine({
      samples: 2,
      config: modelConfig,
      runner: fakeRunner(['minor', 'critical']),
    }).evaluate(request)
    expect(tie.answers.severity!.value).toBe('critical')
  })

  test('score 取加权期望档位', async () => {
    const request: DecisionRequest = {
      state: 'x',
      questions: {
        quality: {
          type: 'score',
          instructions: '质量',
          criteria: ['high', 'medium', 'low'], // 最高档在前
        },
      },
    }
    // 两次 high、一次 medium → 期望分 (3+3+2)/3 = 2.67 → 最近档位 index 0 = high
    const response = await new LocalEngine({
      samples: 3,
      config: modelConfig,
      runner: fakeRunner(['high', 'high', 'medium']),
    }).evaluate(request)
    expect(response.answers.quality!.value).toBe('high')
    expect(response.answers.quality!.distribution).toEqual({ high: 0.6667, medium: 0.3333, low: 0 })
  })

  test('所有采样都没返回时给出中性默认值，不编造概率', async () => {
    const engine = new LocalEngine({
      samples: 3,
      config: modelConfig,
      runner: async () => null, // 全部失败
    })
    const response = await engine.evaluate(q)
    const answer = response.answers.gate!
    expect(answer.value).toBe(0.5)
    expect(answer.confidence).toBe(0)
    expect(answer.calibrated).toBe(false)
    expect(response.notes.join(' ')).toContain('未返回')
  })

  test('结果一律标注未经校准', async () => {
    const response = await new LocalEngine({
      samples: 1,
      config: modelConfig,
      runner: fakeRunner(['{"answer":"yes","probability":0.99}']),
    }).evaluate(q)
    expect(response.answers.gate!.calibrated).toBe(false)
    expect(response.notes.join(' ')).toContain('未经校准')
  })

  test('state 过长时截断并在 notes 说明', async () => {
    const engine = new LocalEngine({
      samples: 1,
      config: modelConfig,
      runner: fakeRunner(['{"answer":"yes","probability":0.5}']),
    })
    const response = await engine.evaluate({
      state: 'x'.repeat(MAX_STATE_CHARS + 1000),
      questions: { gate: { type: 'noul', instructions: '?' } },
    })
    expect(response.notes.join(' ')).toContain('截断')
  })
})

// ---------------------------------------------------------------- 启发式：只说不知道

describe('启发式兜底：刻意中性且不自称确定', () => {
  test('产出中性值、confidence 为 0，并明确警告不要据此决策', async () => {
    const response = await new HeuristicEngine().evaluate({
      state: 'x',
      questions: {
        pick: { type: 'choice', instructions: '选', criteria: { a: '甲', b: '乙' } },
        prob: { type: 'noul', instructions: '是否' },
        grade: { type: 'score', instructions: '打分', criteria: ['高', '中', '低'] },
      },
    })

    expect(response.engine).toBe('heuristic')
    expect(response.answers.pick!.value).toBe('a') // 第一个 key，不指引方向
    expect(response.answers.prob!.value).toBe(0.5) // 绝对中性
    expect(response.answers.grade!.value).toBe('中') // 中间档
    for (const answer of Object.values(response.answers)) {
      expect(answer.calibrated).toBe(false)
      expect(answer.confidence).toBe(0)
    }
    expect(response.notes.join(' ')).toContain('请勿据此做决策')
  })
})

// ---------------------------------------------------------------- Jev 引擎

describe('Jev 引擎', () => {
  test('把内部问题翻成 systemOne 请求，并把结果标为已校准', async () => {
    let capturedBody: any = null
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async (_url: string, init: any) => {
      capturedBody = JSON.parse(init.body)
      return new Response(
        JSON.stringify({
          answers: { severity: { choice: 'critical', confidence: 0.9 } },
          model: 'jev-latest',
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )
    }) as any

    try {
      const engine = new JevEngine({ baseUrl: 'http://jev:8000', apiKey: 'k' })
      const response = await engine.evaluate({
        state: '材料',
        questions: {
          severity: {
            type: 'choice',
            instructions: '严重度',
            criteria: { critical: '严重', minor: '轻微' },
          },
        },
      })

      expect(response.engine).toBe('jev')
      expect(response.answers.severity!.value).toBe('critical')
      // 关键：专用 System One 模型，标为已校准
      expect(response.answers.severity!.calibrated).toBe(true)
      // 请求体形状对齐 pi-jev 的 systemOne
      expect(capturedBody.questions.severity.type).toBe('choice')
      expect(capturedBody.state.text).toBe('材料')
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('端点报错时抛出，不返回编造值', async () => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async () => new Response('boom', { status: 500 })) as any
    try {
      const engine = new JevEngine({ baseUrl: 'http://jev:8000', apiKey: '' })
      await expect(
        engine.evaluate({ state: 'x', questions: { q: { type: 'noul', instructions: '?' } } }),
      ).rejects.toThrow('500')
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})

// ---------------------------------------------------------------- 设计器校验

describe('设计器：严格校验不可信的模型输出', () => {
  test('接受一份合法设计', () => {
    const design = validateDesign({
      state: '材料',
      questions: {
        ok_noul: { type: 'noul', instructions: '是否成立' },
        ok_choice: { type: 'choice', instructions: '选一个', criteria: { a: '甲', b: '乙' } },
        ok_score: { type: 'score', instructions: '打分', criteria: ['高', '中', '低'] },
      },
    })
    expect(design).not.toBeNull()
    expect(Object.keys(design!.questions).sort()).toEqual(['ok_choice', 'ok_noul', 'ok_score'])
  })

  test('choice 的 criteria 若是数组则丢弃（那是 score 的形态）', () => {
    const design = validateDesign({
      state: 's',
      questions: {
        bad: { type: 'choice', instructions: '选', criteria: ['a', 'b'] },
        good: { type: 'noul', instructions: '是否' },
      },
    })
    expect(Object.keys(design!.questions)).toEqual(['good'])
  })

  test('score 的 criteria 为空数组或非数组则丢弃', () => {
    const design = validateDesign({
      state: 's',
      questions: {
        empty: { type: 'score', instructions: '打分', criteria: [] },
        notArray: { type: 'score', instructions: '打分', criteria: { a: 'x' } },
        good: { type: 'noul', instructions: '是否' },
      },
    })
    expect(Object.keys(design!.questions)).toEqual(['good'])
  })

  test('noul 带 criteria 则丢弃（模型误解了 yes/no 语义）', () => {
    const design = validateDesign({
      state: 's',
      questions: {
        bad: { type: 'noul', instructions: '是否', criteria: { a: 'x' } },
        good: { type: 'noul', instructions: '是否' },
      },
    })
    expect(Object.keys(design!.questions)).toEqual(['good'])
  })

  test('缺 instructions 或 type 未知的问题被丢弃', () => {
    const design = validateDesign({
      state: 's',
      questions: {
        noInstr: { type: 'noul' },
        blankInstr: { type: 'noul', instructions: '   ' },
        badType: { type: 'ranking', instructions: '排个序' },
        good: { type: 'noul', instructions: '是否' },
      },
    })
    expect(Object.keys(design!.questions)).toEqual(['good'])
  })

  test('所有问题都不合法时整体失败，不放半残 schema 过去', () => {
    expect(validateDesign({ state: 's', questions: { bad: { type: 'noul' } } })).toBeNull()
    expect(validateDesign({ state: 's', questions: {} })).toBeNull()
    expect(validateDesign({ questions: { a: { type: 'noul', instructions: 'x' } } })).toBeNull()
    expect(validateDesign(null)).toBeNull()
    expect(validateDesign('不是对象')).toBeNull()
  })

  test('问题数超过上限时截断', () => {
    const questions: Record<string, unknown> = {}
    for (let i = 0; i < 20; i++) {
      questions[`q${i}`] = { type: 'noul', instructions: `问题 ${i}` }
    }
    const design = validateDesign({ state: 's', questions })
    expect(Object.keys(design!.questions).length).toBe(MAX_DESIGNED_QUESTIONS)
  })

  test('设上限时按需截断', () => {
    const design = validateDesign(
      { state: 's', questions: { a: { type: 'noul', instructions: '1' }, b: { type: 'noul', instructions: '2' } } },
      1,
    )
    expect(Object.keys(design!.questions).length).toBe(1)
  })

  test('designDecision 能处理带围栏与散文的模型输出', async () => {
    const outcome = await designDecision('测试', {
      config: modelConfig,
      runner: async () =>
        '这是我的设计：\n```json\n{"state":"材料","questions":{"q":{"type":"noul","instructions":"是否"}}}\n```\n完成',
    })
    expect(outcome.design).not.toBeNull()
    expect(Object.keys(outcome.design!.questions)).toEqual(['q'])
  })

  test('模型返回不可用时给出面向用户的错误', async () => {
    const outcome = await designDecision('测试', {
      config: modelConfig,
      runner: async () => '抱歉，我无法完成',
    })
    expect(outcome.design).toBeNull()
    expect(outcome.error).toContain('未返回可用的决策 schema')
  })

  test('设计系统提示词包含类型规则与问题数上限', () => {
    expect(DESIGN_SYSTEM_PROMPT).toContain('noul')
    expect(DESIGN_SYSTEM_PROMPT).toContain(String(MAX_DESIGNED_QUESTIONS))
  })
})

// ---------------------------------------------------------------- 门禁

describe('门禁判定', () => {
  /** 造一个可控引擎，直接给定 noul 概率。 */
  function stubEngine(probability: number, calibrated = true, id: 'jev' | 'local' = 'jev'): DecisionEngine {
    return {
      id,
      async isAvailable() {
        return true
      },
      async evaluate(request) {
        const answers: Record<string, any> = {}
        for (const [key, question] of Object.entries(request.questions)) {
          answers[key] = { type: question.type, value: probability, calibrated }
        }
        return { answers, engine: id, elapsedMs: 1, samples: 1, notes: [] }
      },
    }
  }

  const baseConfig: DecisionConfig = {
    engine: 'auto',
    baseUrl: '',
    apiKey: '',
    threshold: 0.65,
    samples: 1,
    sampleTimeoutMs: 1000,
  }

  test('概率达到阈值即通过，低于则不通过', async () => {
    const pass = await runGate({
      criteria: '测试通过',
      source: 'text',
      text: '产出',
      workspace: home,
      engine: stubEngine(0.8),
      config: baseConfig,
    })
    expect(pass.passed).toBe(true)
    expect(pass.probability).toBe(0.8)

    const fail = await runGate({
      criteria: '测试通过',
      source: 'text',
      text: '产出',
      workspace: home,
      engine: stubEngine(0.5),
      config: baseConfig,
    })
    expect(fail.passed).toBe(false)
  })

  test('阈值可覆盖，且边界值恰好等于阈值算通过', async () => {
    const outcome = await runGate({
      criteria: 'c',
      source: 'text',
      text: 't',
      threshold: 0.9,
      workspace: home,
      engine: stubEngine(0.9),
      config: baseConfig,
    })
    expect(outcome.passed).toBe(true)
    expect(outcome.threshold).toBe(0.9)
  })

  test('无可用引擎时默认 fail-close（这是刻意的，与 decide 相反）', async () => {
    const outcome = await runGate({
      criteria: 'c',
      source: 'text',
      text: 't',
      workspace: home,
      config: baseConfig, // 没配 baseUrl、没配模型 → heuristic
    })
    expect(outcome.engine).toBe('heuristic')
    expect(outcome.passed).toBe(false)
    expect(outcome.calibrated).toBe(false)
    expect(outcome.note).toContain('fail-close')
  })

  test('显式 fail_open 时放行，但标注无判定依据', async () => {
    const outcome = await runGate({
      criteria: 'c',
      source: 'text',
      text: 't',
      failOpen: true,
      workspace: home,
      config: baseConfig,
    })
    expect(outcome.passed).toBe(true)
    expect(outcome.note).toContain('无判定依据')
  })

  test('source=file 读工作区文件', async () => {
    const path = join(home, 'artifact.txt')
    await writeFile(path, '内容', 'utf8')
    let seen = ''
    const engine: DecisionEngine = {
      id: 'jev',
      async isAvailable() {
        return true
      },
      async evaluate(request) {
        seen = String(request.state)
        return { answers: { gate_passed: { type: 'noul', value: 1, calibrated: true } }, engine: 'jev', elapsedMs: 1, samples: 1, notes: [] }
      },
    }
    const outcome = await runGate({
      criteria: 'c',
      source: 'file',
      file: 'artifact.txt',
      workspace: home,
      engine,
      config: baseConfig,
    })
    expect(outcome.passed).toBe(true)
    expect(seen).toContain('内容')
  })

  test('source=file 越出工作区时被沙箱拒绝', async () => {
    await expect(
      runGate({
        criteria: 'c',
        source: 'file',
        file: '../escape.txt',
        workspace: home,
        engine: stubEngine(1),
        config: baseConfig,
      }),
    ).rejects.toThrow('拒绝访问工作区外的路径')
  })

  test('缺少 criteria 时报错', async () => {
    await expect(
      runGate({ criteria: '  ', source: 'text', text: 't', workspace: home, engine: stubEngine(1), config: baseConfig }),
    ).rejects.toThrow('缺少 criteria')
  })

  test('默认门禁阈值比通用决策阈值更严', () => {
    expect(DEFAULT_GATE_THRESHOLD).toBeGreaterThan(DEFAULT_DECISION_THRESHOLD)
  })
})

// ---------------------------------------------------------------- 工具层

describe('decide 工具', () => {
  const tool = createDecideTool()

  test('参数结构完整', () => {
    expect(tool.name).toBe('decide')
    const props = tool.parameters.properties as Record<string, unknown>
    expect(props.state).toBeDefined()
    expect(props.questions).toBeDefined()
    expect(tool.parameters.required).toContain('questions')
    // 描述里必须讲清 calibrated 的含义，这是插件的诚实性契约
    expect(tool.description).toContain('calibrated')
  })

  test('缺少 state 时报错', async () => {
    // 故意绕过类型（模型确实可能不传），验证运行时守卫
    const result = await tool.execute('c1', {
      questions: { a: { type: 'noul', instructions: 'x' } },
    } as any)
    expect(result.ok).toBe(false)
    expect(result.output).toContain('state')
  })

  test('问题定义全不合法时明确报错并列出原因', async () => {
    const result = await tool.execute('c2', {
      state: '材料',
      questions: { bad: { type: 'noul', instructions: '' } },
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('被丢弃的条目')
  })

  test('choice 给数组 criteria 时报错（提示正确形态）', async () => {
    const result = await tool.execute('c3', {
      state: '材料',
      questions: { pick: { type: 'choice', instructions: '选', criteria: ['a', 'b'] } },
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('选项 key')
  })
})

describe('check_gate 工具', () => {
  const tool = createCheckGateTool(process.cwd())

  test('参数结构完整且 source 有枚举', () => {
    expect(tool.name).toBe('check_gate')
    const props = tool.parameters.properties as any
    expect(props.criteria).toBeDefined()
    expect(props.source.enum).toEqual(['diff', 'file', 'text'])
    expect(props.fail_open).toBeDefined()
    // 描述里要写明 fail-close 的取舍
    expect(tool.description).toContain('fail-close')
  })

  test('缺少 criteria 时报错', async () => {
    const result = await tool.execute('g1', { criteria: '  ' })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('criteria')
  })

  test('source=file 缺 file 参数时报错', async () => {
    const result = await tool.execute('g2', { criteria: 'c', source: 'file' })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('file')
  })

  test('source=text 缺 text 参数时报错', async () => {
    const result = await tool.execute('g3', { criteria: 'c', source: 'text' })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('text')
  })

  test('无引擎时工具层也如实报 fail-close，不谎报通过', async () => {
    const result = await tool.execute('g4', { criteria: '所有函数都有类型标注', source: 'text', text: '产出' })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('未通过')
    const details = result.details as any
    expect(details.engine).toBe('heuristic')
    expect(details.passed).toBe(false)
  })
})

// ---------------------------------------------------------------- a_da 集成守护

describe('a_da 集成：注册表与子智能体可见性', () => {
  test('decision 插件在官方内置插件清单里', () => {
    const ids = BUILTIN_PLUGINS.map((p) => p.id)
    expect(ids).toContain('decision')
    const plugin = BUILTIN_PLUGINS.find((p) => p.id === 'decision')!
    // 三位一体：工具 + 技能 + 提示词
    expect(plugin.tools.length).toBe(3)
    expect(plugin.skills?.length).toBeGreaterThan(0)
    expect(plugin.prompts?.length).toBeGreaterThan(0)
  })

  test('三个决策工具都被认作只读（否则只读子智能体与 plan 模式拿不到）', () => {
    // AGENTS.md 第 2 条：isWriteTool 失败安全，只读工具漏登记就会被 mode 过滤器剔除
    expect(defaultToolRegistry.isWriteTool('decide')).toBe(false)
    expect(defaultToolRegistry.isWriteTool('design_decision')).toBe(false)
    expect(defaultToolRegistry.isWriteTool('check_gate')).toBe(false)
  })

  test('decide 与 check_gate 进通用工具表；design_decision 也在（供主智能体探索）', async () => {
    const { defaultExtensionLoader } = await import('../../loader')
    await defaultExtensionLoader.autoLoadExtensions(process.cwd())
    const names = defaultToolRegistry.getToolsForWorkspace(process.cwd()).map((t) => t.name)
    expect(names).toContain('decide')
    expect(names).toContain('design_decision')
    expect(names).toContain('check_gate')
  })
})
