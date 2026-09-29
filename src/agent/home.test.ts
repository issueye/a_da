/**
 * 公共区：a-da 自带的工作区。
 *
 * 全部是注入式验算，**不碰进程级的 `A_DA_HOME`**：那是并发跑在同一进程里的各个
 * 测试文件共享的变量，动它会把别人的会话目录也换掉（AGENTS.md §13 末尾）。
 */

import { describe, expect, test } from 'bun:test'
import { join } from 'node:path'
import { PUBLIC_WORKSPACE_LABEL, isPublicWorkspace, publicWorkspaceOf, sameWorkspacePath, workspaceLabel } from './home'

const HOME = join('E:', 'Codes', 'a-da-home')
const PUBLIC = publicWorkspaceOf(HOME)

describe('public workspace', () => {
  test('lives under the app home, so a test home never touches the real one', () => {
    expect(PUBLIC).toBe(join(HOME, 'workspace'))
  })

  test('recognises its own path in either separator and case', () => {
    expect(isPublicWorkspace(PUBLIC, PUBLIC)).toBe(true)
    // 同一个路径会以两种分隔符、两种大小写出现：都算公共区。
    expect(isPublicWorkspace(PUBLIC.toUpperCase(), PUBLIC)).toBe(true)
    expect(isPublicWorkspace(PUBLIC.replace(/[\\/]+/g, '\\'), PUBLIC)).toBe(true)
    // 结尾多一个分隔符不算另一个工作区。
    expect(isPublicWorkspace(`${PUBLIC}/`, PUBLIC)).toBe(true)
  })

  test('a project of the same name elsewhere is not the public workspace', () => {
    expect(isPublicWorkspace(join('E:', 'workspace'), PUBLIC)).toBe(false)
    expect(isPublicWorkspace(join(HOME, 'workspace-other'), PUBLIC)).toBe(false)
    // 别的应用数据目录下的 workspace 也不该认成自己的公共区。
    expect(isPublicWorkspace(publicWorkspaceOf(join('E:', 'other-home')), PUBLIC)).toBe(false)
  })

  test('path comparison ignores separators and case', () => {
    expect(sameWorkspacePath('E:/Codes/a_da', 'e:\\codes\\A_DA')).toBe(true)
    expect(sameWorkspacePath('E:/Codes/a_da', 'E:/Codes/a_da/')).toBe(true)
    expect(sameWorkspacePath('E:/Codes/a_da', 'E:/Codes/other')).toBe(false)
  })

  test('the label hides the implementation path', () => {
    expect(workspaceLabel(PUBLIC, PUBLIC)).toBe(PUBLIC_WORKSPACE_LABEL)
    // 普通项目仍然是短路径。
    expect(workspaceLabel('E:/Codes/rust_projects/a_da', PUBLIC)).toBe('_rust_projects/a_da')
  })
})
