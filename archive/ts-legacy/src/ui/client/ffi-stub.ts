/**
 * 非 Bun 运行时（原生宿主 / Web）下的 bun:ffi 空桩
 */
export const dlopen = () => null
export const FFIType = {
  function: 0,
  i64: 1,
  i32: 2,
  u32: 3,
  i16: 4,
  bool: 5,
  ptr: 6,
}
export class JSCallback {
  ptr = 0
  constructor(_fn: any, _def: any) {}
  close() {}
}
