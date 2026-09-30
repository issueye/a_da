/**
 * 应用内文件选择器（取代原生选择窗口）。
 *
 * 用一个桩客户端喂固定目录树：这里验的是**界面行为**（浏览、过滤、选中、错误如实显示、
 * 隐藏项计数、新建文件夹），不是文件系统本身——后者由 `src/agent/host/fs-service.test.ts`
 * 用真临时目录覆盖。两边合起来才是完整的一条路。
 */

import { describe, expect, test } from 'bun:test'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import type { AgentClient } from './client'
import { FilePicker } from './FilePicker'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const ROOT = process.platform === 'win32' ? 'C:\\work' : '/work'
const SUB = process.platform === 'win32' ? 'C:\\work\\proj' : '/work/proj'
const join = (base: string, name: string): string =>
  `${base}${process.platform === 'win32' ? '\\' : '/'}${name}`

/** 桩客户端：只实现这一页要用的 fs.*，并把收到的方法记下来。 */
function makeClient() {
  const calls: Array<{ method: string; params: Record<string, unknown> }> = []
  const listings: Record<string, unknown> = {
    [ROOT]: {
      path: ROOT,
      parent: process.platform === 'win32' ? 'C:\\' : '/',
      entries: [
        { name: 'proj', path: SUB, kind: 'dir' },
        { name: 'a.png', path: join(ROOT, 'a.png'), kind: 'file', sizeBytes: 2048 },
        { name: 'b.txt', path: join(ROOT, 'b.txt'), kind: 'file', sizeBytes: 10 },
      ],
      truncated: false,
      omitted: 0,
      hiddenCount: 2,
    },
    [SUB]: {
      path: SUB,
      parent: ROOT,
      entries: [{ name: 'inner.png', path: join(SUB, 'inner.png'), kind: 'file', sizeBytes: 4096 }],
      truncated: false,
      omitted: 0,
      hiddenCount: 0,
    },
  }

  const client = {
    request: async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params })
      if (method === 'fs.roots') {
        return [
          { path: ROOT, label: 'work', kind: 'workspace' },
          { path: process.platform === 'win32' ? 'C:\\' : '/', label: 'C:\\', kind: 'drive' },
        ]
      }
      if (method === 'fs.list') {
        const listing = listings[String(params.path)]
        // 没带 showHidden 时隐藏项被省略（与主机行为一致）
        if (params.showHidden !== true && listing) {
          return { ...(listing as object), entries: (listing as { entries: unknown[] }).entries }
        }
        if (!listing) throw new Error(`路径不存在：${String(params.path)}`)
        return listing
      }
      if (method === 'fs.mkdir') {
        const created = String(params.path)
        listings[created] = { path: created, parent: ROOT, entries: [], truncated: false, omitted: 0, hiddenCount: 0 }
        return { path: created }
      }
      throw new Error(`桩没实现 ${method}`)
    },
  } as unknown as AgentClient

  return { client, calls }
}

async function mountPicker(options: {
  mode: 'directory' | 'files'
  accept?: RegExp
  picked?: string[][]
  closed?: { count: number }
}) {
  const { client, calls } = makeClient()
  const { render, renderer } = createTestRoot({ width: 900, height: 700 })
  render(
    <FilePicker
      client={client}
      mode={options.mode}
      startPath={ROOT}
      accept={options.accept}
      onPicked={(paths) => options.picked?.push(paths)}
      onClose={() => {
        if (options.closed) options.closed.count += 1
      }}
    />
  )
  const app = await connectTest(renderer)
  const screen = (): string => renderer.getPaintedText().join('\n')
  const painted = async (needle: string, timeoutMs = 8000): Promise<void> => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 40))
    }
    throw new Error(`没画出来：${needle}\n${screen()}`)
  }
  return { app, renderer, screen, painted, calls }
}

describeNative('FilePicker', () => {
  test('目录模式：进入子目录后确认选的是当前目录', async () => {
    const picked: string[][] = []
    const { app, painted, calls, screen } = await mountPicker({ mode: 'directory', picked })

    await painted('proj')
    expect(screen()).toContain('选择此目录')

    // 进入子目录
    await app.getByTestId('file-picker-entry-proj').click()
    await painted('inner.png')
    expect(calls.some((call) => call.method === 'fs.list' && call.params.path === SUB)).toBe(true)

    // 确认 → 拿到的是当前目录
    await app.getByTestId('file-picker-confirm').click()
    expect(picked).toEqual([[SUB]])
    await app.close()
  })

  test('文件模式：按类型过滤、多选、确认回传路径', async () => {
    const picked: string[][] = []
    const { app, painted, screen } = await mountPicker({
      mode: 'files',
      accept: /\.png$/i,
      picked,
    })

    await painted('a.png')
    // 被过滤掉的项不出现，而且如实说明过滤了几项
    expect(screen()).not.toContain('b.txt')
    expect(screen()).toContain('已按类型过滤掉 1 项')

    await app.getByTestId('file-picker-entry-a.png').click()
    await painted('添加 1 个文件')
    await app.getByTestId('file-picker-confirm').click()
    expect(picked).toEqual([[join(ROOT, 'a.png')]])
    await app.close()
  })

  test('隐藏项如实计数，点一下才去要它们', async () => {
    const { app, painted, calls, screen } = await mountPicker({ mode: 'directory' })

    await painted('proj')
    expect(screen()).toContain('还有 2 个隐藏项')

    await app.getByTestId('file-picker-show-hidden').click()
    await new Promise((resolve) => setTimeout(resolve, 120))
    expect(calls.some((call) => call.method === 'fs.list' && call.params.showHidden === true)).toBe(true)
    await app.close()
  })

  test('路径不存在时把主机的原话显示出来（不静默、不假装空目录）', async () => {
    const { app, painted, screen } = await mountPicker({ mode: 'directory' })

    await painted('proj')
    // 直接输入一个不存在的路径并回车
    await app.getByTestId('file-picker-path').fill(join(ROOT, 'nope'))
    await app.getByTestId('file-picker-path').press('Enter')
    await painted('路径不存在')
    expect(screen()).toContain(join(ROOT, 'nope'))
    await app.close()
  })

  test('目录模式可以新建文件夹（走 fs.mkdir 并切过去）', async () => {
    const { app, painted, calls } = await mountPicker({ mode: 'directory' })

    await painted('proj')
    await app.getByTestId('file-picker-mkdir').click()
    await app.getByTestId('file-picker-new-folder').fill('新项目')
    await app.getByTestId('file-picker-new-folder-ok').click()
    await painted('这个目录是空的')

    const mkdir = calls.find((call) => call.method === 'fs.mkdir')
    expect(String(mkdir?.params.path)).toContain('新项目')
    await app.close()
  })
})
