/**
 * 明暗切换：按钮、颜色、以及「下次启动还记得」。
 *
 * 这里断言的是状态和调色板，不是像素——GPU 渲染器测试拿到的是文字和边框，
 * 颜色对它是不可见的。真正需要钉住的是：点一下会改 `C`（界面读的就是它），
 * 而且这个选择会落盘、下次启动能装回来。
 *
 * 配置写到 `A_DA_CONFIG` 指的临时文件里，测试不碰用户真实的 `~/.a-da`。
 */

import { afterAll, afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { AgentStore, store } from '../agent/store'
import { readSavedAppearance } from '../agent/config'
import { applyAppearance, C } from '../theme'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

let dir = ''
let configFile = ''

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'a-da-appearance-'))
  configFile = join(dir, 'config.json')
  process.env.A_DA_CONFIG = configFile
})

afterAll(async () => {
  delete process.env.A_DA_CONFIG
  if (dir) await rm(dir, { recursive: true, force: true })
})

/** 每个用例从浅色开始，并且把盘上的选择清掉，免得互相影响。 */
afterEach(async () => {
  applyAppearance('light')
  store.appearance = 'light'
  await rm(configFile, { force: true })
})

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  const screen = () => renderer.getPaintedText().join('\n')
  /** Every aria-label in the painted frame — how an icon-only button is named. */
  const labels = (): string[] =>
    Object.values(renderer.getA11yTree().nodes ?? {}).map((node) => node.aria?.label ?? '')
  return { app, screen, labels }
}

describeNative('appearance toggle', () => {
  test(
    'the caption strip carries the toggle, labelled for what it does',
    async () => {
      const { app, labels } = await mount()

      // 浅色下按钮叫「切换到深色模式」，也就是点下去会变成什么。
      const toggle = app.getByTestId('toggle-appearance')
      expect(await toggle.count()).toBe(1)
      expect(labels()).toContain('切换到深色模式')

      await toggle.click()

      // 点完反过来：图标和标签一起说的是同一件事。
      expect(store.appearance).toBe('dark')
      expect(labels()).toContain('切换到浅色模式')
      expect(labels()).not.toContain('切换到深色模式')

      await toggle.click()
      expect(store.appearance).toBe('light')
      expect(labels()).toContain('切换到深色模式')

      await app.close()
    },
    30_000,
  )

  test(
    'switching swaps the palette the UI paints from',
    async () => {
      const { app } = await mount()
      const light = { canvas: C.canvas, text: C.text, sidebar: C.sidebar }

      await app.getByTestId('toggle-appearance').click()

      expect(C.canvas).not.toBe(light.canvas)
      expect(C.text).not.toBe(light.text)
      expect(C.sidebar).not.toBe(light.sidebar)

      await app.close()
    },
    30_000,
  )

  test(
    'the choice is written to the config file and survives a provider save',
    async () => {
      const { app } = await mount()
      await app.getByTestId('toggle-appearance').click()

      // 落盘是异步的，等它真的写完。
      const started = Date.now()
      while (Date.now() - started < 10_000 && readSavedAppearance() !== 'dark') {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      expect(readSavedAppearance()).toBe('dark')

      // 供应商设置走的是同一个文件，保存它不能把明暗选择冲掉。
      await store.saveProvider({ baseUrl: 'https://api.deepseek.com/v1', apiKey: 'sk-x', model: 'deepseek-chat' })
      const saved = JSON.parse(await readFile(configFile, 'utf8'))
      expect(saved.appearance).toBe('dark')
      expect(saved.model).toBe('deepseek-chat')

      await app.close()
    },
    30_000,
  )

  test(
    'a saved choice is installed when the store starts',
    async () => {
      await store.saveProvider({ baseUrl: '', apiKey: '', model: '' })
      store.setAppearance('dark')
      // 上面那次写入是异步的，等它落地再新起一个 store。
      const started = Date.now()
      while (Date.now() - started < 10_000 && readSavedAppearance() !== 'dark') {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }

      // 回到浅色，模拟「刚启动」的状态。
      applyAppearance('light')
      const fresh = new AgentStore(process.cwd())

      const until = Date.now()
      while (Date.now() - until < 10_000 && fresh.appearance !== 'dark') {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }

      expect(fresh.appearance).toBe('dark')
      expect(C.canvas).toBe('#1C1D1F')
    },
    30_000,
  )

  test(
    'a saved choice is installed before the first frame, not after it',
    async () => {
      // 深色用户每次启动都先闪一下白是可见的缺陷，所以读盘必须是同步的：
      // `new AgentStore` 一返回，调色板就该是选过的那个。
      await store.saveProvider({ baseUrl: '', apiKey: '', model: '' })
      store.setAppearance('dark')
      const started = Date.now()
      while (Date.now() - started < 10_000 && readSavedAppearance() !== 'dark') {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }

      applyAppearance('light')
      const fresh = new AgentStore(process.cwd())

      // 没有任何等待：构造完就已经是深色。等待之后才对，说明它是异步装上的。
      expect(fresh.appearance).toBe('dark')
      expect(C.canvas).toBe('#1C1D1F')
    },
    30_000,
  )

  test(
    'a choice made right after startup is not undone by anything',
    async () => {
      // 盘上是浅色，用户在新会话刚起来时就点了深色，之后不能被任何异步任务冲掉。
      await store.saveProvider({ baseUrl: '', apiKey: '', model: '' })
      store.setAppearance('light')
      const started = Date.now()
      while (Date.now() - started < 10_000 && readSavedAppearance() !== 'light') {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }

      applyAppearance('light')
      const fresh = new AgentStore(process.cwd())
      fresh.setAppearance('dark')

      // 给任何可能迟到的启动任务充足的时间来捣乱。
      await new Promise((resolve) => setTimeout(resolve, 600))

      expect(fresh.appearance).toBe('dark')
      expect(C.canvas).toBe('#1C1D1F')
    },
    30_000,
  )
})
