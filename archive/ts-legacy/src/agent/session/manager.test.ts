import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { SessionManager } from './manager'

let tempDir = ''
let manager: SessionManager

beforeAll(async () => {
  tempDir = await mkdtemp(join(tmpdir(), 'a-da-session-test-'))
  manager = new SessionManager(tempDir)
})

afterAll(async () => {
  if (tempDir) await rm(tempDir, { recursive: true, force: true })
})

describe('SessionManager', () => {
  test('creates a session and writes header in JSONL', async () => {
    const header = await manager.createSession('s_1', 'E:/test_project', '测试会话1')
    expect(header.id).toBe('s_1')
    expect(header.workspace).toBe('E:/test_project')

    const loaded = await manager.loadSession('s_1')
    expect(loaded).not.toBeNull()
    expect(loaded?.header.id).toBe('s_1')
    expect(loaded?.header.title).toBe('测试会话1')
    expect(loaded?.messages.length).toBe(0)
  })

  test('appends messages incrementally and reconstructs history', async () => {
    await manager.appendMessage('s_1', {
      role: 'user',
      content: '你好，帮我写一段代码',
      timestamp: 1000,
    })

    await manager.appendMessage('s_1', {
      role: 'assistant',
      content: '没问题，这是代码。',
      timestamp: 2000,
    })

    const loaded = await manager.loadSession('s_1')
    expect(loaded?.messages.length).toBe(2)
    expect(loaded?.messages[0]?.role).toBe('user')
    expect(loaded?.messages[0]?.content).toBe('你好，帮我写一段代码')
    expect(loaded?.messages[1]?.role).toBe('assistant')
  })

  test('updates session title', async () => {
    await manager.updateSessionTitle('s_1', '新修改的标题')
    const loaded = await manager.loadSession('s_1')
    expect(loaded?.header.title).toBe('新修改的标题')
  })

  test('lists sessions for workspace sorted by updatedAt', async () => {
    await manager.createSession('s_2', 'E:/test_project', '第二个会话')
    await manager.createSession('s_other', 'E:/other_project', '其他项目会话')

    const list = await manager.listSessionsForWorkspace('E:/test_project')
    expect(list.length).toBe(2)
    expect(list.map((s) => s.id)).toContain('s_1')
    expect(list.map((s) => s.id)).toContain('s_2')
    expect(list.map((s) => s.id)).not.toContain('s_other')
  })
  test('sessions land in a per-workspace directory', async () => {
    await manager.createSession('s_layout', 'E:/layout_project', '目录布局')
    // 散列目录名带一个 workspace.json 记下真实路径——启动恢复要靠它把会话挂回项目。
    const dir = dirname((await manager.listSessionsForWorkspace('E:/layout_project'))[0]!.filePath)
    expect(dirname(dir)).toBe(tempDir)
    expect(basename(dir)).not.toContain(':')
    expect(await readFile(join(dir, 'workspace.json'), 'utf-8')).toContain('layout_project')
    // 另一个项目的会话落在另一个目录，互不干扰。
    await manager.createSession('s_other_layout', 'E:/other_layout', '别的项目')
    const otherDir = dirname(
      (await manager.listSessionsForWorkspace('E:/other_layout'))[0]!.filePath,
    )
    expect(otherDir).not.toBe(dir)
  })

  test('listAllSessions spans every workspace', async () => {
    await manager.createSession('s_all_1', 'E:/all_a', '项目 A 的会话')
    await manager.createSession('s_all_2', 'E:/all_b', '项目 B 的会话')

    const all = await manager.listAllSessions()
    const ids = all.map((summary) => summary.id)
    expect(ids).toContain('s_all_1')
    expect(ids).toContain('s_all_2')
    // 每条都带着自己的项目，恢复时才挂得回去。
    for (const summary of all) expect(typeof summary.workspace).toBe('string')
  })

  test('listAllSessionsSync reads the same layout', async () => {
    const sync = manager.listAllSessionsSync().map((summary) => summary.id)
    expect(sync).toContain('s_all_1')
    expect(sync).toContain('s_all_2')
  })

  test('a workspace directory without its pointer is skipped', async () => {
    // 目录名是散列，真实路径只在 workspace.json 里。指针丢了就不知道该挂到哪个
    // 项目上——那个目录只能跳过，而不是把会话归到错误的项目。
    await manager.createSession('s_orphan', 'E:/orphan_project', '没有指针')
    const dir = dirname((await manager.listSessionsForWorkspace('E:/orphan_project'))[0]!.filePath)
    await rm(join(dir, 'workspace.json'), { force: true })

    const all = await manager.listAllSessions()
    expect(all.map((summary) => summary.id)).not.toContain('s_orphan')
  })

  test('a session whose file is gone reads as no history', async () => {
    await manager.createSession('s_gone', 'E:/gone_project', '会被删掉')
    const summary = (await manager.listSessionsForWorkspace('E:/gone_project'))[0]!
    await manager.deleteSession('s_gone', 'E:/gone_project')

    // 启动时撞上这个：文件已经不在了，就当这个会话没有历史，界面照常显示。
    expect(manager.loadSummaryMessagesSync(summary)).toEqual([])
  })

  test('one project written with either separator shares one directory', async () => {
    // 同一个项目会以两种形式出现：用户粘的是 `E:/code`，对话框选的是 `E:\code`。
    // 不归一化就会散列成两个目录，一个项目的会话被劈成两半——而且看不出来。
    const slashed = 'E:/codes/rust_projects/a_da'
    const backslashed = slashed.split('/').join('\\')
    await manager.createSession('s_sep_a', slashed, '斜杠形式')
    await manager.createSession('s_sep_b', backslashed, '反斜杠形式')

    expect(dirname((await manager.listSessionsForWorkspace(slashed))[0]!.filePath)).toBe(
      dirname((await manager.listSessionsForWorkspace(backslashed))[0]!.filePath),
    )
    // 两种写法都能看到全部会话。
    for (const form of [slashed, backslashed]) {
      const ids = (await manager.listSessionsForWorkspace(form)).map((s) => s.id)
      expect(ids).toContain('s_sep_a')
      expect(ids).toContain('s_sep_b')
    }
  })

  test('deleteWorkspace removes directory, pointer and all sessions', async () => {
    const ws = 'E:/doomed_workspace'
    await manager.createSession('s_doom_1', ws, '会话1')
    await manager.createSession('s_doom_2', ws, '会话2')
    const listBefore = await manager.listSessionsForWorkspace(ws)
    expect(listBefore.length).toBe(2)

    await manager.deleteWorkspace(ws)

    expect(await manager.listSessionsForWorkspace(ws)).toEqual([])
    const all = await manager.listAllSessions()
    expect(all.some((s) => s.workspace === ws)).toBe(false)
  })
})
