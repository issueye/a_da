import { useEffect, useState } from 'react'

/**
 * 监听一条 CSS 媒体查询的匹配状态。
 *
 * 为什么不用 `window.innerWidth` + `resize` 监听：窗口缩放时 `resize` 每帧都可能触发，
 * 而媒体查询的 `change` 事件只在**跨越断点**时发一次——用它做"到窄了就把右栏收起来"
 * 这类**结构性**判断，不会在拖动窗口边缘的过程中反复重排。
 *
 * 初始值同步读取（`matchMedia().matches`），避免首帧按默认值渲染再闪一下。
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return false
    return window.matchMedia(query).matches
  })

  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return
    const mql = window.matchMedia(query)
    const onChange = (event: MediaQueryListEvent) => setMatches(event.matches)
    setMatches(mql.matches)
    mql.addEventListener('change', onChange)
    return () => mql.removeEventListener('change', onChange)
  }, [query])

  return matches
}
