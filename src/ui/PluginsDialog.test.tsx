/**
 * 插件管理弹窗测试
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
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
  // 一个"缺必填配置"的插件：状态徽标与诊断那一组断言靠它（写在 beforeAll 里是为了
  // 不额外开窗口——GPU 测试渲染器开真窗口，两个窗口同时活着时按坐标派发的 click
  // 会落到另一个窗口上，实测会让标签栏那组用例集体翻红）
  const extensions = join(dir, '.ada', 'extensions')
  await mkdir(extensions, { recursive: true })
  await writeFile(
    join(extensions, 'needs-token.ts'),
    `export default {
  name: '需要密钥的插件',
  configSchema: { properties: { token: { type: 'string', title: '访问令牌', required: true } } },
  tools: [{
    name: 'needs_token_probe',
    description: '探针工具',
    parameters: { type: 'object' },
    async execute() {
      return { output: 'ok', ok: true }
    },
  }],
}
`,
    'utf-8'
  )
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
  /**
   * 把列表里的某个元素滚进可视区再交互。
   *
   * 弹窗主体是滚动容器：新建的提示词/技能排在十几个内置项之后，初始都在
   * 可视区外（实测 apply 按钮 y≈947 > 窗口 760）。文字仍会出现在 painted
   * 文本里，但 click() 按窗口坐标派发，点在窗口外等于没点——handler 不执行、
   * 也不报错。bounds() 是滚动感知的窗口坐标，滚到位后普通 click 即可命中。
   *
   * 方向约定（实测）：wheel deltaY 为**负**是向下滚；且滚到顶/底时同向滚轮
   * 是无操作——所以"连续无位移"也要翻向，不能只看 y 是否反向。
   */
  const scrollIntoView = async (
    locator: { bounds: () => Promise<{ y: number; height: number }> },
    maxSteps = 12,
  ) => {
    let dy = -300
    let idle = 0
    for (let i = 0; i < maxSteps; i++) {
      const b = await locator.bounds()
      // 可视区间取保守值：避开弹窗头部与底部按钮栏
      if (b.y > 180 && b.y + b.height < 680) return
      const before = b.y
      await app.mouse.wheel({ x: 560, y: 400 }, 0, dy)
      await new Promise((resolve) => setTimeout(resolve, 90))
      renderer.flush?.()
      const after = (await locator.bounds()).y
      if (Math.abs(after - before) < 1) {
        // 同向滚到头是无操作：翻向试另一边
        idle += 1
        if (idle >= 2) {
          dy = -dy
          idle = 0
        }
        continue
      }
      idle = 0
      if (after > before && dy < 0) dy = -dy // 越滚越远，反向
      if (after < before && dy > 0) dy = -dy
    }
    // 滚完仍不在区间内也不报错：让后续点击自己暴露问题
  }
  return { app, screen, renderer, painted, gone, scrollIntoView }
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
    expect(text).toContain('技能库')
    expect(text).toContain('子智能体')
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
    // 先看一眼工作区页：缺配置的插件要显示"待配置"徽标与原因（M3-1）。
    // 断言挂在已有用例里而不是新开一条：少开一个窗口就少一次上面注释里那种碰撞
    await app.getByTestId('plugins-nav-workspace').click()
    await painted('needs-token.ts')
    await painted('待配置')
    expect(screen()).toContain('缺少必填配置：token')
    expect(screen()).toContain('pluginConfig')

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
        const { app, screen, painted, gone, scrollIntoView } = await mount()
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

        // 新建的提示词排在内置项之后，多半在滚动容器可视区外，先滚进来再点
        const applyButton = app.getByTestId('prompt-apply-workspace_ui_review')
        await scrollIntoView(applyButton)

        // 点击应用到输入框：弹窗自动关闭且内容进入 Composer
        await applyButton.click()
        await gone('插件管理')

      // 点击发送按钮以验证草稿已成功填入 Composer 并发送上屏
      await app.getByTestId('send').click()
      await painted('请审查前端组件的可访问性与重渲染性能。')

      await app.close()
    },
    30_000,
  )

  test('switches to subagents tab and displays subagent profiles', async () => {
    const { app, screen, painted } = await mount()
    await app.getByTestId('open-plugins').click()
    await painted('插件管理')

    // 切换到子智能体选项卡
    await app.getByTestId('plugins-nav-subagents').click()
    await painted('代码调研专员')
    const text = screen()

    expect(text).toContain('子智能体拥有专属提示词、隔离上下文与工具白名单')
    expect(text).toContain('代码调研专员')
    expect(text).toContain('代码审查专家')
    expect(text).toContain('自动化测试专家')
    expect(text).toContain('researcher')
    expect(text).toContain('code_reviewer')
    expect(text).toContain('tester')
    expect(text).toContain('只读安全')
    expect(text).toContain('读写模式')
    expect(text).toContain('查看专属提示词')

    await app.getByTestId('plugins-close').click()
    await app.close()
  })

  test('switches to skills tab and allows creating and managing skills', async () => {
    const { app, screen, painted, gone, scrollIntoView } = await mount()
    await app.getByTestId('open-plugins').click()
    await painted('插件管理')

    // 切换到技能库选项卡
    await app.getByTestId('plugins-nav-skills').click()
    await painted('新建技能')
    expect(screen()).toContain('技能库')

    // 点击新建技能按钮展开表单
    await app.getByTestId('skill-create-btn').click()
    await painted('新建技能规范 (SKILL.md)')

    // 填入技能信息
    await app.getByTestId('skill-input-name').fill('rust-linter')
    await app.getByTestId('skill-input-desc').fill('自动化运行 cargo clippy 并分析警告')
    await app.getByTestId('skill-submit-create').click()

    // 创建成功后表单收起，列表中出现 rust-linter
    await gone('新建技能规范 (SKILL.md)')
    await painted('rust-linter')
    expect(screen()).toContain('自动化运行 cargo clippy 并分析警告')

    // 新建的技能排在既有技能之后，先滚进可视区再交互
    const expandButton = app.getByTestId('skill-expand-rust-linter')
    await scrollIntoView(expandButton)

    // 切换展开正文
    await expandButton.click()
    await painted('SKILL.md 正文指令')

    // 切换启停状态
    await app.getByTestId('skill-toggle-rust-linter').click()

    // 删除该技能（二次确认防误删）
    await app.getByTestId('skill-delete-rust-linter').click()
    await app.getByTestId('skill-delete-rust-linter').click()
    await gone('rust-linter')

    await app.getByTestId('plugins-close').click()
    await app.close()
  }, 30_000)
})
