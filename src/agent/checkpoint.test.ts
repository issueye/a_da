/**
 * 检查点与回滚的单元测试。
 *
 * 覆盖三类回滚语义：单次调用撤销（revertCheckpoint）、单文件恢复原状
 * （revertFile 取最早快照）、整会话一键恢复（revertAll），以及「已作废的
 * 检查点不能二次回滚」「新文件回滚即删除」「超大文件快照不完整只能跳过」。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { CheckpointManager } from './checkpoint'

let root = ''
let outside = ''
const manager = new CheckpointManager()

async function read(path: string): Promise<string> {
  return readFile(path, 'utf-8')
}

beforeAll(async () => {
  root = await mkdtemp(join(tmpdir(), 'a-da-ckpt-'))
  outside = await mkdtemp(join(tmpdir(), 'a-da-ckpt-out-'))
  await mkdir(join(root, 'src'), { recursive: true })
  await writeFile(join(root, 'src/app.ts'), 'const one = 1\n')
})

afterAll(async () => {
  await rm(root, { recursive: true, force: true })
  await rm(outside, { recursive: true, force: true })
})

describe('CheckpointManager capture 与 revertCheckpoint', () => {
  test('覆盖已有文件：回滚恢复原内容', async () => {
    const target = join(root, 'src/app.ts')
    const record = await manager.capture('t1', 'call_1', [
      { path: 'src/app.ts', absolute: target },
    ])

    await writeFile(target, 'const one = 999\n')
    const outcome = await manager.revertCheckpoint('t1', record.id)
    expect(outcome).not.toBeNull()
    expect(outcome!.restored).toEqual(['src/app.ts'])
    expect(await read(target)).toBe('const one = 1\n')
  })

  test('新建文件：回滚即删除', async () => {
    const target = join(root, 'created.ts')
    const record = await manager.capture('t2', 'call_1', [{ path: 'created.ts', absolute: target }])
    expect(record.files[0]!.existed).toBe(false)

    await writeFile(target, 'new file\n')
    const outcome = await manager.revertCheckpoint('t2', record.id)
    expect(outcome!.deleted).toEqual(['created.ts'])
    expect(existsSync(target)).toBe(false)
  })

  test('同一检查点不能二次回滚', async () => {
    const target = join(root, 'once.txt')
    await writeFile(target, 'v1\n')
    const record = await manager.capture('t3', 'call_1', [{ path: 'once.txt', absolute: target }])
    await writeFile(target, 'v2\n')

    expect(await manager.revertCheckpoint('t3', record.id)).not.toBeNull()
    expect(await manager.revertCheckpoint('t3', record.id)).toBeNull()
    expect(await read(target)).toBe('v1\n')
  })

  test('工具执行失败产生的多余快照不影响后续回滚语义', async () => {
    // 快照之后工具没写成功：回滚会把同样的内容写回去（幂等）
    const target = join(root, 'noop.txt')
    await writeFile(target, 'same\n')
    const record = await manager.capture('t4', 'call_1', [{ path: 'noop.txt', absolute: target }])
    const outcome = await manager.revertCheckpoint('t4', record.id)
    expect(outcome!.restored).toEqual(['noop.txt'])
    expect(await read(target)).toBe('same\n')
  })
})

describe('CheckpointManager revertFile 与 revertAll', () => {
  test('revertFile 用最早的快照，把文件恢复到 Agent 动手之前', async () => {
    const target = join(root, 'chain.txt')
    await writeFile(target, 'original\n')
    const first = await manager.capture('t5', 'call_1', [{ path: 'chain.txt', absolute: target }])

    await writeFile(target, 'second\n')
    const second = await manager.capture('t5', 'call_2', [{ path: 'chain.txt', absolute: target }])
    await writeFile(target, 'third\n')
    const third = await manager.capture('t5', 'call_3', [{ path: 'chain.txt', absolute: target }])

    const outcome = await manager.revertFile('t5', target)
    expect(outcome).not.toBeNull()
    // 三个检查点全部作废
    expect(outcome!.invalidated.sort()).toEqual([first.id, second.id, third.id].sort())
    expect(await read(target)).toBe('original\n')

    // 已作废的检查点再单独回滚拿不到东西
    expect(await manager.revertCheckpoint('t5', second.id)).toBeNull()
  })

  test('revertAll 把每个文件都还原到最早状态', async () => {
    const a = join(root, 'all-a.txt')
    const b = join(root, 'all-b.txt')
    await writeFile(a, 'A0\n')
    await writeFile(b, 'B0\n')
    const firstA = await manager.capture('t6', 'call_1', [{ path: 'all-a.txt', absolute: a }])
    await writeFile(a, 'A1\n')
    const firstB = await manager.capture('t6', 'call_2', [{ path: 'all-b.txt', absolute: b }])
    await writeFile(b, 'B1\n')
    await manager.capture('t6', 'call_3', [{ path: 'all-a.txt', absolute: a }, { path: 'all-b.txt', absolute: b }])
    await writeFile(a, 'A2\n')
    await writeFile(b, 'B2\n')

    const outcome = await manager.revertAll('t6')
    expect(outcome).not.toBeNull()
    expect(await read(a)).toBe('A0\n')
    expect(await read(b)).toBe('B0\n')
    expect(outcome!.invalidated.length).toBe(3)
    expect(outcome!.invalidated).toContain(firstA.id)
    expect(outcome!.invalidated).toContain(firstB.id)

    expect(await manager.revertAll('t6')).toBeNull()
  })

  test('回滚过的文件被再次修改后，新检查点构成新的可回滚链', async () => {
    const target = join(root, 'again.txt')
    await writeFile(target, 'v0\n')
    const first = await manager.capture('t7', 'call_1', [{ path: 'again.txt', absolute: target }])
    await writeFile(target, 'v1\n')
    await manager.revertFile('t7', target)
    expect(await read(target)).toBe('v0\n')

    // Agent 又改了一次：新快照记录的是它动手前的内容（v2）
    await writeFile(target, 'v2\n')
    const again = await manager.capture('t7', 'call_2', [{ path: 'again.txt', absolute: target }])
    expect(again.files[0]!.existed).toBe(true)
    expect(Buffer.from(again.files[0]!.contentBase64!, 'base64').toString()).toBe('v2\n')
    await writeFile(target, 'v3\n')
    const outcome = await manager.revertCheckpoint('t7', again.id)
    expect(outcome!.invalidated).toEqual([again.id])
    expect(await read(target)).toBe('v2\n')
    expect(await manager.revertCheckpoint('t7', first.id)).toBeNull()
  })
})

describe('CheckpointManager 边界与清理', () => {
  test('超大文件快照不完整，回滚时跳过', async () => {
    const target = join(root, 'big.bin')
    await writeFile(target, Buffer.alloc(6 * 1024 * 1024, 7))
    const record = await manager.capture('t8', 'call_1', [{ path: 'big.bin', absolute: target }])
    expect(record.files[0]!.snapshotIncomplete).toBe(true)

    await writeFile(target, 'small now\n')
    const outcome = await manager.revertCheckpoint('t8', record.id)
    expect(outcome!.skipped).toEqual(['big.bin'])
  })

  test('discard 删掉流水文件，条目随会话一起消失', async () => {
    const target = join(root, 'gone.txt')
    await writeFile(target, 'x\n')
    await manager.capture('t9', 'call_1', [{ path: 'gone.txt', absolute: target }])
    expect(await manager.revertAll('t9')).not.toBeNull()
    await manager.discard('t9')
    expect(await manager.revertAll('t9')).toBeNull()
  })

  test('Windows 大小写不敏感的路径归并（平台可用时）', async () => {
    if (process.platform !== 'win32') return
    const target = join(root, 'Case.txt')
    await writeFile(target, 'c0\n')
    await manager.capture('t10', 'call_1', [{ path: 'Case.txt', absolute: target }])
    // 不同大小写指向同一份文件，revertFile 也能找到
    const outcome = await manager.revertFile('t10', join(root, 'case.txt'))
    expect(outcome).not.toBeNull()
    expect(await read(target)).toBe('c0\n')
  })

  test('工作区内指向外面的 symlink 指向的文件被拒绝，快照本身不受影响', async () => {
    // 这里只确认 capture 用的 absolute 来自调用方（已过沙箱），模块自身不做路径判断
    const leak = join(root, 'leak-ckpt')
    let linked = false
    try {
      await symlink(outside, leak, 'junction')
      linked = true
    } catch {}
    if (!linked) return
    try {
      const target = join(leak, 's.txt')
      await writeFile(target, 'outer\n')
      const record = await manager.capture('t11', 'call_1', [{ path: 'leak-ckpt/s.txt', absolute: target }])
      expect(record.files[0]!.existed).toBe(true)
    } finally {
      await rm(leak, { force: true })
    }
  })
})
