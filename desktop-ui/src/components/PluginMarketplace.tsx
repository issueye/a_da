import React, { useCallback, useEffect, useState } from 'react'
import {
  Store,
  Download,
  Search,
  RefreshCw,
  KeyRound,
  Globe,
  Check,
  AlertTriangle,
  Loader2,
  ExternalLink,
} from 'lucide-react'
import type { MarketConfigView, MarketPlugin, PluginItem } from '../types'
import { agentClient } from '../client/ws-client'
import { ConfirmModal } from './ConfirmModal'

interface PluginMarketplaceProps {
  /** 已安装的扩展：用来把市场条目标成「已安装 / 重新安装」 */
  installed: PluginItem[]
  /** 安装成功后刷新插件中心（`loadData`），让新插件立刻出现在「扩展」页 */
  onInstalled: () => void | Promise<void>
  onNotice: (message: string) => void
}

/**
 * 插件市场（对接 plugin-hub）。
 *
 * 数据流全部在 Rust 侧完成（`market.*` RPC）：渲染进程不直接 fetch 插件库，
 * 因此没有 CORS 问题，Token 也只留在宿主配置里、不回传完整值。
 */
export const PluginMarketplace: React.FC<PluginMarketplaceProps> = ({
  installed,
  onInstalled,
  onNotice,
}) => {
  const [config, setConfig] = useState<MarketConfigView | null>(null)
  const [baseUrl, setBaseUrl] = useState('')
  const [token, setToken] = useState('')
  const [savingConfig, setSavingConfig] = useState(false)
  const [configError, setConfigError] = useState<string | null>(null)

  const [query, setQuery] = useState('')
  const [plugins, setPlugins] = useState<MarketPlugin[]>([])
  const [total, setTotal] = useState(0)
  const [searched, setSearched] = useState(false)
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)

  const [installing, setInstalling] = useState<string | null>(null)
  const [overwriteTarget, setOverwriteTarget] = useState<MarketPlugin | null>(null)

  // 市场安装固定落到**全局**扩展目录；工作区里同名插件不算"已安装"，
  // 否则用户会以为重装的是工作区那份。
  const installedSlugs = new Set(
    installed.filter((item) => item.scope === 'global').map((item) => item.fileName),
  )

  const loadConfig = useCallback(async (autoSearch: boolean) => {
    try {
      const view = await agentClient.getMarketConfig()
      setConfig(view)
      setBaseUrl(view.baseUrl || '')
      setToken('')
      if (autoSearch && view.baseUrl && view.hasToken) {
        try {
          const result = await agentClient.searchMarket({ query: '', limit: 24, offset: 0 })
          setPlugins(result.plugins)
          setTotal(result.total)
          setSearched(true)
        } catch (error: any) {
          // 配置读到了、只是搜索失败：不要把错误显示成"读取配置失败"
          setSearchError(error?.message || '搜索失败')
        }
      }
    } catch (error: any) {
      setConfigError(error?.message || '读取插件市场配置失败')
    }
  }, [])

  useEffect(() => {
    loadConfig(true)
  }, [loadConfig])

  const saveConfig = async (clearToken = false) => {
    setSavingConfig(true)
    setConfigError(null)
    try {
      const result = await agentClient.setMarketConfig({
        baseUrl: baseUrl.trim(),
        apiToken: token.trim() || undefined,
        clearToken,
      })
      setConfig(result.config)
      setToken('')
      onNotice(clearToken ? '已清除插件市场 Token' : '插件市场配置已保存')
      if (result.config.baseUrl && result.config.hasToken) {
        await search('', result.config)
      }
    } catch (error: any) {
      setConfigError(error?.message || '保存失败')
    } finally {
      setSavingConfig(false)
    }
  }

  const search = async (keyword = query, current = config) => {
    if (!current?.baseUrl) {
      setSearchError('请先填写插件库地址')
      return
    }
    if (!current.hasToken) {
      setSearchError('请先填写 API Token（plugin-hub 的 /settings/tokens 可创建）')
      return
    }
    setSearching(true)
    setSearchError(null)
    try {
      const result = await agentClient.searchMarket({
        query: keyword.trim(),
        limit: 24,
        offset: 0,
      })
      setPlugins(result.plugins)
      setTotal(result.total)
      setSearched(true)
    } catch (error: any) {
      setSearchError(error?.message || '搜索失败')
    } finally {
      setSearching(false)
    }
  }

  const install = async (plugin: MarketPlugin, overwrite: boolean) => {
    if (!plugin.version) {
      onNotice(`${plugin.name} 还没有发布版本`)
      return
    }
    setInstalling(plugin.slug)
    try {
      const result = await agentClient.installMarketPlugin({
        owner: plugin.owner,
        slug: plugin.slug,
        version: plugin.version,
        overwrite,
      })
      const applied = result.applied
      onNotice(
        applied === false
          ? `${plugin.name} 已安装，但重装配被拒（见「插件诊断」）`
          : applied === null || applied === undefined
            ? `${plugin.name}@${plugin.version} 已安装（当前宿主未接重装配，重启后生效）`
            : `${plugin.name}@${plugin.version} 已安装并生效`,
      )
      await onInstalled()
      await search()
    } catch (error: any) {
      onNotice(`安装失败: ${error?.message || error}`)
    } finally {
      setInstalling(null)
    }
  }

  const requestInstall = (plugin: MarketPlugin) => {
    if (!plugin.hasRelease || !plugin.version) {
      onNotice(`${plugin.name} 还没有可安装的版本`)
      return
    }
    if (installedSlugs.has(plugin.slug)) {
      setOverwriteTarget(plugin)
      return
    }
    install(plugin, false)
  }

  const configured = Boolean(config?.baseUrl && config?.hasToken)

  return (
    <div className="space-y-3">
      <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
        插件市场对接 <span className="font-medium text-zinc-800 dark:text-zinc-200">plugin-hub</span>。
        发布包必须在包根 <code className="mx-1 font-mono text-[10px]">index.ts</code> 的
        <code className="mx-1 font-mono text-[10px]">metadata</code>
        里包含 name / description / author / version；安装到
        <code className="mx-1 font-mono text-[10px]">~/.a-da/agent-coding/extensions/</code>
        后会立即重装配。
      </div>

      {/* 连接配置 */}
      <div className="rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] p-3.5 space-y-2.5">
        <div className="flex items-center space-x-2">
          <Globe size={14} className="text-blue-500" />
          <span className="font-semibold text-zinc-800 dark:text-zinc-200 text-xs">插件库连接</span>
          {configured ? (
            <span className="flex items-center space-x-1 text-[10px] text-emerald-600 dark:text-emerald-400">
              <Check size={11} />
              <span>已配置</span>
            </span>
          ) : (
            <span className="text-[10px] text-amber-600 dark:text-amber-400">未配置</span>
          )}
        </div>

        <div className="grid grid-cols-1 md:grid-cols-[1fr_1fr_auto] gap-2">
          <label className="block">
            <span className="text-[10px] text-zinc-500 dark:text-zinc-400">地址</span>
            <input
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="http://127.0.0.1:8787"
              spellCheck={false}
              className="mt-0.5 w-full px-2.5 py-1.5 bg-white dark:bg-zinc-900 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] font-mono text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500 transition-colors"
            />
          </label>
          <label className="block">
            <span className="text-[10px] text-zinc-500 dark:text-zinc-400">
              API Token
              {config?.hasToken && (
                <span className="ml-1 font-mono text-zinc-400">（已配置 {config.tokenHint}，留空保持不变）</span>
              )}
            </span>
            <div className="mt-0.5 relative">
              <KeyRound
                size={12}
                className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-400"
              />
              <input
                value={token}
                onChange={(event) => setToken(event.target.value)}
                placeholder={config?.hasToken ? '粘贴新 Token 以替换' : 'ada_pat_...'}
                type="password"
                autoComplete="off"
                spellCheck={false}
                className="w-full pl-8 pr-2.5 py-1.5 bg-white dark:bg-zinc-900 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] font-mono text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500 transition-colors"
              />
            </div>
          </label>
          <div className="flex items-end space-x-1.5">
            <button
              onClick={() => saveConfig(false)}
              disabled={savingConfig}
              className="px-3 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 disabled:opacity-50 text-white font-medium text-[11px] shadow-sm transition-colors"
            >
              {savingConfig ? '保存中…' : '保存'}
            </button>
            {config?.hasToken && (
              <button
                onClick={() => saveConfig(true)}
                disabled={savingConfig}
                title="清除已保存的 Token"
                className="px-2.5 py-1.5 rounded-lg border border-zinc-200 dark:border-[#2e2e33] text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 disabled:opacity-50 text-[11px] transition-colors"
              >
                清除 Token
              </button>
            )}
          </div>
        </div>

        {configError && (
          <div className="flex items-start space-x-1.5 text-[11px] text-rose-600 dark:text-rose-400">
            <AlertTriangle size={12} className="mt-0.5 shrink-0" />
            <span>{configError}</span>
          </div>
        )}
        <div className="text-[10px] text-zinc-400 dark:text-zinc-500">
          Token 只保存在本机 <code className="font-mono">~/.a-da/plugin_hub.json</code>，界面不会显示完整值。
        </div>
      </div>

      {/* 搜索 */}
      <div className="flex items-center gap-2">
        <div className="relative flex-1">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-400" />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') search()
            }}
            placeholder="搜索插件名称、作者、标签…"
            className="w-full pl-8 pr-3 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500 transition-colors"
          />
        </div>
        <button
          onClick={() => search()}
          disabled={searching || !configured}
          className="flex items-center space-x-1 px-3 py-1.5 rounded-lg bg-zinc-200 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 disabled:opacity-50 text-zinc-700 dark:text-zinc-200 font-medium text-[11px] border border-zinc-300 dark:border-zinc-700 transition-colors"
        >
          {searching ? <Loader2 size={12} className="animate-spin" /> : <Search size={12} />}
          <span>搜索</span>
        </button>
        <button
          onClick={() => {
            setQuery('')
            search('')
          }}
          disabled={searching || !configured}
          title="刷新市场列表"
          className="p-1.5 rounded-lg border border-zinc-200 dark:border-[#2e2e33] text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 disabled:opacity-50 transition-colors"
        >
          <RefreshCw size={13} className={searching ? 'animate-spin' : ''} />
        </button>
      </div>

      {searchError && (
        <div className="flex items-start space-x-1.5 px-3 py-2 rounded-lg bg-rose-50 dark:bg-rose-950/30 border border-rose-200 dark:border-rose-900/50 text-[11px] text-rose-700 dark:text-rose-300">
          <AlertTriangle size={12} className="mt-0.5 shrink-0" />
          <span>{searchError}</span>
        </div>
      )}

      {!configured && !searchError && (
        <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500">
          <Store size={22} className="mx-auto text-zinc-400" />
          <div className="mt-2 text-[12px] font-medium text-zinc-700 dark:text-zinc-300">
            先连接一个插件库
          </div>
          <div className="mt-1 text-[11px]">
            填写 plugin-hub 地址，并在其「API Token」页创建一个 read Token 填到上面。
          </div>
        </div>
      )}

      {configured && searched && plugins.length === 0 && !searchError && (
        <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500 text-[11px]">
          没有找到匹配的插件。
        </div>
      )}

      {plugins.length > 0 && (
        <div className="space-y-2.5">
          <div className="text-[10px] text-zinc-500 font-mono">
            共 {total} 个插件 · 已安装 {plugins.filter((p) => installedSlugs.has(p.slug)).length} 个
          </div>
          {plugins.map((plugin) => {
            const isInstalled = installedSlugs.has(plugin.slug)
            const isInstalling = installing === plugin.slug
            const canInstall = plugin.hasRelease && Boolean(plugin.version)
            const author = plugin.author || plugin.owner
            const metaName =
              typeof plugin.metadata?.name === 'string' ? plugin.metadata.name : ''
            return (
              <div
                key={`${plugin.owner}/${plugin.slug}`}
                className="p-3.5 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors shadow-sm dark:shadow-none"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center flex-wrap gap-1.5">
                      <span className="font-semibold text-zinc-900 dark:text-zinc-200 text-xs">
                        {plugin.name}
                      </span>
                      <span className="text-[10px] font-mono text-zinc-500 dark:text-zinc-400">
                        @{plugin.owner}/{plugin.slug}
                      </span>
                      {plugin.version && (
                        <span className="text-[10px] px-1.5 py-0.2 rounded bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20 font-mono">
                          v{plugin.version}
                        </span>
                      )}
                      {isInstalled && (
                        <span className="flex items-center space-x-1 text-[10px] px-1.5 py-0.2 rounded bg-emerald-50 dark:bg-emerald-950/30 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/20">
                          <Check size={10} />
                          <span>已安装</span>
                        </span>
                      )}
                    </div>
                    <div className="mt-1 text-[11px] text-zinc-600 dark:text-zinc-400 leading-relaxed">
                      {plugin.description || '（暂无描述）'}
                    </div>
                    {plugin.tags.length > 0 && (
                      <div className="mt-1.5 flex flex-wrap gap-1">
                        {plugin.tags.map((tag) => (
                          <span
                            key={tag}
                            className="text-[10px] px-1.5 py-0.2 rounded bg-zinc-200/70 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300"
                          >
                            #{tag}
                          </span>
                        ))}
                      </div>
                    )}
                    <div className="mt-1.5 flex flex-wrap items-center gap-3 text-[10px] text-zinc-400 dark:text-zinc-500">
                      <span>作者 {author}</span>
                      <span>↓ {plugin.downloads}</span>
                      <span>★ {plugin.stars}</span>
                      {metaName && metaName !== plugin.name && (
                        <span className="font-mono">包名 {metaName}</span>
                      )}
                    </div>
                  </div>

                  <div className="flex flex-col items-end space-y-1 shrink-0">
                    <button
                      onClick={() => requestInstall(plugin)}
                      disabled={!canInstall || isInstalling || installing !== null}
                      title={
                        !canInstall
                          ? '这个插件还没有发布可安装的版本'
                          : isInstalled
                            ? '覆盖安装（会保留配置与技能文件吗？不会——包内文件整体替换）'
                            : `安装 ${plugin.name}@${plugin.version}`
                      }
                      className={`flex items-center space-x-1 px-2.5 py-1.5 rounded-lg font-medium text-[11px] transition-colors ${
                        canInstall
                          ? isInstalled
                            ? 'bg-zinc-200 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-200 border border-zinc-300 dark:border-zinc-700'
                            : 'bg-blue-600 hover:bg-blue-500 text-white shadow-sm'
                          : 'bg-zinc-100 dark:bg-zinc-800/60 text-zinc-400 dark:text-zinc-500 border border-zinc-200 dark:border-zinc-800 cursor-not-allowed'
                      }`}
                    >
                      {isInstalling ? (
                        <Loader2 size={12} className="animate-spin" />
                      ) : (
                        <Download size={12} />
                      )}
                      <span>
                        {isInstalling
                          ? '安装中…'
                          : !canInstall
                            ? '无版本'
                            : isInstalled
                              ? '重新安装'
                              : '安装'}
                      </span>
                    </button>
                    <a
                      href={`${config?.baseUrl || ''}/plugins/${plugin.owner}/${plugin.slug}`}
                      target="_blank"
                      rel="noreferrer"
                      className="flex items-center space-x-0.5 text-[10px] text-zinc-400 hover:text-blue-500 transition-colors"
                    >
                      <span>详情</span>
                      <ExternalLink size={10} />
                    </a>
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      )}

      <ConfirmModal
        isOpen={Boolean(overwriteTarget)}
        title="覆盖已安装的插件"
        message={`${overwriteTarget?.name || ''} 已经安装过了。`}
        subMessage={`继续会用插件包内的文件整体替换 ~/.a-da/agent-coding/extensions/${overwriteTarget?.slug || ''}（包内技能等文件一起替换）；插件在 ~/.a-da/config.json 里的配置与 Token 不受影响。`}
        confirmText="覆盖安装"
        variant="warning"
        onConfirm={async () => {
          const target = overwriteTarget
          setOverwriteTarget(null)
          if (target) await install(target, true)
        }}
        onCancel={() => setOverwriteTarget(null)}
      />
    </div>
  )
}
