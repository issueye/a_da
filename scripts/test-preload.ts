/**
 * 测试套件的前置脚本（见 bunfig.toml 的 [test] preload）。
 *
 * 会话会真的落盘，所以先把应用数据目录挪到临时目录，测试不该往用户真实的
 * `~/.a-da` 里写东西——和 `A_DA_CONFIG` 让测试避开真实配置是同一个道理。
 * 目录选择弹窗同理：`A_DA_NO_DIALOG=1` 让它永远不去开真窗口，否则一个模态
 * 对话框就能把整个测试挂在那里。
 *
 * 这件事只能在这里做：`defaultSessionManager` 是模块级单例，`beforeAll` 跑的时候
 * 它早就被创建了。
 */

import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll } from 'bun:test'

const home = mkdtempSync(join(tmpdir(), 'a-da-test-home-'))
process.env.A_DA_HOME = home
process.env.A_DA_NO_DIALOG = '1'

function cleanup(): void {
  try {
    rmSync(home, { recursive: true, force: true })
  } catch {
    // 清理失败不该影响测试结果
  }
}

// 在 preload 里注册的钩子跑在所有测试文件之后。
afterAll(cleanup)
// 兜底：运行器没走钩子时（例如中途崩溃）也别留下空目录。
process.on('exit', cleanup)
