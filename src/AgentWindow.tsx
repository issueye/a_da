/**
 * The window: a title row, the sidebar, the conversation, and the task box.
 *
 * The store lives outside React because async turns keep writing to it while
 * the model streams, so this component only subscribes and re-renders.
 */

import React, { useEffect, useState } from 'react'
import { store, type AgentStore } from './agent/store'
import { Composer } from './ui/Composer'
import { DebugPanel } from './ui/DebugPanel'
import { PluginsDialog } from './ui/PluginsDialog'
import { SettingsDialog } from './ui/SettingsDialog'
import { Sidebar } from './ui/Sidebar'
import { EmptyConversationView } from './ui/EmptyConversationView'
import { TabStrip } from './ui/TabStrip'
import { TitleBar } from './ui/TitleBar'
import { Transcript } from './ui/Transcript'
import { C, FONT_SANS } from './theme'

function useAgentStore(): AgentStore {
  const [, setTick] = useState(0)
  useEffect(() => store.subscribe(() => setTick((tick) => tick + 1)), [])
  return store
}

export function AgentWindow() {
  const agent = useAgentStore()
  const [sidebarOpen, setSidebarOpen] = useState(true)
  const [searchOpen, setSearchOpen] = useState(false)
  const isEmpty = agent.active.items.length === 0

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
        title={agent.active.title}
        appearance={agent.appearance}
        onToggleSidebar={() => setSidebarOpen((open) => !open)}
        onToggleAppearance={() => agent.toggleAppearance()}
        onSearch={() => {
          setSidebarOpen(true)
          setSearchOpen((open) => !open)
        }}
        onDragNotice={(text) => agent.trace(text)}
      />
      <div style={{ display: 'flex', flexDirection: 'row', flexGrow: 1, minHeight: 0 }}>
        {sidebarOpen ? (
          <Sidebar
            store={agent}
            searchOpen={searchOpen}
            onCloseSearch={() => setSearchOpen(false)}
          />
        ) : null}
        <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
          <TabStrip store={agent} />
          {isEmpty ? (
            <EmptyConversationView store={agent} />
          ) : (
            <>
              <Transcript store={agent} />
              <Composer store={agent} />
            </>
          )}
        </div>
        {agent.debugOpen ? <DebugPanel store={agent} /> : null}
      </div>
      {agent.settingsOpen ? <SettingsDialog store={agent} /> : null}
      {agent.pluginsOpen ? <PluginsDialog store={agent} /> : null}
    </div>
  )
}
