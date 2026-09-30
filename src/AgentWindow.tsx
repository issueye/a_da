/**
 * The window: a title row, the sidebar, the conversation, and the task box.
 *
 * The store lives outside React because async turns keep writing to it while
 * the model streams, so this component only subscribes and re-renders.
 *
 * ## M0 迁移期的双重接线（临时）
 *
 * 拆分计划里 UI 最终只认 `client`（`ui/client`）。迁移是**父先子后**：一个组件要接收
 * `client`，它得先有 client 可传。所以这里在过渡期**同时**持有 `store`（给尚未迁移的子组件）
 * 与 `client`（给已迁移的）。每迁完一个子组件，就把对应那一行的 `store={agent}` 换成
 * `client={client}`；全部换完时 `store` 这条线从这里消失（`useAgentStore` 只留订阅）。
 * 守门测试盯着这条线：`src/ui/**` 一旦还有 `store.` 用法就在"待迁移清单"里，清单必须清空。
 */

import React, { useEffect, useState } from 'react'
import { store, type AgentStore } from './agent/store'
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

/** 订阅 store 的变化并重渲染（M0 起等价于「订阅客户端状态变化」，见 client.subscribe）。 */
function useAgentStore(): AgentStore {
  const [, setTick] = useState(0)
  useEffect(() => store.subscribe(() => setTick((tick) => tick + 1)), [])
  return store
}

export function AgentWindow() {
  const agent = useAgentStore()
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
            store={agent}
            searchOpen={client.state.searchOpen}
            onCloseSearch={() => client.ui.setSearchOpen(false)}
          />
        ) : null}
        <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
          <TabStrip client={client} />
          {isEmpty ? (
            <EmptyConversationView store={agent} />
          ) : (
            <>
              <Transcript client={client} />
              <Composer store={agent} />
            </>
          )}
        </div>
        {client.state.debugOpen ? <DebugPanel client={client} /> : null}
      </div>
      {client.state.settingsOpen ? <SettingsDialog store={agent} /> : null}
      {client.state.pluginsOpen ? <PluginsDialog store={agent} /> : null}
      {client.state.paletteOpen ? <CommandPalette store={agent} /> : null}
      {client.state.confirmModal ? (
        <ConfirmDialog
          options={client.state.confirmModal}
          onClose={() => client.ui.closeConfirm()}
        />
      ) : null}
    </div>
  )
}
