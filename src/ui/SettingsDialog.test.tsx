/**
 * The settings dialog.
 *
 * It writes a real config file, so `A_DA_CONFIG` points at a temp path: the
 * test must never touch the user's `~/.a-da/config.json`.
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

let dir = ''
let configFile = ''

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'a-da-settings-'))
  configFile = join(dir, 'config.json')
  process.env.A_DA_CONFIG = configFile
  delete process.env.A_DA_API_KEY
  delete process.env.A_DA_MODEL
  delete process.env.A_DA_BASE_URL
})

afterAll(async () => {
  delete process.env.A_DA_CONFIG
  if (dir) await rm(dir, { recursive: true, force: true })
})

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  return { app, screen: () => renderer.getPaintedText().join('\n') }
}

describeNative('settings', () => {
  test(
    'opens from the sidebar with a header, a rule and a two-column body',
    async () => {
      const { app, screen } = await mount()
      expect(screen()).not.toContain('设置')

      await app.getByTestId('open-settings').click()
      const painted = screen()
      expect(painted).toContain('设置')
      expect(painted).toContain('供应商')
      expect(painted).toContain('工作区')
      expect(painted).toContain('接口地址')
      expect(painted).toContain('API Key')
      expect(painted).toContain('模型')
      expect(painted).toContain('保存')
      expect(painted).toContain('测试连接')

      await app.getByTestId('settings-close').click()
      expect(screen()).not.toContain('测试连接')

      await app.close()
    },
    20_000,
  )

  test(
    'the left column switches the section',
    async () => {
      const { app, screen } = await mount()
      await app.getByTestId('open-settings').click()

      await app.getByTestId('settings-nav-workspace').click()
      expect(screen()).toContain('当前项目')
      expect(screen()).toContain('list_files')
      expect(screen()).not.toContain('测试连接')

      await app.getByTestId('settings-nav-provider').click()
      expect(screen()).toContain('测试连接')

      await app.getByTestId('settings-close').click()
      await app.close()
    },
    20_000,
  )

  test(
    'a preset fills the address and the model, and saving writes the file',
    async () => {
      const { app, screen } = await mount()
      await app.getByTestId('open-settings').click()

      await app.getByTestId('settings-preset-deepseek').click()
      await app.getByTestId('settings-api-key').fill('sk-test-123')
      await app.getByTestId('settings-save').click()
      await app.getByText('已保存，下一轮对话生效').waitFor({ timeoutMs: 10_000 })

      const saved = JSON.parse(await readFile(configFile, 'utf8'))
      expect(saved).toEqual({
        baseUrl: 'https://api.deepseek.com/v1',
        apiKey: 'sk-test-123',
        model: 'deepseek-chat',
      })

      // Saving must not disturb the rest of the window.
      expect(screen()).toContain('新会话')

      await app.getByTestId('settings-close').click()
      await app.close()
    },
    20_000,
  )

  test(
    'a hand-written key the dialog does not edit survives a save',
    async () => {
      await writeFile(
        configFile,
        JSON.stringify({ apiKey: 'sk-old', model: 'old-model', extra: 'keep-me' }),
        'utf8',
      )
      const { app } = await mount()
      await app.getByTestId('open-settings').click()
      await app.getByTestId('settings-model').fill('new-model')
      await app.getByTestId('settings-save').click()
      await app.getByText('已保存，下一轮对话生效').waitFor({ timeoutMs: 10_000 })

      const saved = JSON.parse(await readFile(configFile, 'utf8'))
      expect(saved.model).toBe('new-model')
      expect(saved.extra).toBe('keep-me')

      await app.getByTestId('settings-close').click()
      await app.close()
    },
    20_000,
  )
})
