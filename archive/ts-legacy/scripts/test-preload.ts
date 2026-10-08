/**
 * 测试套件的前置脚本（见 bunfig.toml 的 [test] preload）。
 *
 * 会话会真的落盘，所以先把应用数据目录挪到临时目录，测试不该往用户真实的
 * `~/.a-da` 里写东西——和 `A_DA_CONFIG` 让测试避开真实配置是同一个道理。
 * `A_DA_NO_DIALOG=1` 用来挡住"真会开系统窗口"的动作（打开资源管理器、系统通知）：
 * 一个模态对话框就能把整个测试挂在那里。
 *
 * 注意：**目录选择已经不是系统弹窗了**——改成了应用内的 `FilePicker`，数据来自主机 `fs.*`
 * （见 `docs/jsonrpc-protocol.md` §3.14）。所以那条路不需要这个开关，测试里也能真跑。
 *
 * 这件事只能在这里做：`defaultSessionManager` 是模块级单例，`beforeAll` 跑的时候
 * 它早就被创建了。
 */

import { mkdtempSync, rmSync } from 'node:fs'
import { rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll } from 'bun:test'

const home = mkdtempSync(join(tmpdir(), 'a-da-test-home-'))
process.env.A_DA_HOME = home
process.env.A_DA_NO_DIALOG = '1'

/**
 * 删临时目录，**失败时容忍**。
 *
 * 测试里的每个 `afterEach` 都会删自己去临时目录，而 Windows 上这很容易撞
 * `EBUSY: resource busy or locked`：应用里有些写入是刻意 fire-and-forget 的
 * （`newThread` 里的 `refresh()`、扩展加载器写状态），清理时那个句柄可能还没释放。
 *
 * 撞上时**不该让测试失败**——用例本身已经跑完并通过了，红的只是收尾。
 * 这与本文件末尾 `cleanup()` 的取向一致，只是那个只处理套件级的 home，
 * 各测试文件自己建的 home/workspace 需要同一个待遇。
 *
 * 真正的目录残留由操作系统在重启时清理，代价远小于一个随机翻红的测试套件。
 */
export async function cleanupTempDir(dir: string): Promise<void> {
  try {
    await rm(dir, { recursive: true, force: true })
  } catch {
    // 见上：清理失败不该影响测试结果
  }
}

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
