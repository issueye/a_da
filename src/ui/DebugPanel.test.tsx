import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import { DebugPanel } from './DebugPanel'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative('DebugPanel', () => {
  test('渲染事件日志，支持展开查看请求内容与返回内容，并支持清空', async () => {
    store.clearLog()

    // 注入包含请求与响应详细载荷的测试日志
    ;(store as any).log = [
      {
        id: 1,
        at: 1700000000000,
        kind: 'request',
        model: 'deepseek-chat',
        text: 'deepseek-chat @ https://api.deepseek.com（2 条消息 · 1 个工具）',
        payload: {
          model: 'deepseek-chat',
          baseUrl: 'https://api.deepseek.com',
          messages: [
            { role: 'system', content: '你是编码助手' },
            { role: 'user', content: '帮我查一下文件' },
          ],
          tools: [{ name: 'list_files' }],
        },
        raw: JSON.stringify(
          {
            model: 'deepseek-chat',
            messages: [
              { role: 'system', content: '你是编码助手' },
              { role: 'user', content: '帮我查一下文件' },
            ],
          },
          null,
          2
        ),
      },
      {
        id: 2,
        at: 1700000001200,
        kind: 'response',
        model: 'deepseek-chat',
        durationMs: 1200,
        text: 'deepseek-chat 响应 · 1.2s · 450 tok · 调用 list_files',
        payload: {
          content: '好的，正在为您查找文件。',
          thinking: '用户需要查找文件，调用 list_files 工具',
          toolCalls: [{ id: 'call_1', name: 'list_files', args: {} }],
        },
        raw: JSON.stringify(
          {
            content: '好的，正在为您查找文件。',
            thinking: '用户需要查找文件，调用 list_files 工具',
          },
          null,
          2
        ),
      },
    ]

    const { render, renderer } = createTestRoot({ width: 600, height: 800 })
    render(<DebugPanel store={store} />)
    const app = await connectTest(renderer)

    // 验证日志条目数量
    expect(await app.getByTestId('debug-entry-1').count()).toBe(1)
    expect(await app.getByTestId('debug-entry-2').count()).toBe(1)

    // 初始状态下未展开详情
    expect(await app.getByTestId('debug-detail-1').count()).toBe(0)
    expect(await app.getByTestId('debug-detail-2').count()).toBe(0)

    // 点击第 1 条日志的头部展开查看请求内容
    await app.getByTestId('debug-entry-header-1').click()
    renderer.flush?.()

    // 验证展开面板已出现并包含请求文本
    expect(await app.getByTestId('debug-detail-1').count()).toBe(1)
    let screenText = renderer.getPaintedText().join(' ')
    expect(screenText).toContain('你是编码助手')
    expect(screenText).toContain('帮我查一下文件')

    // 点击第 2 条日志的头部展开查看返回内容
    await app.getByTestId('debug-entry-header-2').click()
    renderer.flush?.()

    expect(await app.getByTestId('debug-detail-2').count()).toBe(1)
    screenText = renderer.getPaintedText().join(' ')
    expect(screenText).toContain('好的，正在为您查找文件。')
    expect(screenText).toContain('用户需要查找文件')

    // 点击清空日志按钮
    const clearBtn = app.getByTestId('debug-clear')
    expect(await clearBtn.count()).toBe(1)
    await clearBtn.click()
    renderer.flush?.()

    expect(store.log.length).toBe(0)

    await app.close()
  })
})
