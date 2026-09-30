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
import { agentClient, type AgentClient } from './ui/client'
import { Composer } from './ui/Composer'
import { DebugPanel } from './ui/DebugPanel'
import { PluginsDialog } from './ui/PluginsDialog'
import { SettingsDialog } from './ui/SettingsDialog'
import { ConfirmDialog } from './ui/ConfirmDialog'
import { CommandPalette } from './ui/CommandPalette'
import { Sidebar } from './ui/Sidebar'
import { EmptyConversationView } from './ui/EmptyConversationView'
import { TabStrip } from './ui/TabStrip'
import { ConnectionBanner } from './ui/ConnectionBanner'
import { FilePicker } from './ui/FilePicker'
import { TitleBar } from './ui/TitleBar'
import { Transcript } from './ui/Transcript'
import { C, FONT_SANS } from './theme'

/**
 * 订阅客户端状态变化并重渲染。
 *
 * M0 时 `client.subscribe` 就是 store 的那次广播；M1 起它由事件流驱动，
 * 这个组件的写法不用再改。
 */
function useAgentClient(client: AgentClient): void {
  const [, setTick] = useState(0)
  useEffect(() => client.subscribe(() => setTick((tick) => tick + 1)), [client])
}

/**
 * 窗口根组件。
 *
 * `client` 可传可不传：**测试与开发态**默认用进程内单例（`bun test` 与 `bun run dev` 都是这条），
 * **打包形态**由 `src/ui/main.tsx` 先 `resolveAgentClient()` 拿到 WebSocket 客户端再传进来
 * （协议 §1.8：本机也走 WS，只是用户看不见）。
 */
export function AgentWindow({ client: injected }: { client?: AgentClient } = {}) {
  const client = injected ?? agentClient
  useAgentClient(client)
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
      {/* 与主机的连接断了就说出来（协议 §1.5）；进程内传输永远不会显示它 */}
      <ConnectionBanner client={client} />
      {/* 文件/目录选择器是**窗口级**模态层（与确认框同一套做法）：
          挂在下拉内部会在下拉关闭时被一起卸载 */}
      {client.state.filePicker ? (
        <FilePicker
          client={client}
          mode={client.state.filePicker.mode}
          title={client.state.filePicker.title}
          startPath={client.state.filePicker.startPath}
          accept={client.state.filePicker.accept}
          onPicked={(paths) => {
            const request = client.state.filePicker
            client.ui.closeFilePicker()
            request?.onPicked(paths)
          }}
          onClose={() => client.ui.closeFilePicker()}
        />
      ) : null}
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
