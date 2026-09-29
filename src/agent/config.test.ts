/**
 * 配置文件的两条不变量：写入不会丢键，读到的永远不会是半个文件。
 *
 * The window saves the appearance without being awaited, so an appearance write
 * and a provider save genuinely overlap in normal use. Both are read-modify-write
 * of one whole-file JSON document, which is why they are serialized and why the
 * bytes go to a sibling and get renamed: a truncated read would look like "no
 * config" to `readSavedConfig`, and the next write would then drop the API key.
 *
 * `A_DA_CONFIG` points at a temp file: these must never touch the real one.
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  formatHeadersText,
  parseHeadersText,
  readSavedAppearance,
  readSavedConfig,
  writeSavedAppearance,
  writeSavedConfig,
} from './config'

let dir = ''
let configFile = ''

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'a-da-config-'))
  configFile = join(dir, 'config.json')
  process.env.A_DA_CONFIG = configFile
})

afterAll(async () => {
  delete process.env.A_DA_CONFIG
  if (dir) await rm(dir, { recursive: true, force: true })
})

describe('the config file', () => {
  test('an overlapping appearance write and provider save keep both keys', async () => {
    await rm(configFile, { force: true })

    // 这就是应用里的真实顺序：切换明暗不 await，紧接着保存供应商设置。
    await Promise.all([
      writeSavedAppearance('dark'),
      writeSavedConfig({ baseUrl: 'https://api.deepseek.com/v1', apiKey: 'sk-keep', model: 'deepseek-chat' }),
    ])

    const saved = await readSavedConfig()
    expect(saved.appearance).toBe('dark')
    expect(saved.apiKey).toBe('sk-keep')
    expect(saved.model).toBe('deepseek-chat')
  })

  test('a reader never sees a half-written file', async () => {
    await rm(configFile, { force: true })
    await writeSavedConfig({ apiKey: 'sk-1' })

    let unparseable = 0
    let missingKey = 0
    const writers = Array.from({ length: 10 }, (_, index) =>
      writeSavedConfig({ apiKey: `sk-${index}` }).catch(() => {}),
    )
    const readers = (async () => {
      for (let i = 0; i < 80; i++) {
        // 直接读字节：ENOENT 表示「还没建好」，那是合法状态，不是撕裂。
        const raw = await readFile(configFile, 'utf8').catch((error: NodeJS.ErrnoException) =>
          error.code === 'ENOENT' ? undefined : '',
        )
        if (raw !== undefined) {
          try {
            if (typeof JSON.parse(raw).apiKey !== 'string') missingKey += 1
          } catch {
            unparseable += 1
          }
        }
        await new Promise((resolve) => setTimeout(resolve, 1))
      }
    })()

    await Promise.all([...writers, readers])
    expect(unparseable).toBe(0)
    expect(missingKey).toBe(0)
  })

  test('a hand-written key the dialog does not edit survives every write', async () => {
    await writeFile(configFile, JSON.stringify({ extra: 'keep-me', apiKey: 'sk-old' }), 'utf8')

    await writeSavedAppearance('dark')
    await writeSavedConfig({ model: 'new-model' })

    const saved = await readSavedConfig()
    expect(saved.extra).toBe('keep-me')
    expect(saved.apiKey).toBe('sk-old')
    expect(saved.appearance).toBe('dark')
    expect(saved.model).toBe('new-model')
  })

  test('a nonsense appearance value reads as "never chosen"', async () => {
    await writeFile(configFile, JSON.stringify({ appearance: 'chartreuse' }), 'utf8')
    expect(readSavedAppearance()).toBeNull()
  })
})

describe('自定义请求头的文本解析（设置弹窗的输入格式）', () => {
  test('分号分隔多个头，按第一个冒号切分（值里可以带冒号）', () => {
    expect(parseHeadersText('X-A: 1; X-Url: https://api.example.com/v1')).toEqual({
      'X-A': '1',
      'X-Url': 'https://api.example.com/v1',
    })
  })

  test('也接受换行分隔（config.json 与环境变量里可以一行一个）', () => {
    expect(parseHeadersText('X-A: 1\nX-B: 2\r\nX-C: 3')).toEqual({ 'X-A': '1', 'X-B': '2', 'X-C': '3' })
  })

  test('没有冒号、空名字、空值的项被丢弃', () => {
    expect(parseHeadersText('乱写的; X-A: 1; : 空名字; X-Empty:')).toEqual({ 'X-A': '1' })
  })

  test('解析不出任何一项时返回 undefined（空对象会让 envOverrides 误报）', () => {
    expect(parseHeadersText('')).toBeUndefined()
    expect(parseHeadersText('   ')).toBeUndefined()
    expect(parseHeadersText('没有冒号')).toBeUndefined()
    expect(parseHeadersText(undefined)).toBeUndefined()
    expect(parseHeadersText(42)).toBeUndefined()
  })

  test('格式化与解析互为逆运算（弹窗回填依赖它）', () => {
    const source = { 'X-A': '1', Authorization: 'Api-Key t' }
    expect(parseHeadersText(formatHeadersText(source))).toEqual(source)
  })

  test('undefined / 空对象格式化成空串', () => {
    expect(formatHeadersText(undefined)).toBe('')
    expect(formatHeadersText({})).toBe('')
  })
})
