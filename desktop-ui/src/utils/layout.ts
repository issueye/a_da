/**
 * 布局常量与宽度换算（**单一真源**）。
 *
 * 🔴 这里的数字以前散在三处：`Sidebar.tsx` 的 `MIN/MAX/DEFAULT_SIDEBAR_WIDTH`、
 * `AgentContextPanel.tsx` 的 `w-[264px]`/`w-12`、以及 `electron/main.ts` 的窗口最小尺寸。
 * 三处各自演化 → 窗口缩到最小时主区被挤成 36px（两侧都是 `flex-shrink-0`，
 * 唯一能被压缩的只有 `main`），而且侧边栏宽度只在**拖动时**夹过一次，
 * 之后缩窗口不再重算。
 *
 * 现在统一成一张账：
 *
 * | 项 | 值 |
 * |---|---|
 * | 窗口最小宽度（`electron/main.ts`） | 860 |
 * | 对话主区最小可用宽度 | 420 |
 * | 右侧栏展开 / 收起 | 264 / 48 |
 * | 侧边栏默认 / 下限 / 上限 | 270 / 180 / 560 |
 *
 * 不变式：`侧边栏 + 右侧栏 + 主区最小宽度 ≤ 窗口宽`，违反时**先压侧边栏**，
 * 侧边栏压到下限还不够就把右侧栏变成覆盖式抽屉（见 `AgentContextPanel`）。
 */

/** 窗口最小宽度，与 `electron/main.ts` 的 `minWidth` 保持一致。 */
export const WINDOW_MIN_WIDTH = 860

/** 对话主区（消息流 + 输入框）的最小可用宽度。低于它遥测条会换行堆叠。 */
export const MAIN_MIN_WIDTH = 420

/** 宽度大于此值时右侧栏以"并排 dock"呈现，否则只能覆盖式展开。 */
export const RIGHT_DOCK_BREAKPOINT = 1100

/** 右侧栏展开宽度。 */
export const RIGHT_DOCK_WIDTH = 264

/** 两侧栏收起后的图标栏宽度（Sidebar 与 AgentContextPanel 共用）。 */
export const RAIL_WIDTH = 48

/**
 * 对话流的**最大阅读宽度**。
 *
 * 🔴 没有它的时候，助手正文会一路顶到容器右边缘（实测窗口 1439 宽时一行能排 90+ 个汉字）：
 * 眼睛从行尾折返到行首要扫过整屏，长回答读起来非常累；而且右上角的「任务规划」浮层
 * 正好压在这段超宽文字上。居中收窄之后，两侧留出的空档还能给浮卡让位。
 *
 * 840px 是按 13px 正文算的：约 60 个汉字一行，接近阅读舒适区上限；
 * 代码块与 diff 自带横向滚动，不会被这个宽度裁掉。
 */
export const TRANSCRIPT_MAX_WIDTH = 840

export const SIDEBAR_DEFAULT_WIDTH = 270
export const SIDEBAR_MIN_WIDTH = 180
export const SIDEBAR_MAX_WIDTH = 560

/** 侧边栏当前可用的最大宽度（按窗口宽与右栏占用算，保证主区不低于 `MAIN_MIN_WIDTH`）。 */
export function maxSidebarWidth(viewport: number): number {
  // 窄窗口下右栏只能是图标栏或覆盖式抽屉，占位固定 48
  const dock = viewport > RIGHT_DOCK_BREAKPOINT ? RIGHT_DOCK_WIDTH : RAIL_WIDTH
  return Math.max(
    SIDEBAR_MIN_WIDTH,
    Math.min(SIDEBAR_MAX_WIDTH, Math.floor(viewport - dock - MAIN_MIN_WIDTH))
  )
}

/** 把侧边栏宽度夹进合法区间（拖动、窗口缩放、读本地存储三处共用）。 */
export function clampSidebarWidth(width: number, viewport: number): number {
  const max = maxSidebarWidth(viewport)
  return Math.min(Math.max(Math.round(width), SIDEBAR_MIN_WIDTH), max)
}
