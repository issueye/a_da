/**
 * 轻提示（toast）测试。
 *
 * 盯住两件事，都是"看起来装上了、其实没生效"的高危点：
 *
 * 1. **纯逻辑层**：入队、自动消失、上限丢最旧、空消息不入队——这些不依赖窗口，
 *    所以用假快照来源直接测，跑得快也不受单窗口约束影响；
 * 2. **文案层**：启停提示有没有把行为事实说清楚（"停用是全局的""系统提示词不再注入
 *    上下文"这类）。这些是纯函数，断言成本远低于开真窗口点按钮。
 *
 * 真窗口只留一条、只证明"确实画出来了"，原因见该 describe 前的注释。
 */

import { describe, expect, test } from 'bun:test'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { ToastHost } from './ToastHost'
import { agentClient } from './client'
import { createViewStore, type SnapshotSource } from './client/view-store'
import {
  capabilityToggleNotice,
  pluginToggleNotice,
  promptToggleFailedNotice,
  promptToggleNotice,
  skillToggleNotice,
  subagentToggleNotice,
} from './action-notices'
import type { ClientSnapshot } from '../shared/protocol'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

/** 一份最小可用快照：复制视图只要求这些字段存在。 */
function makeSnapshot(): ClientSnapshot {
  const thread = {
    id: 't1',
    title: '会话',
    workspace: 'C:/ws',
    createdAt: 1,
    mode: 'code' as const,
    items: [],
  }
  return {
    threads: [thread as never],
    activeThreadId: 't1',
    runningThreadIds: [],
    waitingThreadIds: [],
    queue: [],
    log: [],
    workspace: { project: 'C:/ws', files: 0, dirs: 0, scanning: false, entries: [] },
    config: {
      model: 'm',
      contextWindow: 1000,
      supportsImages: false,
      approval: 'ask' as const,
      effort: 'max' as const,
      mode: 'code' as const,
    },
    appearance: 'dark',
    pendingQuestions: [],
    publicWorkspace: 'C:/public',
    ui: {
      activeId: 't1',
      openTabIds: ['t1'],
      debugOpen: false,
      settingsOpen: false,
      pluginsOpen: false,
      changesOpen: false,
      paletteOpen: false,
      sidebarOpen: true,
      searchOpen: false,
      pendingDraft: null,
    },
  } as unknown as ClientSnapshot
}

function fakeSource(): SnapshotSource {
  const listeners = new Set<(snapshot: ClientSnapshot) => void>()
  return {
    snapshot: () => makeSnapshot(),
    subscribe: (listener) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}

describe('轻提示（纯逻辑）', () => {
  test('入队后能读到，级别与 detail 原样保留', () => {
    const view = createViewStore(fakeSource())
    view.notify({ level: 'warn', message: '已在全局停用插件「x」', detail: '对所有工作区生效' })
    const toasts = view.getState().toasts
    expect(toasts.length).toBe(1)
    expect(toasts[0]!.level).toBe('warn')
    expect(toasts[0]!.message).toBe('已在全局停用插件「x」')
    expect(toasts[0]!.detail).toBe('对所有工作区生效')
  })

  test('不给级别默认 success；空消息不入队（不渲染空框）', () => {
    const view = createViewStore(fakeSource())
    view.notify({ message: '已启用' })
    expect(view.getState().toasts[0]!.level).toBe('success')

    view.notify({ message: '   ' })
    expect(view.getState().toasts.length).toBe(1)
  })

  test('错误默认不自动消失，成功默认会自动消失', () => {
    const view = createViewStore(fakeSource())
    view.notify({ level: 'error', message: '失败了' })
    view.notify({ message: '成功了' })
    const [errorToast, successToast] = view.getState().toasts
    expect(errorToast!.durationMs).toBe(0)
    expect(successToast!.durationMs).toBeGreaterThan(0)
  })

  test('超过 3 条时丢最旧的', () => {
    const view = createViewStore(fakeSource())
    for (const text of ['一', '二', '三', '四']) {
      view.notify({ message: text, durationMs: 0 })
    }
    const messages = view.getState().toasts.map((toast) => toast.message)
    expect(messages).toEqual(['二', '三', '四'])
  })

  test('dismissToast 只删指定那条；删不存在的 id 不改变状态引用', () => {
    const view = createViewStore(fakeSource())
    view.notify({ message: '一', durationMs: 0 })
    view.notify({ message: '二', durationMs: 0 })
    const before = view.getState().toasts
    view.dismissToast('不存在')
    expect(view.getState().toasts).toBe(before)

    view.dismissToast(before[0]!.id)
    expect(view.getState().toasts.map((toast) => toast.message)).toEqual(['二'])
  })

  test('到点自动消失', async () => {
    const view = createViewStore(fakeSource())
    view.notify({ message: '马上就没', durationMs: 30 })
    expect(view.getState().toasts.length).toBe(1)
    await new Promise((resolve) => setTimeout(resolve, 80))
    expect(view.getState().toasts.length).toBe(0)
  })
})

describe('启停提示文案', () => {
  // 这组断言盯的是"文案有没有把行为事实说清楚"。这些事实来自实现里几处**刻意的不对称**，
  // 一旦后人改了语义而没改文案，这里会红——反过来说，现在红着就说明文案已经不诚实了。

  test('停用插件：说清"全局"这条关键限定，且级别高于普通停用', () => {
    const notice = pluginToggleNotice('探针插件', 2, false)
    expect(notice.level).toBe('warn')
    expect(notice.message).toContain('已在全局停用插件')
    // ExtensionLoader.togglePlugin 忽略 workspace，写的是全局 disabledPlugins。
    // 不说这句，用户换个工作区发现插件也没了，会当成 bug 报上来。
    expect(notice.detail).toContain('对所有工作区生效')
  })

  test('启用插件：只报成功，并说明工具何时可用', () => {
    const notice = pluginToggleNotice('探针插件', 3, true)
    expect(notice.level).toBe('success')
    expect(notice.message).toBe('已启用插件「探针插件」')
    expect(notice.message).not.toContain('全局')
    expect(notice.detail).toContain('3')
  })

  test('系统提示词停用：说清"不再注入模型上下文"', () => {
    const notice = promptToggleNotice('系统提示词', true, false)
    expect(notice.level).toBe('warn')
    expect(notice.detail).toContain('不再注入模型上下文')
  })

  test('普通提示词停用：说清斜杠命令里不再出现', () => {
    const notice = promptToggleNotice('写周报', false, false)
    expect(notice.detail).toContain('斜杠命令')
  })

  test('提示词启停失败：必须是 error，且不能报"已停用"', () => {
    const notice = promptToggleFailedNotice('写周报')
    expect(notice.level).toBe('error')
    expect(notice.message).toContain('失败')
    expect(notice.message).not.toContain('已停用')
  })

  test('子智能体停用：说清已有子会话不受影响', () => {
    const notice = subagentToggleNotice('审查员', false)
    expect(notice.level).toBe('warn')
    expect(notice.detail).toContain('历史子会话不受影响')
  })

  test('关闭能力：说清受影响的是用到它的插件，并带上现成的影响描述', () => {
    const notice = capabilityToggleNotice('审批', '插件将改用默认审批策略', false)
    expect(notice.level).toBe('warn')
    expect(notice.message).toBe('已关闭能力：审批')
    expect(notice.detail).toContain('用到它的插件')
    expect(notice.detail).toContain('插件将改用默认审批策略')
  })

  test('能力开关查不到描述时也要有兜底说明，不能是空白', () => {
    const notice = capabilityToggleNotice('未知开关', undefined, false)
    expect(notice.detail).toBeTruthy()
  })

  test('技能停用：说清模型不再看到它', () => {
    const notice = skillToggleNotice('重构助手', false, false)
    expect(notice.level).toBe('warn')
    expect(notice.detail).toContain('模型不会再看到这个技能')
  })

  test('仅指令唤醒的技能：启用后说清模型不会自动调用', () => {
    const notice = skillToggleNotice('重构助手', true, true)
    expect(notice.level).toBe('success')
    expect(notice.detail).toContain('仅指令唤醒')
  })
})

/**
 * 真窗口只留**一条**，而且刻意做到最轻：不建临时工作区、不改 A_DA_CONFIG、不动 store
 * 单例、不开任何弹窗。
 *
 * 这么克制是有实测依据的：原本这里有两条用例（开插件弹窗、点停用开关），它们把全量测试
 * 从 ~80s 拖到 420s+，并让后面几十个文件系统用例集体撞 5s 超时——因为真窗口用例受
 * "同一时刻只允许一个真窗口活着"的约束，多一个窗口就会让后面按坐标派发的 click 落空，
 * 于是每个用例都跑满自己的轮询超时（同步 fs 反而快、异步 fs 全线卡死，正是事件循环被
 * 渲染循环饿死的特征）。所以文案验证交给上面的纯函数，这里只证明"真的画出来了"。
 */
describeNative('轻提示（真窗口渲染）', () => {
  async function mount() {
    const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
    // 只渲染提示层本身，不渲染 AgentWindow：整棵应用树会带上自己的后台订阅与定时刷新，
    // 那不是这条用例要验的东西，却实打实地拖慢后续几十个文件。
    render(<ToastHost client={agentClient} />)
    const app = await connectTest(renderer)
    const screen = () => renderer.getPaintedText().join('\n')
    const waitFor = async (fn: () => Promise<boolean>, timeoutMs = 10_000) => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        renderer.flush?.()
        if (await fn()) return true
        await new Promise((resolve) => setTimeout(resolve, 40))
      }
      return false
    }
    return { app, renderer, screen, waitFor, close: () => app.close() }
  }

  test('提示画在窗口上，并能手动关掉', async () => {
    const { app, renderer, screen, waitFor, close } = await mount()
    try {
      // 先清干净，保证下面按 testId 取到的是确定的那一条
      for (const toast of [...agentClient.state.toasts]) {
        agentClient.ui.dismissToast(toast.id)
      }
      renderer.flush?.()

      agentClient.ui.notify({ message: '这是一条测试提示', durationMs: 0 })
      const shown = await waitFor(async () => (await app.getByTestId('toast-success').count()) > 0)
      expect(shown, `提示没有画出来。屏幕文本：\n${screen()}`).toBe(true)
      expect(screen()).toContain('这是一条测试提示')
      expect(agentClient.state.toasts.length).toBe(1)

      await app.getByTestId('toast-dismiss').click()
      const dismissed = await waitFor(async () => (await app.getByTestId('toast-host').count()) === 0)
      expect(dismissed).toBe(true)
    } finally {
      // 必须兜住：漏关窗口就是上面说的级联卡死的起点
      await close()
    }
  }, 30_000)
})
