/**
 * 插件管理弹窗测试
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

let dir = ''

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'a-da-plugins-test-'))
  process.env.A_DA_CONFIG = join(dir, 'config.json')
  store.newThread(dir)
})

afterAll(async () => {
  delete process.env.A_DA_CONFIG
  if (dir) await rm(dir, { recursive: true, force: true })
})

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  const screen = () => renderer.getPaintedText().join('\n')
  const painted = async (needle: string, timeoutMs = 10_000) => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 40))
    }
    throw new Error(`never painted ${needle}\n${screen()}`)
  }
  const gone = async (needle: string, timeoutMs = 10_000) => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (!screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 40))
    }
    throw new Error(`still paints ${needle}\n${screen()}`)
  }
  return { app, screen, renderer, painted, gone }
}

describeNative('plugins dialog', () => {
  test('opens from sidebar plug button and shows tabs and header', async () => {
    const { app, screen, painted } = await mount()
    expect(screen()).not.toContain('插件管理')

    // 点击侧边栏底部插头按钮打开插件弹窗
    await app.getByTestId('open-plugins').click()
    await painted('插件管理')
    const text = screen()

    expect(text).toContain('插件管理')
    expect(text).toContain('扩展 Agent 工具库与自动化能力')
    expect(text).toContain('提示词管理')
    expect(text).toContain('工作区插件')
    expect(text).toContain('全局插件')
    expect(text).toContain('内置核心工具')
    expect(text).toContain('新建插件')
    expect(text).toContain('刷新')

    // 点击关闭按钮
    await app.getByTestId('plugins-close').click()
    expect(screen()).not.toContain('扩展 Agent 工具库与自动化能力')

    await app.close()
  })

  test('switches to builtins tab and displays all core builtin tools', async () => {
    const { app, screen, painted } = await mount()
    await app.getByTestId('open-plugins').click()
    await painted('插件管理')

    // 切换到内置核心工具选项卡
    await app.getByTestId('plugins-nav-builtins').click()
    await painted('核心内置工具')
    const text = screen()

    expect(text).toContain('核心内置工具（系统预装）')
    expect(text).toContain('list_files')
    expect(text).toContain('read_file')
    expect(text).toContain('search_files')
    expect(text).toContain('write_file')
    expect(text).toContain('edit_file')
    expect(text).toContain('run_command')
    expect(text).toContain('todo')
    expect(text).toContain('只读安全')
    expect(text).toContain('需审批写入')

    await app.getByTestId('plugins-close').click()
    await app.close()
  })

  test('displays prompts management tab and allows toggling and previewing builtin prompts', async () => {
    const { app, screen, painted } = await mount()
    await app.getByTestId('open-plugins').click()
    await painted('插件管理')

    // 切换到提示词管理标签页
    await app.getByTestId('plugins-nav-prompts').click()
    await painted('中文专业编码规范')

    expect(screen()).toContain('提示词管理')
    expect(screen()).toContain('中文专业编码规范')
    expect(screen()).toContain('深度代码审查')
    expect(screen()).toContain('单元测试生成器')
    expect(screen()).toContain('Git 语义化提交助手')
    expect(screen()).toContain('已启用')
    expect(screen()).toContain('内置预装')

    // 展开预览正文
    await app.getByTestId('prompt-expand-builtin-chinese-coding-standards').click()
    await painted('所有的对话沟通、架构解释、思路说明与代码注释均使用专业')

    // 切换启用/停用
    await app.getByTestId('prompt-toggle-builtin-chinese-coding-standards').click()
    await painted('已停用')

    // 再次切换恢复启用
    await app.getByTestId('prompt-toggle-builtin-chinese-coding-standards').click()
    await painted('已启用')

    await app.getByTestId('plugins-close').click()
    await app.close()
  }, 30_000)

  test(
    'creates custom prompt and applies prompt content to composer',
    async () => {
      const { app, screen, painted, gone } = await mount()
      await app.getByTestId('open-plugins').click()
      await painted('插件管理')

      // 切换到提示词管理标签页
      await app.getByTestId('plugins-nav-prompts').click()
      await painted('新建提示词')

      // 点击新建提示词
      await app.getByTestId('prompt-create-btn').click()
      await painted('快速新建提示词模板 (.md)')

      // 填写名称与正文
      await app.getByTestId('prompt-name-input').fill('ui_review')
      await app.getByTestId('prompt-desc-input').fill('重点检查组件拆分与无障碍支持')
      await app.getByTestId('prompt-content-input').fill('请审查前端组件的可访问性与重渲染性能。')

      // 提交创建
      await app.getByTestId('prompt-submit-btn').click()
      await gone('快速新建提示词模板 (.md)')
      expect(screen()).toContain('ui_review')
      expect(screen()).toContain('重点检查组件拆分与无障碍支持')

      // 点击应用到输入框：弹窗自动关闭且内容进入 Composer
      await app.getByTestId('prompt-apply-workspace_ui_review').click()
      await gone('插件管理')

      // 点击发送按钮以验证草稿已成功填入 Composer 并发送上屏
      await app.getByTestId('send').click()
      await painted('请审查前端组件的可访问性与重渲染性能。')

      await app.close()
    },
    30_000,
  )
})
