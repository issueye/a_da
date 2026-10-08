/**
 * a_da — 具备原生 GPU 加速窗口的本地 AI 编程助手。
 *
 * 开发运行：`bun --hot app.tsx`
 * 二进制构建：`bun run build`（产物仍是**一个** `dist/a-da.exe`）
 *
 * ## 这个文件是"单文件双角色"的分流点（协议 §1.8）
 *
 * | 启动方式 | 角色 |
 * |---|---|
 * | `a-da.exe`（无参数） | **UI**：起窗口 |
 * | `a-da.exe --host --port 0 --token <t>` | **主机**：只跑 agent，不开窗口 |
 *
 * 两个分支都用**动态 import**，这是刻意的：主机角色绝不能顺带加载渲染层
 * （`src/platform/init` 会初始化原生 addon，最坏情况是多出一个空窗口）。
 * 静态 import 做不到这一点——ESM 的 import 会先于任何分支判断执行。
 *
 * `scripts/protocol-boundary.test.ts` 里有一条断言盯着这个形状别被改回去。
 */

const isHostRole = process.argv.includes('--host')

/**
 * 分流入口。
 *
 * 用 async 函数包一层而不是顶层 await：打包成独立二进制后，入口模块的模块形态越简单越好
 * （顶层 await 要求这个文件是 ESM 模块，而它除此之外不需要任何 import/export）。
 */
async function main(): Promise<void> {
  if (isHostRole) {
    // 主机角色：不 import 任何 UI/渲染模块
    const { hostEntry } = await import('./src/agent/host/main')
    const { store } = await import('./src/agent/store')
    await hostEntry(process.argv.slice(2), { store })
    return
  }

  // UI 角色：平台引导、窗口、界面都在这里面（顺序照旧）
  await import('./src/ui/main')
}

void main()
