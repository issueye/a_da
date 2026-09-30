/**
 * 「打开文件选择器」两条接线的确定性测试（不挂窗口）。
 *
 * 为什么不写成真窗口 e2e：那两条用例单独跑是绿的，全量跑必红——诊断显示点击根本没到本窗口
 * （`client.state.filePicker` 为空、下拉项计数 0），原因是 bun 把各测试文件放在同一进程里并发跑，
 * 别的文件也开着真窗口，按坐标派发的 click 落到了别人身上（`AGENTS.md` §13 末尾）。
 * 与其把测试改成"重试到偶然通过"，不如把**接线**抽成可确定性测试的东西（`picker-requests.ts`）。
 *
 * 覆盖：请求形状（模式/标题/起始目录/类型过滤）+ `onPicked` 之后该发的命令。
 */

import { describe, expect, test } from 'bun:test'
import type { AgentClient } from './client'
import { directoryPickerRequest, imagePickerRequest } from './picker-requests'

/** 记录命令的桩客户端。 */
function makeClient(options: { addError?: string | null } = {}) {
  const calls: Array<{ method: string; params: Record<string, unknown> }> = []
  const client = {
    state: { active: { id: 'thread-1', workspace: 'E:/repo' } },
    request: async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params })
      if (method === 'workspace.add') return { error: options.addError ?? null }
      return undefined
    },
  } as unknown as AgentClient
  return { client, calls }
}

describe('目录选择请求（添加工作区）', () => {
  test('形状：目录模式、带起始目录、标题明确', () => {
    const { client } = makeClient()
    const request = directoryPickerRequest(client, 'E:/repo')

    expect(request.mode).toBe('directory')
    expect(request.title).toContain('工作区')
    expect(request.startPath).toBe('E:/repo')
  })

  test('确认后：先 workspace.add，再把这个会话绑到新工作区', async () => {
    const { client, calls } = makeClient()
    const request = directoryPickerRequest(client, 'E:/repo')

    request.onPicked(['E:/new-repo'])
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(calls.map((call) => call.method)).toEqual(['workspace.add', 'thread.setWorkspace'])
    expect(calls[0]!.params).toEqual({ path: 'E:/new-repo' })
    expect(calls[1]!.params).toEqual({ threadId: 'thread-1', workspace: 'E:/new-repo' })
  })

  test('host 拒绝了（error 非空）就不再绑会话——别把失败掩盖成"绑定成功"', async () => {
    const { client, calls } = makeClient({ addError: '路径不存在：E:/nope' })
    const request = directoryPickerRequest(client, 'E:/repo')

    request.onPicked(['E:/nope'])
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(calls.map((call) => call.method)).toEqual(['workspace.add'])
  })

  test('没有选到路径时什么都不做', async () => {
    const { client, calls } = makeClient()
    directoryPickerRequest(client).onPicked([])
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(calls).toEqual([])
  })
})

describe('图片选择请求（输入框附件）', () => {
  test('形状：文件模式、按图片类型过滤、起始目录是当前会话的工作区', () => {
    const { client } = makeClient()
    const request = imagePickerRequest(client, /\.(png|jpe?g)$/i, () => {})

    expect(request.mode).toBe('files')
    expect(request.startPath).toBe('E:/repo')
    expect(request.accept?.test('a.png')).toBe(true)
    expect(request.accept?.test('notes.txt')).toBe(false)
  })

  test('确认后把路径交给调用方（Composer 负责去重与落 state）', () => {
    const { client } = makeClient()
    const received: string[][] = []
    const request = imagePickerRequest(client, /\.png$/i, (paths) => received.push(paths))

    request.onPicked(['E:/a.png', 'E:/b.png'])
    expect(received).toEqual([['E:/a.png', 'E:/b.png']])
  })
})
