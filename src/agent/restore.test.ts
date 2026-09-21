/**
 * 启动恢复：上次的会话要按「工作区/会话」摆回来。
 *
 * `store` 是模块级单例，构造发生在 import 那一刻，所以这里每个用例都 new 一个
 * `AgentStore`（不是重导入）。
 *
 * 磁盘位置必须换成自己的临时目录，理由有两层：不能碰用户真实的 `~/.a-da`；更要
 * 紧的是整个套件共用 `scripts/test-preload.ts` 设的那个 `A_DA_HOME`，别的测试
 * 文件一直在往里面写会话——共用它就会把别人的会话也恢复出来。所以这里每个用例
 * 换一个全新目录，用完把原值还回去，并且只删自己那个目录。
 *
 * 要钉住的是四件事：会话按项目分组回来、历史消息接回去、当前项目是最近用过的
 * 那个、以及恢复出来的会话还能继续写。
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AgentStore } from './store'
import { defaultSessionManager } from './session/manager'

let home = ''
/** 套件共用的那个 home，用完还回去——不能因为我换了目录就让别的文件换个家。 */
let suiteHome: string | undefined

beforeAll(() => {
  suiteHome = process.env.A_DA_HOME
})

afterAll(() => {
  if (suiteHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = suiteHome
})

/**
 * 每个用例换一个全新的 home。
 *
 * 必须换：构造 `AgentStore` 本身就会写一个会话文件，不换的话下一个用例会把它当成
 * 「上次的会话」恢复出来。
 */
beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'a-da-restore-'))
  process.env.A_DA_HOME = home
})

afterEach(async () => {
  if (suiteHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = suiteHome
  // 删不干净（Windows 上刚写过会 EBUSY）只是留个临时目录，不该把测试判红。
  await rm(home, { recursive: true, force: true }).catch(() => {})
})

/** 建一个临时项目目录，用完删掉。 */
async function project(name: string): Promise<{ path: string; cleanup: () => Promise<void> }> {
  const path = await mkdtemp(join(tmpdir(), `a-da-${name}-`))
  return { path, cleanup: () => rm(path, { recursive: true, force: true }) }
}

describe('restoring sessions on startup', () => {
  test('a fresh install starts on one empty session', () => {
    const store = new AgentStore(home)
    expect(store.threads).toHaveLength(1)
    expect(store.active.messages).toEqual([])
    expect(store.openTabs).toHaveLength(1)
  })

  test('sessions come back grouped by workspace, newest project active', async () => {
    const alpha = await project('alpha')
    const beta = await project('beta')
    try {
      await defaultSessionManager.createSession('s_alpha', alpha.path, '老项目的会话')
      await defaultSessionManager.appendMessage(
        's_alpha',
        { role: 'user', content: '上一个项目的问题', timestamp: 1000 },
        alpha.path,
      )
      // beta 建得晚，恢复后它应该是当前项目。
      await defaultSessionManager.createSession('s_beta', beta.path, '新项目的会话')
      await defaultSessionManager.appendMessage(
        's_beta',
        { role: 'user', content: '当前项目的问题', timestamp: 2000 },
        beta.path,
      )

      const store = new AgentStore(home)
      expect(store.threads.map((thread) => thread.id).sort()).toEqual(['s_alpha', 's_beta'])
      // 两个项目都回来了。
      expect(store.projects).toHaveLength(2)
      // 当前项目是最近动过的那个，不是磁盘顺序的第一个。
      expect(store.project).toBe(beta.path)
      expect(store.activeId).toBe('s_beta')
    } finally {
      await alpha.cleanup()
      await beta.cleanup()
    }
  })

  test('message history is reattached, so the next turn has its context', async () => {
    const workspace = await project('history')
    try {
      await defaultSessionManager.createSession('s_hist', workspace.path, '有历史的会话')
      await defaultSessionManager.appendMessage(
        's_hist',
        { role: 'user', content: '第一句', timestamp: 1000 },
        workspace.path,
      )
      await defaultSessionManager.appendMessage(
        's_hist',
        { role: 'assistant', content: '第一答', timestamp: 2000 },
        workspace.path,
      )

      const store = new AgentStore(home)
      const thread = store.threads.find((candidate) => candidate.id === 's_hist')!
      // 上下文原样接回，模型下一轮还认得前面的对话。
      expect(thread.messages.map((message) => message.content)).toEqual(['第一句', '第一答'])
      // 界面也把对话显示回来。
      expect(thread.items.map((item) => item.kind)).toEqual(['user', 'assistant'])
      expect(thread.items.map((item) => ('text' in item ? item.text : ''))).toEqual([
        '第一句',
        '第一答',
      ])
    } finally {
      await workspace.cleanup()
    }
  })

  test('a restored session can keep being written to', async () => {
    const workspace = await project('continue')
    try {
      await defaultSessionManager.createSession('s_cont', workspace.path, '接着聊')
      await defaultSessionManager.appendMessage(
        's_cont',
        { role: 'user', content: '之前的话', timestamp: 1000 },
        workspace.path,
      )

      const store = new AgentStore(home)
      // 恢复出来的会话照样能发新消息。send 之后消息先进队列，一轮跑起来才真正
      // 落进 thread.messages，所以这里等它排到队里就算接上了。
      store.send('新的一句')

      const thread = store.threads.find((candidate) => candidate.id === 's_cont')!
      expect(thread.messages[0]?.content).toBe('之前的话')
      expect(thread.items.some((item) => 'text' in item && item.text === '新的一句')).toBe(true)
    } finally {
      await workspace.cleanup()
    }
  })

  test('a session pointing at a deleted folder still restores', async () => {
    const workspace = await project('deleted')
    try {
      await defaultSessionManager.createSession('s_deleted', workspace.path, '目录会被删掉')
      const store = new AgentStore(home)
      expect(store.threads.map((thread) => thread.id)).toContain('s_deleted')

      // 用户把项目目录删了。会话文件还在，恢复照样进行——历史还在，只是这个项目
      // 现在指向一个不存在的路径（工具会报「索引工作区失败」，不会崩）。
      await workspace.cleanup()
      const after = new AgentStore(home)
      expect(after.threads.map((thread) => thread.id)).toContain('s_deleted')
    } finally {
      await rm(workspace.path, { recursive: true, force: true }).catch(() => {})
    }
  })

  test('restores thinking blocks and tool call cards from session messages', async () => {
    const workspace = await project('tools-restore')
    try {
      await defaultSessionManager.createSession('s_tools', workspace.path, '工具会话')
      await defaultSessionManager.appendMessage(
        's_tools',
        { role: 'user', content: '查看当前目录并创建文件', timestamp: 1000 },
        workspace.path,
      )
      await defaultSessionManager.appendMessage(
        's_tools',
        {
          role: 'assistant',
          content: '我来查看文件并写入。',
          thinking: '用户需要查看目录并新建文件，先调用 list_files。',
          toolCalls: [
            {
              id: 'call_1',
              name: 'list_files',
              arguments: { depth: 1 },
              rawArguments: '{"depth":1}',
            },
          ],
          timestamp: 2000,
        },
        workspace.path,
      )
      await defaultSessionManager.appendMessage(
        's_tools',
        {
          role: 'toolResult',
          toolCallId: 'call_1',
          toolName: 'list_files',
          content: 'package.json, src/',
          isError: false,
          timestamp: 2500,
        },
        workspace.path,
      )

      const store = new AgentStore(home)
      const thread = store.threads.find((t) => t.id === 's_tools')!
      expect(thread).toBeDefined()

      // items 包含 user, thinking, tool, assistant
      const kinds = thread.items.map((item) => item.kind)
      expect(kinds).toContain('user')
      expect(kinds).toContain('thinking')
      expect(kinds).toContain('tool')
      expect(kinds).toContain('assistant')

      const thinking = thread.items.find((item) => item.kind === 'thinking')
      expect(thinking).toBeDefined()
      if (thinking && thinking.kind === 'thinking') {
        expect(thinking.text).toContain('先调用 list_files')
      }

      const tool = thread.items.find((item) => item.kind === 'tool')
      expect(tool).toBeDefined()
      if (tool && tool.kind === 'tool') {
        expect(tool.name).toBe('list_files')
        expect(tool.output).toBe('package.json, src/')
        expect(tool.status).toBe('done')
      }
    } finally {
      await workspace.cleanup()
    }
  })
})
