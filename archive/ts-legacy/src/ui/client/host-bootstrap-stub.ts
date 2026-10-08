/**
 * UI 生产打包的 host-bootstrap 桩模块
 *
 * 在桌面独立运行态下，原生 Rust 宿主（a-da.exe）已预先派生无头 Agent 后端，
 * 并将通信端口和握手令牌注入到全局环境（__A_DA_HOST_PORT / A_DA_WS_PORT）。
 * UI 侧直接连接该端口，无需在前端二次调用 child_process.spawn。
 */
export async function spawnHostProcess() {
  const port = Number(
    process.env.A_DA_WS_PORT ||
    (globalThis as any).__A_DA_HOST_PORT ||
    60617
  )
  const token = String(
    process.env.A_DA_WS_TOKEN ||
    (globalThis as any).__A_DA_HOST_TOKEN ||
    ''
  )
  return {
    pid: process.pid,
    port,
    token,
    url: `ws://127.0.0.1:${port}/rpc`,
    stop: () => {},
    alive: true,
  }
}
