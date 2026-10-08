/**
 * 决策插件的数据契约。
 *
 * 这一层只描述「问什么、答什么」，不关心由谁回答——远端 Jev、本地模型自评还是
 * 确定性启发式，都实现同一个 DecisionEngine 接口（见 engine.ts）。这样上层工具与
 * 测试只依赖契约，换引擎不动调用方。
 */

export type QuestionType = 'choice' | 'noul' | 'score'

/** 从若干选项中选一个。criteria 是「选项 key → 选项说明」。 */
export interface ChoiceQuestion {
  type: 'choice'
  instructions: string
  criteria: Record<string, string | null>
}

/** 是/否概率问题。刻意不携带 criteria：语义就是「这件事成立吗」。 */
export interface NoulQuestion {
  type: 'noul'
  instructions: string
}

/** 按评分档位打分。criteria 是档位描述，**最高档在前**。 */
export interface ScoreQuestion {
  type: 'score'
  instructions: string
  criteria: string[]
}

export type DecisionQuestion = ChoiceQuestion | NoulQuestion | ScoreQuestion

export interface DecisionRequest {
  /** 被判断的材料：一段文本，或结构化对象 */
  state: string | Record<string, unknown>
  /** 问题 id → 问题定义 */
  questions: Record<string, DecisionQuestion>
  /** 单题覆盖阈值（不传用默认阈值） */
  threshold?: number
}

export interface DecisionAnswer {
  type: QuestionType
  /** choice=选项 key；noul=概率 0–1；score=档位名 */
  value: string | number
  /** 引擎自报置信度（可能缺省） */
  confidence?: number
  /** choice/score 的分布：选项/档位 → 占比 */
  distribution?: Record<string, number>
  /**
   * 这个值是否经过校准。
   *
   * 远端 Jev（专用 System One 模型）= true；本地模型自评与启发式 = false。
   * 这条字段是全插件诚实性的落点：调用方必须据此决定能把这个数字信到几分。
   */
  calibrated: boolean
}

/** 决策引擎标识：远端 Jev / 本地模型自评 / 确定性启发式 */
export type EngineId = 'jev' | 'local' | 'heuristic'

export interface DecisionResponse {
  answers: Record<string, DecisionAnswer>
  engine: EngineId
  model?: string
  elapsedMs: number
  /** 实际采样次数：本地引擎 >1，远端与启发式为 1 */
  samples: number
  /** 非致命说明，例如「本地自评未经校准」「state 被截断」「部分采样失败」 */
  notes: string[]
}

export interface DecisionEngine {
  readonly id: EngineId
  /** 是否可用（配置齐备 / 依赖就绪） */
  isAvailable(): Promise<boolean>
  evaluate(request: DecisionRequest, signal?: AbortSignal): Promise<DecisionResponse>
}

/** 门禁判定结果 */
export interface GateOutcome {
  passed: boolean
  probability: number
  threshold: number
  criteria: string
  engine: EngineId
  calibrated: boolean
  elapsedMs: number
  /** 引擎不可用等情形的说明 */
  note?: string
}
