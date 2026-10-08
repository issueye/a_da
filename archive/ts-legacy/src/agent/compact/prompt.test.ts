import { describe, expect, test } from 'bun:test'
import {
  NO_TOOLS_PREAMBLE,
  NO_TOOLS_TRAILER,
  buildCompactPrompt,
  formatCompactSummary,
  buildCompactSummaryMessage,
} from './prompt'

describe('会话压缩提示词与格式化引擎', () => {
  test('buildCompactPrompt 包含强文本无工具约束、9 大核心板块以及自定义要求', () => {
    const promptWithoutCustom = buildCompactPrompt()
    expect(promptWithoutCustom).toContain('CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.')
    expect(promptWithoutCustom).toContain('1. Primary Request and Intent:')
    expect(promptWithoutCustom).toContain('2. Key Technical Concepts:')
    expect(promptWithoutCustom).toContain('3. Files and Code Sections:')
    expect(promptWithoutCustom).toContain('4. Errors and fixes:')
    expect(promptWithoutCustom).toContain('5. Problem Solving:')
    expect(promptWithoutCustom).toContain('6. All user messages:')
    expect(promptWithoutCustom).toContain('7. Pending Tasks:')
    expect(promptWithoutCustom).toContain('8. Current Work:')
    expect(promptWithoutCustom).toContain('9. Optional Next Step:')
    expect(promptWithoutCustom).toContain(NO_TOOLS_TRAILER)

    const promptWithCustom = buildCompactPrompt('请重点关注 Rust 桥接和 UI 样式')
    expect(promptWithCustom).toContain('Additional Instructions:')
    expect(promptWithCustom).toContain('请重点关注 Rust 桥接和 UI 样式')
  })

  test('formatCompactSummary 正确剥离 <analysis> 思考标签并提取 <summary> 正文', () => {
    const rawResponse = `
<analysis>
Chronological analysis:
- Step 1: User requested feature X.
- Step 2: Implemented file A.
- Double-check complete.
</analysis>

<summary>
1. Primary Request and Intent:
   用户要求实现会话压缩功能

2. Key Technical Concepts:
   - 结构化总结
   - 上下文预算控制

3. Files and Code Sections:
   - src/agent/compact/runner.ts: 核心执行器

9. Optional Next Step:
   接入 Composer 一键交互按钮
</summary>
`

    const formatted = formatCompactSummary(rawResponse)
    expect(formatted).not.toContain('<analysis>')
    expect(formatted).not.toContain('Chronological analysis:')
    expect(formatted).not.toContain('<summary>')
    expect(formatted).not.toContain('</summary>')
    expect(formatted).toContain('1. Primary Request and Intent:\n   用户要求实现会话压缩功能')
    expect(formatted).toContain('9. Optional Next Step:\n   接入 Composer 一键交互按钮')
  })

  test('formatCompactSummary 容错处理未包含标签的纯文本输出', () => {
    const rawPlain = '1. Primary Request:\nFix bug\n\n2. Key Concepts:\nTypes'
    expect(formatCompactSummary(rawPlain)).toBe(rawPlain)
    expect(formatCompactSummary(undefined)).toBe('')
  })

  test('buildCompactSummaryMessage 组装标准 continuation 引导消息', () => {
    const summary = '1. Primary Intent: 测试项目压缩'
    const msgWithoutPreserved = buildCompactSummaryMessage(summary)
    expect(msgWithoutPreserved).toContain('This session is being continued from a previous conversation')
    expect(msgWithoutPreserved).toContain(summary)
    expect(msgWithoutPreserved).not.toContain('Recent messages are preserved verbatim.')

    const msgWithPreserved = buildCompactSummaryMessage(summary, { recentMessagesPreserved: true })
    expect(msgWithPreserved).toContain('Recent messages are preserved verbatim.')
  })
})
