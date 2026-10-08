/**
 * UI 生产打包的 in-process 客户端空桩
 *
 * 独立桌面运行时使用 WebSocket 连接原生 Host，不使用进程内直连派发。
 */
export function createInProcessClient() {
  return {
    state: {},
    ui: {},
    request: async () => ({}),
    send: () => {},
    onEvent: () => () => {},
    subscribe: () => () => {},
    close: () => {},
  } as any
}
