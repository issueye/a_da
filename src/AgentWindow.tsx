/**
 * The window: a title row, the sidebar, the conversation, and the task box.
 *
 * The state lives outside React because async turns keep writing to it while
 * the model streams, so this component only subscribes and re-renders.
 *
 * **M0 已完成**：这里只认 `client`（`ui/client`），不再有 `store` 这条线——
 * 全部子组件都通过同一个客户端接口读写。
 */

import React, { useEffect, useState } from 'react'
import { agentClient } from './ui/client'
import { Composer } from './ui/Composer'
import { DebugPanel } from './ui/DebugPanel'
import { PluginsDialog } from './ui/PluginsDialog'
import { SettingsDialog } from './ui/SettingsDialog'
import { ConfirmDialog } from './ui/ConfirmDialog'
import { CommandPalette } from './ui/CommandPalette'
import { Sidebar } from './ui/Sidebar'
import { EmptyConversationView } from './ui/EmptyConversationView'
import { TabStrip } from './ui/TabStrip'
import { TitleBar } from './ui/TitleBar'
import { Transcript } from './ui/Transcript'
import { C, FONT_SANS } from './theme'

/**
 * 订阅客户端状态变化并重渲染。
 *
 * M0 时 `client.subscribe` 就是 store 的那次广播；M1 起它由事件流驱动，
 * 这个组件的写法不用再改。
 */
function useAgentClient(): void {
  const [, setTick] = useState(0)
  useEffect(() => agentClient.subscribe(() => setTick((tick) => tick + 1)), [])
}

export function AgentWindow() {
  useAgentClient()
  const client = agentClient
  const isEmpty = client.state.active.items.length === 0

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        height: '100%',
        backgroundColor: C.canvas,
        fontFamily: FONT_SANS,
        color: C.text,
        // The settings modal is an absolute layer over the whole window.
        position: 'relative',
      }}
    >
      <TitleBar
        title={client.state.active.title}
        appearance={client.state.appearance}
        onToggleSidebar={() => client.ui.toggleSidebar()}
        onToggleAppearance={() => client.ui.toggleAppearance()}
        onSearch={() => client.ui.setSearchOpen(!client.state.searchOpen)}
        onDragNotice={(text) => void client.request('debug.trace', { text })}
      />
      <div style={{ display: 'flex', flexDirection: 'row', flexGrow: 1, minHeight: 0 }}>
        {client.state.sidebarOpen ? (
          <Sidebar
            client={client}
            searchOpen={client.state.searchOpen}
            onCloseSearch={() => client.ui.setSearchOpen(false)}
          />
        ) : null}
        <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
          <TabStrip client={client} />
          {isEmpty ? (
            <EmptyConversationView client={client} />
          ) : (
            <>
              <Transcript client={client} />
              <Composer client={client} />
            </>
          )}
        </div>
        {client.state.debugOpen ? <DebugPanel client={client} /> : null}
      </div>
      {client.state.settingsOpen ? <SettingsDialog client={client} /> : null}
      {client.state.pluginsOpen ? <PluginsDialog client={client} /> : null}
      {client.state.paletteOpen ? <CommandPalette client={client} /> : null}
      {client.state.confirmModal ? (
        <ConfirmDialog
          options={client.state.confirmModal}
          onClose={() => client.ui.closeConfirm()}
        />
      ) : null}
    </div>
  )
}
