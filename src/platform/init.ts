/**
 * 跨平台启动前置初始化模块
 * 必须作为应用的首个静态导入引入，在加载任何原生 GUI 渲染器之前就绪。
 */

import { ensureValidStdHandles } from './win32'

// 在 Windows 环境下，确保底层 stdio 句柄指向合法设备（NUL），
// 避免 Rust 原生层向 stderr 输出日志时因 NULL/INVALID_HANDLE 引发 panic (os error 6) 闪退。
try {
  ensureValidStdHandles()
} catch {}
