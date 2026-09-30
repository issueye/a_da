/**
 * 全局快捷键层与命令面板的共享定义。
 *
 * GPUX 在 render() 选项上暴露窗口级 onKeyDown（冒泡相：未被聚焦元素消费的
 * 组合键会落到这里），这就是命令面板与全局快捷键的挂点。payload 形如
 * { key, keyChar, isHeld, modifiers: { shift, ctrl, alt, cmd } }。
 *
 * 规则：只响应带修饰键的组合键（裸键留给输入框），isHeld（按住重复）一律忽略；
 * 命令面板开着时只认 Ctrl+K 与 Escape。
 */

import type { AgentClient } from './client'

export interface ShortcutEvent {
  key?: string
  keyChar?: string
  isHeld?: boolean
  modifiers?: { shift?: boolean; ctrl?: boolean; alt?: boolean; cmd?: boolean }
}

export interface ShortcutSpec {
  id: string
  /** 展示用键位（命令面板里显示） */
  keys: string
  label: string
  description?: string
  /** 判定：key 为小写后的 event.key；mods 为归一化的修饰键 */
  match: (key: string, mods: { shift: boolean; ctrl: boolean; alt: boolean; cmd: boolean }) => boolean
  run: (client: AgentClient) => void
  /** 面板开着时是否仍然生效（默认 false） */
  activeWithPalette?: boolean
}

function normMods(event: ShortcutEvent): { shift: boolean; ctrl: boolean; alt: boolean; cmd: boolean } {
  const m = event?.modifiers ?? {}
  return { shift: Boolean(m.shift), ctrl: Boolean(m.ctrl) || Boolean(m.cmd), alt: Boolean(m.alt), cmd: Boolean(m.cmd) }
}

export const GLOBAL_SHORTCUTS: ShortcutSpec[] = [
  {
    id: 'palette',
    keys: 'Ctrl+K',
    label: '打开 / 关闭命令面板',
    description: '所有动作的快捷入口',
    match: (key) => key === 'k',
    run: (client) => client.ui.setPaletteOpen(!client.state.paletteOpen),
    activeWithPalette: true,
  },
  {
    id: 'new-thread',
    keys: 'Ctrl+T',
    label: '新建对话',
    match: (key, mods) => key === 't' && !mods.shift,
    run: (client) => {
      void client
        .request('thread.create', { workspace: client.state.project })
        .then(({ threadId }) => client.ui.openTab(threadId))
    },
  },
  {
    id: 'toggle-sidebar',
    keys: 'Ctrl+B',
    label: '显示 / 隐藏侧边栏',
    match: (key) => key === 'b',
    run: (client) => client.ui.toggleSidebar(),
  },
  {
    id: 'toggle-debug',
    keys: 'Ctrl+D',
    label: '显示 / 隐藏调试日志',
    match: (key) => key === 'd',
    run: (client) => client.ui.toggleDebug(),
  },
  {
    id: 'settings',
    keys: 'Ctrl+,',
    label: '打开设置',
    match: (key) => key === ',',
    run: (client) => client.ui.setSettings(!client.state.settingsOpen),
  },
  {
    id: 'plugins',
    keys: 'Ctrl+Shift+P',
    label: '打开插件管理',
    match: (key, mods) => key === 'p' && mods.shift,
    run: (client) => client.ui.setPlugins(!client.state.pluginsOpen),
  },
  {
    id: 'changes',
    keys: 'Ctrl+R',
    label: '打开 / 关闭改动审阅',
    description: '查看本会话的文件改动并支持恢复原状',
    match: (key) => key === 'r',
    run: (client) => client.ui.setChangesOpen(!client.state.changesOpen),
  },
  {
    id: 'close-tab',
    keys: 'Ctrl+W',
    label: '关闭当前标签',
    match: (key) => key === 'w',
    run: (client) => client.ui.closeTab(client.state.activeId),
  },
  {
    id: 'escape',
    keys: 'Esc',
    label: '关闭浮层',
    match: (key) => key === 'escape',
    run: (client) => {
      if (client.state.paletteOpen) client.ui.setPaletteOpen(false)
      else if (client.state.changesOpen) client.ui.setChangesOpen(false)
    },
    activeWithPalette: true,
  },
]

/**
 * 窗口级 keyDown 的统一入口。返回是否消费了这个事件（消费了的可以阻止
 * 后续默认处理——GPUX 目前没有 preventDefault，这里只做语义上的标记）。
 */
export function handleGlobalShortcut(event: ShortcutEvent, client: AgentClient): boolean {
  const key = String(event?.key ?? '').toLowerCase()
  if (!key || event?.isHeld) return false
  // 带输入的组合键（Ctrl+V 之类已在输入框处理的）不属于这里管；但窗口级的
  // V 没有别的用途，留给 Composer 自己的粘贴逻辑，这里直接不拦。
  const mods = normMods(event)
  if (!mods.ctrl) return false

  const paletteOpen = client.state.paletteOpen
  for (const shortcut of GLOBAL_SHORTCUTS) {
    if (paletteOpen && !shortcut.activeWithPalette) continue
    if (shortcut.match(key, mods)) {
      shortcut.run(client)
      return true
    }
  }
  return false
}
