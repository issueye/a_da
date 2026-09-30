/**
 * 主机侧的命令派发表（协议 §3 的服务端实现）。
 *
 * **为什么在这里而不是客户端适配器里**：M0/M1 阶段"进程内客户端"既是传输又是服务端，
 * 派发表顺手写在了 `ui/client/in-process.ts` 里。M3 起主机是独立角色（`--host`），
 * 命令必须由主机执行——所以搬到这一侧，进程内适配器与 WebSocket 服务端**共用同一份**。
 *
 * 这是**唯一**允许直接调用 store 与各管理器的地方（协议 §0.2 的不变量：实现只在主机侧）。
 */

import type { AgentStore } from '../store'
import { PROTOCOL_VERSION, RpcErrorCode, appError, AppErrorCode, ProtocolError } from '../../shared/protocol'
import { readHostSnapshot } from './snapshot'
import type {
  AgentMode,
  ParamsOf,
  PluginCapabilities,
  PromptItem,
  ProtocolMethod,
  ProviderConfig,
  ResultOf,
} from '../../shared/protocol'
import { defaultExtensionLoader } from '../tools/loader'
import { BUILTIN_TOOLS_CATALOG, defaultToolRegistry } from '../tools/registry'
import { getPluginDiagnostics } from '../plugins/registry'
import { defaultSkillManager } from '../skills'
import { defaultPromptManager } from '../prompts'
import { defaultSubagentManager } from '../subagents'
import {
  PROVIDER_PRESETS,
  configPath,
  readPluginCapabilities,
  readPluginConfig,
  readPluginSecret,
  readSavedConfig,
  savePluginCapabilities,
  savePluginConfig,
  savePluginSecret,
} from '../config'
import { getAppHome } from '../home'

/** 造一个绑定了 store 的命令派发器。 */
export function createCommandDispatcher(
  store: AgentStore,
): (method: ProtocolMethod, params: unknown) => Promise<unknown> {
  /**
   * 会话 id：每个派发器一份。
   *
   * M3 起主机是独立角色；一个主机进程就一个会话。M4 支持多客户端时这里会变成
   * "每个连接一个 session"，`session.hello` 也要跟着记客户端信息。
   */
  const sessionId = `host-${process.pid}-${Math.random().toString(36).slice(2, 10)}`

  /** 未实现的协议方法：明确报错，而不是静默 no-op（"不允许静默失效"）。 */
  const notImplemented = (method: ProtocolMethod): never => {
    throw new ProtocolError(
      AppErrorCode.NotReady,
      `协议方法 ${method} 在主机侧尚未实现`,
      { what: method }
    )
  }

  async function dispatch(method: ProtocolMethod, params: unknown): Promise<unknown> {
    const p = (params ?? {}) as Record<string, unknown>
    switch (method) {
      // ── 会话（连接与快照，协议 §1.2/§1.3）──
      case 'session.initialize': {
        const requested = p.protocolVersion as string | undefined
        if (requested && requested !== PROTOCOL_VERSION) {
          throw appError(
            'ProtocolVersionMismatch',
            `协议版本不匹配：主机 ${PROTOCOL_VERSION}，客户端 ${requested}`,
            { serverVersion: PROTOCOL_VERSION, minClient: requested }
          )
        }
        // 令牌校验在传输层做（升级时一次、握手时一次，协议 §1.6）；这里只认会话身份。
        // `client` 与 `capabilities` 接受但未使用：能力协商是 M4（协议 §12.3）的事，
        // 现在假装协商过只会让"降级到底有没有生效"变得不可查。
        return { sessionId, protocolVersion: PROTOCOL_VERSION, host: { pid: process.pid } }
      }
      case 'session.snapshot':
        return readHostSnapshot(store)

      // ── UI 外壳状态（M1/M2 期间由主机镜像；见协议方法表里的说明）──
      case 'ui.setShell': {
        const patch = (p.patch ?? {}) as Record<string, unknown>
        if (patch.debugOpen !== undefined) store.setDebugOpen(patch.debugOpen as boolean)
        if (patch.settingsOpen !== undefined) store.setSettings(patch.settingsOpen as boolean)
        if (patch.pluginsOpen !== undefined) store.setPlugins(patch.pluginsOpen as boolean)
        if (patch.changesOpen !== undefined) store.setChangesOpen(patch.changesOpen as boolean)
        if (patch.paletteOpen !== undefined) store.setPaletteOpen(patch.paletteOpen as boolean)
        if (patch.searchOpen !== undefined) store.setSearchOpen(patch.searchOpen as boolean)
        if (patch.sidebarOpen !== undefined) {
          if (patch.sidebarOpen !== store.sidebarOpen) store.toggleSidebar()
        }
        if (patch.appearance !== undefined) {
          // 取值校验：线上来的是字符串，不合法就明说（不静默按默认值处理）
          const next = patch.appearance
          if (next !== 'dark' && next !== 'light') {
            // 参数不合法是 JSON-RPC 标准码（-32602），不是应用级错误
            throw new ProtocolError(
              RpcErrorCode.InvalidParams,
              `外观只认 dark / light，收到 ${String(next)}`,
              { key: 'appearance', invalid: [String(next)] }
            )
          }
          if (next !== store.appearance) store.setAppearance(next)
        }
        if (patch.pendingDraft !== undefined) {
          if (patch.pendingDraft === null) store.clearPendingDraft()
          else store.applyPromptToComposer(String(patch.pendingDraft))
        }
        return undefined
      }
      case 'ui.openTab':
        store.openTab(p.threadId as string)
        return undefined
      case 'ui.closeTab':
        store.closeTab(p.threadId as string)
        return undefined

      // ── 会话内容 ──
      case 'thread.create': {
        const thread = store.newThread(p.workspace as string)
        return { threadId: thread.id }
      }
      case 'thread.delete':
        return { message: await store.deleteThread(p.threadId as string) }
      case 'thread.send': {
        // M0：store.send 只用"当前会话"；传了别的 id 就如实说出来，别假装路由成功
        if (p.threadId && p.threadId !== store.activeId) {
          store.trace(`[客户端] thread.send 的 threadId 与当前会话不一致，M0 仍发给当前会话`)
        }
        store.send(p.text as string, p.images as string[] | undefined)
        return undefined
      }
      case 'thread.abort':
        store.stop(p.threadId as string)
        return undefined
      case 'thread.compact':
        return await store.compactThread(p.threadId as string, {
          customInstructions: p.customInstructions as string | undefined,
          trigger: p.trigger as 'manual' | 'auto' | undefined,
        })
      case 'thread.setMode':
        // M0：store.setMode 是全局写作模式（会同步给当前会话）；threadId 只作说明
        store.setMode(p.mode as ParamsOf<'thread.setMode'>['mode'])
        return undefined
      case 'thread.setWorkspace':
        store.setThreadWorkspace(p.threadId as string, p.workspace as string)
        return undefined
      case 'thread.editAndResend':
        // 注意参数顺序与协议不同：store 是 (itemId, text, images?, threadId?)
        await store.editUserMessageAndResend(
          p.itemId as string,
          p.text as string,
          p.images as string[] | undefined,
          p.threadId as string
        )
        return undefined

      // ── 排队指令 ──
      case 'queue.clear':
        store.clearQueue(p.threadId as string | undefined)
        return undefined
      case 'queue.promote':
        store.sendQueuedImmediately(p.index as number, p.threadId as string | undefined)
        return undefined
      case 'queue.remove':
        return store.removeQueuedItem(p.index as number, p.threadId as string | undefined)

      // ── 子智能体 ──
      case 'subagent.resume': {
        const { thread } = await store.resumeSubagentThread({
          subagentThreadId: p.subagentThreadId as string,
          instruction: p.instruction as string | undefined,
        })
        return { threadId: thread.id }
      }

      // ── 审批 / 提问 ──
      case 'approval.decide':
        store.decide(p.toolItemId as string, p.approved as boolean)
        return undefined
      case 'question.answer':
        store.answerQuestion(p.callId as string, {
          choice: p.choice as string | undefined,
          text: p.text as string | undefined,
        })
        return undefined

      // ── 工作区 ──
      case 'workspace.add':
        return { error: await store.addProject(p.path as string) }
      case 'workspace.remove':
        return { message: store.removeProject(p.workspace as string) }
      case 'workspace.openPublic':
        await store.openPublicWorkspace(p.threadId as string | undefined)
        return undefined
      case 'workspace.rescan':
        await store.refresh()
        return undefined
      case 'workspace.entries':
        // M0：整份返回（协议要求分页，M2 再做）
        return store.entries

      // ── 焦点上报 ──
      case 'ui.activeThread':
        store.selectThread(p.threadId as string)
        return undefined
      case 'ui.activeProject':
        store.selectProject(p.workspace as string)
        return undefined

      // ── 配置 ──
      case 'config.setProvider':
        return { error: await store.saveProvider(p.config as ParamsOf<'config.setProvider'>['config']) }
      case 'config.checkProvider':
        return { message: await store.checkProvider(p.config as ParamsOf<'config.checkProvider'>['config']) }
      case 'config.setApproval':
        store.setApproval(p.mode as ParamsOf<'config.setApproval'>['mode'])
        return undefined
      case 'config.setEffort':
        store.setEffort(p.effort as ParamsOf<'config.setEffort'>['effort'])
        return undefined

      // ── 改动审阅 ──
      case 'change.count':
        return { count: store.getThreadChangeCount(p.threadId as string) }
      case 'change.list':
        return store.getThreadFileChanges(p.threadId as string)
      case 'change.revertCard':
        return { ok: await store.revertCard(p.threadId as string, p.cardId as string) }
      case 'change.revertFile':
        return { ok: await store.revertFile(p.threadId as string, p.path as string) }
      case 'change.revertAll':
        return { ok: await store.revertAllChanges(p.threadId as string) }

      // ── 调试 ──
      case 'debug.trace':
        store.trace(p.text as string)
        return undefined
      case 'debug.log.clear':
        store.clearLog()
        return undefined
      case 'debug.hostInfo':
        // 主机环境：管理页要展示"模板会建到哪、配置文件在哪"
        return {
          homeDir: getAppHome(),
          extensionsDir: `${getAppHome()}/extensions`,
          configPath: configPath(),
        }

      // ── 统计 ──
      case 'stats.promptChars':
        return {
          systemChars: defaultPromptManager.getCompositeSystemPromptSync(
            p.workspace as string,
            p.mode as AgentMode
          ).length,
          toolSpecsChars: JSON.stringify(
            defaultToolRegistry.getToolsForMode(p.workspace as string, p.mode as AgentMode)
          ).length,
        }

      // ── 插件管理（M2：UI 不再直接摸加载器与配置文件）──
      case 'plugin.list': {
        const workspace = p.workspace as string
        const [plugins, resolved] = await Promise.all([
          defaultExtensionLoader.scanPlugins(workspace),
          readPluginCapabilities(workspace),
        ])
        // 每个插件的配置草稿与"密钥是否已设置"在这里一次备齐：界面打开这一页全都要
        const configs: Record<string, Record<string, unknown>> = {}
        const secrets: Record<string, boolean> = {}
        for (const item of plugins) {
          const properties = item.plugin.contributions.configSchema?.properties
          if (!properties) continue
          configs[item.id] = await readPluginConfig<Record<string, unknown>>(item.id)
          for (const [key, property] of Object.entries(properties)) {
            if (property.type !== 'secret') continue
            // 只回布尔：密钥本身永远不出主机
            secrets[`${item.id}:${key}`] = readPluginSecret(item.id, key).length > 0
          }
        }
        return {
          plugins,
          capabilities: {
            capabilities: resolved.capabilities,
            invalid: resolved.invalid,
            overrides: resolved.overrides,
          },
          configs,
          secrets,
          diagnostics: getPluginDiagnostics(workspace),
        } satisfies ResultOf<'plugin.list'>
      }
      case 'plugin.capabilities.set':
        await savePluginCapabilities(p.patch as Partial<PluginCapabilities>)
        return undefined
      case 'plugin.config.set':
        await savePluginConfig(p.pluginId as string, p.values as Record<string, unknown>)
        return undefined
      case 'plugin.secret.set':
        await savePluginSecret(p.pluginId as string, p.key as string, p.value as string)
        return undefined
      case 'plugin.setEnabled':
        await defaultExtensionLoader.togglePlugin(
          p.pluginId as string,
          p.enabled as boolean,
          p.workspace as string
        )
        return undefined
      case 'plugin.delete':
        return {
          ok: await defaultExtensionLoader.deletePlugin(p.filePath as string, p.workspace as string),
        }
      case 'plugin.createTemplate':
        return {
          filePath: await defaultExtensionLoader.createPluginTemplate(
            p.workspace as string,
            p.scope as 'workspace' | 'global',
            p.name as string,
            p.code as string | undefined
          ),
        }
      case 'plugin.builtinCatalog':
        return BUILTIN_TOOLS_CATALOG

      // ── 技能 / 提示词 / 子智能体档案（M2：UI 不再直接摸管理器）──
      case 'skill.list':
        return defaultSkillManager.scanSkills(p.workspace as string)
      case 'skill.setEnabled':
        await defaultSkillManager.toggleSkill(p.id as string, p.enabled as boolean)
        return undefined
      case 'skill.create':
        return {
          filePath: await defaultSkillManager.createSkillTemplate({
            name: p.name as string,
            description: p.description as string,
            scope: p.scope as 'workspace' | 'global',
            workspaceRoot: p.workspace as string,
            body: p.body as string | undefined,
          }),
        }
      case 'skill.delete':
        await defaultSkillManager.deleteSkill(p.id as string, p.workspace as string)
        return { ok: true }

      case 'prompt.list':
        return defaultPromptManager.scanPrompts(p.workspace as string)
      case 'prompt.setEnabled':
        return {
          ok: await defaultPromptManager.togglePrompt(
            p.id as string,
            p.enabled as boolean,
            p.workspace as string
          ),
        }
      case 'prompt.create':
        return await defaultPromptManager.createPrompt(
          p.workspace as string,
          p.options as ParamsOf<'prompt.create'>['options']
        )
      case 'prompt.update':
        return { ok: await defaultPromptManager.updatePrompt(p.item as PromptItem) }
      case 'prompt.delete':
        return { ok: await defaultPromptManager.deletePrompt(p.filePath as string) }

      case 'subagentProfile.list':
        return defaultSubagentManager.getSubagents(p.workspace as string | undefined)
      case 'subagentProfile.setEnabled':
        await defaultSubagentManager.toggleSubagent(
          p.id as string,
          p.enabled as boolean,
          p.workspace as string | undefined
        )
        return undefined
      case 'subagentProfile.delete':
        return {
          ok: await defaultSubagentManager.deleteSubagent(
            p.id as string,
            p.workspace as string | undefined
          ),
        }

      // ── 配置读取 ──
      case 'config.get': {
        const cfg = await readSavedConfig()
        // 显式列出要交出去的字段：`SavedConfig` 还有一堆别的东西（外观、能力开关…），
        // 不该顺手漏出去。
        const saved: Partial<ProviderConfig> = {
          baseUrl: cfg.baseUrl,
          apiKey: cfg.apiKey,
          model: cfg.model,
          contextWindow: cfg.contextWindow,
          supportsImages: cfg.supportsImages,
          headers: cfg.headers,
        }
        return { saved, path: configPath() }
      }
      case 'config.presets':
        return PROVIDER_PRESETS

      default:
        return notImplemented(method)
    }
  }

  return dispatch
}
