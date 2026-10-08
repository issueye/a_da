/**
 * 启停类动作的轻提示文案。
 *
 * ## 为什么要单独抽成纯函数
 *
 * 这些文案不只是"把话说漂亮"，它们承载的是**只有一句话才说得清的行为事实**：
 *
 * - 停用插件是**全局**的（`ExtensionLoader.togglePlugin` 忽略 workspace，写的是全局
 *   `disabledPlugins`，见 `config.ts` 的解析顺序）。用户若在 A 项目停用、到 B 项目发现
 *   也没了，不说"全局"就会被当成 bug。
 * - 关能力开关不是关某个插件，而是让**别的插件**静默少做一步。
 * - 停用系统提示词会**停止注入模型上下文**——这类"以为还在、其实没了"最伤人。
 *
 * 而验证这些事实，最稳的方式是纯函数断言，不是开真窗口点按钮：真窗口用例会把整套测试
 * 拖垮（同一时刻只允许一个真窗口活着，见 AGENTS.md；实测多加两个窗口会把全量从 80s
 * 拖到 420s+ 并让后面几十个文件系统用例级联超时）。所以这里保持纯函数、文案就地断言。
 */

import type { ToastLevel } from './client'

/** 与 `ui.notify` 的入参同构（`durationMs` 交给提示层按级别自己定）。 */
export interface ActionNotice {
  level?: ToastLevel
  message: string
  detail?: string
}

/**
 * 插件启停。
 *
 * 停用级别更高（warn）且必须带"全局"限定——见本文件顶部。
 */
export function pluginToggleNotice(name: string, toolCount: number, next: boolean): ActionNotice {
  return {
    level: next ? 'success' : 'warn',
    message: next ? `已启用插件「${name}」` : `已在全局停用插件「${name}」`,
    detail: next
      ? `导出的 ${toolCount} 个工具下一轮对话可用`
      : '该停用对所有工作区生效，在此处可随时重新启用',
  }
}

/** 提示词启停。`isSystem` 的两种结果差别很大，所以 detail 要分开写。 */
export function promptToggleNotice(name: string, isSystem: boolean, next: boolean): ActionNotice {
  return {
    level: next ? 'success' : 'warn',
    message: next ? `已启用提示词「${name}」` : `已停用提示词「${name}」`,
    detail: isSystem
      ? next
        ? '将作为 System Prompt 注入模型上下文'
        : '不再注入模型上下文，模型不会再看到它'
      : next
        ? '可在输入框用斜杠命令调用'
        : '斜杠命令里不再出现它',
  }
}

/**
 * 提示词启停**失败**（命令回了 `ok: false` 而没抛错）。
 *
 * 这种失败必须单列：命令返回成功形状时若照常报"已停用"，用户会以为设置生效了。
 */
export function promptToggleFailedNotice(name: string): ActionNotice {
  return { level: 'error', message: `切换提示词「${name}」失败`, detail: '主机没有写入这条状态' }
}

/** 子智能体启停。 */
export function subagentToggleNotice(name: string, next: boolean): ActionNotice {
  return {
    level: next ? 'success' : 'warn',
    message: next ? `已启用子智能体「${name}」` : `已停用子智能体「${name}」`,
    detail: next
      ? '主 Agent 可通过 invoke_subagent 委派给它'
      : '主 Agent 不会再委派任务给它；已建的历史子会话不受影响',
  }
}

/**
 * 能力开关。
 *
 * 关闭时必须 warn + 说明影响面：这个开关不是"这个插件"的开关，关掉后**别的插件**会
 * 静默少做一步。`effect` 直接取自 `capabilities-view` 里现成的那句描述，不手写第二份。
 */
export function capabilityToggleNotice(
  label: string,
  effect: string | undefined,
  next: boolean
): ActionNotice {
  return {
    level: next ? 'success' : 'warn',
    message: next ? `已开启能力：${label}` : `已关闭能力：${label}`,
    detail: next ? undefined : effect ? `用到它的插件将受限：${effect}` : '用到它的插件将显示受限原因',
  }
}

/**
 * 技能启停。`disableModelInvocation` 决定启用后模型能不能自动调用——
 * 它在 `SkillSummary` 上是可选的，**缺省等价于 false**（即允许自动调用），
 * 所以这里也收 `undefined`，别让"字段没填"和"明确允许"走成两条路。
 */
export function skillToggleNotice(
  name: string,
  disableModelInvocation: boolean | undefined,
  next: boolean
): ActionNotice {
  return {
    level: next ? 'success' : 'warn',
    message: next ? `已启用技能「${name}」` : `已停用技能「${name}」`,
    detail: next
      ? disableModelInvocation
        ? '仅指令唤醒：模型不会自动调用它'
        : '模型可在合适的任务上自动调用它'
      : '模型不会再看到这个技能；随时可重新启用',
  }
}
