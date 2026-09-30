/**
 * 协议契约层（`src/shared/protocol/`）的统一出口。
 *
 * 这一层被 UI 与主机**两边**依赖，因此**只放类型与常量，不放任何实现**（协议设计 §7.1）。
 * 实现侧的依赖方向是「实现 → 契约」，永不反过来。
 */

export * from './dto'
export * from './methods'
export * from './errors'
