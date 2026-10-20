import React, { useEffect, useMemo, useState } from 'react'
import { X, Upload, Loader2, AlertTriangle, Check, ExternalLink, Globe } from 'lucide-react'
import type { MarketConfigView, MarketPublishResult, PluginItem } from '../types'
import { agentClient } from '../client/ws-client'

interface PublishPluginModalProps {
  isOpen: boolean
  plugin: PluginItem | null
  onClose: () => void
  onNotice: (message: string) => void
  onPublished?: () => void
}

function defaultSlug(item: PluginItem): string {
  return (item.fileName || item.name || 'my-plugin')
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64)
}

/**
 * 把本地插件包上传到 plugin-hub。
 *
 * 包内 `index.ts` 的 metadata 由插件库在发布时校验（name/description/author/version 必填）；
 * 这里只填"仓库元数据"（slug/名称/简介/tags/README）与可选的版本号。
 */
export const PublishPluginModal: React.FC<PublishPluginModalProps> = ({
  isOpen,
  plugin,
  onClose,
  onNotice,
  onPublished,
}) => {
  const [config, setConfig] = useState<MarketConfigView | null>(null)
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [tags, setTags] = useState('')
  const [license, setLicense] = useState('')
  const [readme, setReadme] = useState('')
  const [version, setVersion] = useState('')
  const [notes, setNotes] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<MarketPublishResult['plugin'] | null>(null)

  const workspace = agentClient.snapshot.activeWorkspace

  useEffect(() => {
    if (!isOpen || !plugin) return
    setError(null)
    setResult(null)
    setSubmitting(false)
    setSlug(defaultSlug(plugin))
    setName(plugin.name || '')
    // 简介与 README 默认留空：发布时插件库会用索引里的 metadata.description 与包内 README.md 填充
    setDescription('')
    setTags('')
    setLicense('')
    setVersion('')
    setNotes('')
    setReadme('')
    agentClient
      .getMarketConfig()
      .then(setConfig)
      .catch((err: any) => setError(err?.message || '读取插件库配置失败'))
  }, [isOpen, plugin])

  const configured = Boolean(config?.baseUrl && config?.hasToken)
  const slugOk = useMemo(() => /^[a-z0-9][a-z0-9-]{1,63}$/.test(slug), [slug])

  if (!isOpen || !plugin) return null

  const submit = async () => {
    if (!slugOk) {
      setError('slug 只能包含小写字母、数字与连字符（2–64 位）')
      return
    }
    if (!name.trim()) {
      setError('请填写插件名称')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const response = await agentClient.publishPlugin({
        filePath: plugin.filePath,
        workspace,
        slug,
        name: name.trim(),
        description: description.trim(),
        tags: tags.trim(),
        license: license.trim(),
        readme,
        version: version.trim(),
        notes: notes.trim(),
      })
      setResult(response.plugin)
      onNotice(
        response.plugin.existing
          ? `已更新 ${response.plugin.slug} 并发布 v${response.plugin.version}`
          : `已创建 ${response.plugin.slug} 并发布 v${response.plugin.version}`,
      )
      onPublished?.()
    } catch (err: any) {
      setError(err?.message || '上传失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="fixed inset-0 bg-black/40 dark:bg-black/60 backdrop-blur-sm z-[60] flex items-center justify-center p-4 select-none">
      <div className="w-full max-w-xl bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#2e2e33] rounded-2xl shadow-2xl overflow-hidden flex flex-col text-xs">
        <div className="flex items-center justify-between px-5 py-3.5 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#1e1e22]/50">
          <div className="flex items-center space-x-2">
            <Upload size={15} className="text-blue-500" />
            <h3 className="font-semibold text-zinc-900 dark:text-zinc-100 text-sm">上传插件到插件库</h3>
            <span className="text-[10px] text-zinc-500 font-mono truncate max-w-[220px]" title={plugin.filePath}>
              {plugin.fileName}
            </span>
          </div>
          <button
            onClick={onClose}
            className="p-1 text-zinc-500 hover:text-zinc-900 dark:hover:text-white hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors"
          >
            <X size={15} />
          </button>
        </div>

        <div className="p-5 space-y-3.5 overflow-y-auto max-h-[70vh]">
          <div className="flex items-center space-x-2 text-[11px] text-zinc-600 dark:text-zinc-400">
            <Globe size={12} className="text-zinc-400" />
            <span>目标插件库：</span>
            <span className="font-mono text-zinc-800 dark:text-zinc-200">
              {config?.baseUrl || '未配置'}
            </span>
            {config?.hasToken && (
              <span className="text-[10px] text-emerald-600 dark:text-emerald-400">
                Token {config.tokenHint}
              </span>
            )}
          </div>

          {!configured && (
            <div className="flex items-start space-x-1.5 px-3 py-2 rounded-lg bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-900/50 text-[11px] text-amber-700 dark:text-amber-300">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" />
              <span>请先在「插件市场」页填写插件库地址与 API Token（发布需要 write 作用域）。</span>
            </div>
          )}

          {error && (
            <div className="flex items-start space-x-1.5 px-3 py-2 rounded-lg bg-rose-50 dark:bg-rose-950/30 border border-rose-200 dark:border-rose-900/50 text-[11px] text-rose-700 dark:text-rose-300">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" />
              <span className="break-all">{error}</span>
            </div>
          )}

          {result ? (
            <div className="rounded-xl border border-emerald-200 dark:border-emerald-500/30 bg-emerald-50 dark:bg-emerald-950/30 p-4 space-y-2">
              <div className="flex items-center space-x-1.5 text-emerald-700 dark:text-emerald-300 font-semibold text-[12px]">
                <Check size={14} />
                <span>已发布 v{result.version}</span>
              </div>
              <div className="text-[11px] text-emerald-700/90 dark:text-emerald-300/90">
                {result.existing ? '仓库已存在，本次更新了元数据并发布新版本。' : '已新建插件仓库并发布首个版本。'}
              </div>
              <a
                href={result.url}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center space-x-1 text-[11px] text-blue-600 dark:text-blue-400 hover:underline break-all"
              >
                <span>{result.url}</span>
                <ExternalLink size={11} />
              </a>
            </div>
          ) : (
            <>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <label className="block">
                  <span className="text-[11px] text-zinc-600 dark:text-zinc-400">Slug（URL 标识）</span>
                  <input
                    value={slug}
                    onChange={(event) => setSlug(event.target.value.toLowerCase())}
                    spellCheck={false}
                    className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] font-mono text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                  />
                  {!slugOk && <span className="text-[10px] text-rose-500">仅小写字母/数字/连字符</span>}
                </label>
                <label className="block">
                  <span className="text-[11px] text-zinc-600 dark:text-zinc-400">名称</span>
                  <input
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    maxLength={80}
                    className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                  />
                </label>
              </div>

              <label className="block">
                <span className="text-[11px] text-zinc-600 dark:text-zinc-400">简介（留空用 metadata.description）</span>
                <textarea
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  rows={2}
                  maxLength={300}
                  placeholder="留空则使用插件包 index.ts 里的 metadata.description"
                  className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                />
              </label>

              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <label className="block">
                  <span className="text-[11px] text-zinc-600 dark:text-zinc-400">标签（逗号分隔）</span>
                  <input
                    value={tags}
                    onChange={(event) => setTags(event.target.value)}
                    placeholder="git, review"
                    className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                  />
                </label>
                <label className="block">
                  <span className="text-[11px] text-zinc-600 dark:text-zinc-400">License</span>
                  <input
                    value={license}
                    onChange={(event) => setLicense(event.target.value)}
                    placeholder="MIT"
                    className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                  />
                </label>
                <label className="block">
                  <span className="text-[11px] text-zinc-600 dark:text-zinc-400">版本（可留空）</span>
                  <input
                    value={version}
                    onChange={(event) => setVersion(event.target.value)}
                    placeholder="用包内 metadata.version"
                    className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] font-mono text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500"
                  />
                </label>
              </div>

              <label className="block">
                <span className="text-[11px] text-zinc-600 dark:text-zinc-400">README（留空用包内 README.md）</span>
                <textarea
                  value={readme}
                  onChange={(event) => setReadme(event.target.value)}
                  rows={5}
                  placeholder="留空则使用插件包根目录的 README.md"
                  className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] font-mono text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                />
              </label>

              <label className="block">
                <span className="text-[11px] text-zinc-600 dark:text-zinc-400">发布说明</span>
                <input
                  value={notes}
                  onChange={(event) => setNotes(event.target.value)}
                  placeholder="这个版本改了什么？"
                  className="mt-0.5 w-full px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-lg text-[11px] text-zinc-900 dark:text-zinc-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                />
              </label>

              <div className="text-[10px] text-zinc-400 dark:text-zinc-500 leading-4">
                上传内容为插件包目录（自动跳过隐藏文件、node_modules、target）。插件库会从包根
                index.ts 校验 name / description / author / version；版本号留空时采用包内版本。
                简介与 README 留空时，发布后会自动采用包内 metadata.description 与 README.md。
              </div>
            </>
          )}
        </div>

        <div className="flex items-center justify-end space-x-2 px-5 py-3 bg-zinc-50/70 dark:bg-[#151518] border-t border-zinc-100 dark:border-[#232326]">
          {result ? (
            <button
              onClick={onClose}
              className="px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-white text-xs font-medium transition-colors"
            >
              完成
            </button>
          ) : (
            <>
              <button
                onClick={onClose}
                disabled={submitting}
                className="px-3.5 py-1.5 rounded-xl border border-zinc-200 dark:border-[#2e2e33] text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-xs font-medium transition-colors disabled:opacity-50"
              >
                取消
              </button>
              <button
                onClick={submit}
                disabled={submitting || !configured}
                className="flex items-center space-x-1 px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 disabled:opacity-50 text-white text-xs font-medium shadow-sm transition-colors"
              >
                {submitting ? <Loader2 size={12} className="animate-spin" /> : <Upload size={12} />}
                <span>{submitting ? '上传中…' : '上传并发布'}</span>
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
