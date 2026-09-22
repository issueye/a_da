/**
 * The settings dialog.
 *
 * A modal, not an anchored popup: it covers the window and takes the wheel, so
 * nothing behind it can be clicked or scrolled while it is open. Header and body
 * are split by a rule, and the body is a two-column layout — sections on the
 * left, the section's form on the right. 供应商 is the first section because
 * that is the one setting the app cannot work without.
 */

import React, { useEffect, useState } from 'react'
import {
  PROVIDER_PRESETS,
  envOverrides,
  readSavedConfig,
  configPath,
  type ProviderConfig,
} from '../agent/config'
import type { AgentStore } from '../agent/store'
import { C, editorTheme, FONT_MONO, M } from '../theme'
import { Checkbox, Icon, IconButton } from './controls'
import type { IconName } from '../icons'

type SectionId = 'provider' | 'workspace'

const SECTIONS: { id: SectionId; label: string; icon: IconName }[] = [
  { id: 'provider', label: '供应商', icon: 'server' },
  { id: 'workspace', label: '工作区', icon: 'folder' },
]

function Field({
  label,
  hint,
  testId,
  value,
  placeholder,
  mono,
  onChange,
  onEnter,
  autoFocus,
}: {
  label: string
  hint?: string
  testId: string
  value: string
  placeholder?: string
  mono?: boolean
  onChange: (next: string) => void
  onEnter?: () => void
  autoFocus?: boolean
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 5, width: '100%' }}>
      <text style={{ fontSize: 12, lineHeight: 16, fontWeight: 600, color: C.secondary }}>
        {label}
      </text>
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          height: 32,
          width: '100%',
          paddingLeft: 9,
          paddingRight: 9,
          borderRadius: 8,
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: C.borderStrong,
        }}
      >
        <input
          testId={testId}
          value={value}
          placeholder={placeholder}
          autoFocus={autoFocus}
          theme={editorTheme()}
          style={{
            flexGrow: 1,
            minWidth: 0,
            fontSize: 12.5,
            color: C.text,
            fontFamily: mono ? FONT_MONO : undefined,
            backgroundColor: '#00000000',
            borderWidth: 0,
          }}
          onChange={(event) => onChange(event.value ?? '')}
          onSubmit={onEnter}
        />
      </div>
      {hint ? (
        <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>{hint}</text>
      ) : null}
    </div>
  )
}

function Button({
  label,
  onClick,
  testId,
  primary,
  disabled,
}: {
  label: string
  onClick: () => void
  testId: string
  primary?: boolean
  disabled?: boolean
}) {
  return (
    <div
      testId={testId}
      role="button"
      aria-label={label}
      onClick={disabled ? undefined : onClick}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'center',
        height: 30,
        paddingLeft: 13,
        paddingRight: 13,
        borderRadius: 8,
        flexShrink: 0,
        cursor: disabled ? 'default' : 'pointer',
        backgroundColor: primary ? C.inverse : C.raised,
        borderWidth: primary ? 0 : 1,
        borderColor: C.borderStrong,
        opacity: disabled ? 0.5 : 1,
        hover: { opacity: disabled ? 0.5 : 0.88 },
      }}
    >
      <text
        style={{
          fontSize: 12.5,
          lineHeight: 16,
          fontWeight: primary ? 600 : 500,
          color: primary ? C.onInverse : C.text,
        }}
      >
        {label}
      </text>
    </div>
  )
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'flex-start',
        gap: 12,
        width: '100%',
        paddingTop: 6,
        paddingBottom: 6,
      }}
    >
      <text style={{ fontSize: 12, lineHeight: 17, color: C.tertiary, width: 78, flexShrink: 0 }}>
        {label}
      </text>
      {/* `minWidth: 0` so a long value wraps instead of overflowing the card. */}
      <text
        style={{
          fontSize: 12,
          lineHeight: 17,
          color: C.text,
          flexGrow: 1,
          flexShrink: 1,
          minWidth: 0,
        }}
      >
        {value}
      </text>
    </div>
  )
}

export function SettingsDialog({ store }: { store: AgentStore }) {
  const [section, setSection] = useState<SectionId>('provider')
  const [draft, setDraft] = useState<ProviderConfig>({ baseUrl: '', apiKey: '', model: '' })
  const [preset, setPreset] = useState('openai')
  const [status, setStatus] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const overrides = envOverrides()

  // The file is the source of truth for this form; the environment may shadow it.
  useEffect(() => {
    void (async () => {
      const saved = await readSavedConfig()
      const next: ProviderConfig = {
        baseUrl: saved.baseUrl ?? PROVIDER_PRESETS[0]!.baseUrl,
        apiKey: saved.apiKey ?? '',
        model: saved.model ?? PROVIDER_PRESETS[0]!.model,
        contextWindow: typeof saved.contextWindow === 'number' ? saved.contextWindow : (PROVIDER_PRESETS[0]!.contextWindow ?? 128000),
        supportsImages: typeof saved.supportsImages === 'boolean' ? saved.supportsImages : (PROVIDER_PRESETS[0]!.supportsImages ?? false),
      }
      setDraft(next)
      const match = PROVIDER_PRESETS.find((item) => item.baseUrl && item.baseUrl === next.baseUrl)
      setPreset(match?.id ?? 'custom')
    })()
  }, [])

  const update = (patch: Partial<ProviderConfig>) => setDraft((current) => ({ ...current, ...patch }))

  const choosePreset = (id: string) => {
    setPreset(id)
    const found = PROVIDER_PRESETS.find((item) => item.id === id)
    if (found && found.id !== 'custom') {
      update({
        baseUrl: found.baseUrl,
        model: found.model,
        contextWindow: found.contextWindow,
        supportsImages: found.supportsImages,
      })
    }
  }

  const save = async () => {
    setBusy(true)
    const message = await store.saveProvider(draft)
    setBusy(false)
    setError(message)
    setStatus(message ? null : '已保存，下一轮对话生效')
  }

  const test = async () => {
    setBusy(true)
    setStatus(null)
    const detail = await store.checkProvider(draft)
    setBusy(false)
    setError(null)
    setStatus(detail)
  }

  return (
    <div
      onKeyDown={(event) => {
        if (event.key === 'escape') store.setSettings(false)
      }}
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: C.scrim,
        // `auto` and not the default: a modal has to swallow the wheel too, or
        // it scrolls the transcript behind it.
        pointerEvents: 'auto',
      }}
    >
      <div
        testId="settings-dialog"
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: '70%',
          maxWidth: '92%',
          minWidth: 500,
          height: '80%',
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: C.borderStrong,
          borderRadius: 12,
          overflow: 'hidden',
          boxShadow: {
            offsetX: 0,
            offsetY: 18,
            blurRadius: 48,
            spreadRadius: 0,
            color: C.shadowStrong,
          },
        }}
      >
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            height: M.settingsHeader,
            flexShrink: 0,
            paddingLeft: 16,
            paddingRight: 8,
          }}
        >
          <Icon name="settings" size={14} color={C.secondary} />
          <text
            style={{
              fontSize: 13.5,
              lineHeight: 18,
              fontWeight: 600,
              color: C.text,
              paddingLeft: 8,
            }}
          >
            设置
          </text>
          <div style={{ flexGrow: 1 }} />
          <IconButton
            icon="close"
            testId="settings-close"
            label="关闭设置"
            onClick={() => store.setSettings(false)}
          />
        </div>

        {/* The rule between header and body. */}
        <div style={{ height: 1, flexShrink: 0, backgroundColor: C.border }} />

        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            flexGrow: 1,
            minHeight: 0,
            alignItems: 'stretch',
            borderBottomLeftRadius: 11,
            borderBottomRightRadius: 11,
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              width: M.settingsNav,
              flexShrink: 0,
              padding: 10,
              gap: 2,
              backgroundColor: C.sidebar,
              borderBottomLeftRadius: 11,
            }}
          >
            {SECTIONS.map((item) => (
              <div
                key={item.id}
                testId={`settings-nav-${item.id}`}
                role="button"
                aria-label={item.label}
                onClick={() => setSection(item.id)}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 8,
                  height: M.row,
                  paddingLeft: 8,
                  paddingRight: 8,
                  borderRadius: 6,
                  cursor: 'pointer',
                  backgroundColor: section === item.id ? C.raised : '#00000000',
                  borderWidth: section === item.id ? 1 : 0,
                  borderColor: C.border,
                  hover: { backgroundColor: section === item.id ? C.raised : C.overlay },
                }}
              >
                <Icon
                  name={item.icon}
                  size={13}
                  color={section === item.id ? C.text : C.tertiary}
                />
                <text
                  style={{
                    fontSize: 12.5,
                    lineHeight: 16,
                    fontWeight: section === item.id ? 600 : 500,
                    color: section === item.id ? C.text : C.secondary,
                  }}
                >
                  {item.label}
                </text>
              </div>
            ))}
          </div>

          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              flexGrow: 1,
              minWidth: 0,
              overflowY: 'scroll',
              paddingTop: 16,
              paddingBottom: 16,
              paddingLeft: 18,
              paddingRight: 18,
              gap: 13,
              borderBottomRightRadius: 11,
            }}
          >
            {section === 'provider' ? (
              <>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
                  <text style={{ fontSize: 12, lineHeight: 16, fontWeight: 600, color: C.secondary }}>
                    供应商
                  </text>
                  <div style={{ display: 'flex', flexDirection: 'row', flexWrap: 'wrap', gap: 6 }}>
                    {PROVIDER_PRESETS.map((item) => (
                      <div
                        key={item.id}
                        testId={`settings-preset-${item.id}`}
                        role="button"
                        aria-label={item.label}
                        onClick={() => choosePreset(item.id)}
                        style={{
                          display: 'flex',
                          flexDirection: 'row',
                          alignItems: 'center',
                          height: 26,
                          paddingLeft: 9,
                          paddingRight: 9,
                          borderRadius: 7,
                          cursor: 'pointer',
                          backgroundColor: preset === item.id ? C.chip : C.raised,
                          borderWidth: 1,
                          borderColor: preset === item.id ? C.borderStrong : C.border,
                          hover: { backgroundColor: C.chip },
                        }}
                      >
                        <text
                          style={{
                            fontSize: 12,
                            lineHeight: 16,
                            color: preset === item.id ? C.text : C.secondary,
                          }}
                        >
                          {item.label}
                        </text>
                      </div>
                    ))}
                  </div>
                  <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>
                    选一个预设会填好接口地址与常用模型，也可以直接在下面改。
                  </text>
                </div>

                <Field
                  label="接口地址"
                  testId="settings-base-url"
                  value={draft.baseUrl}
                  placeholder="https://api.openai.com/v1"
                  mono
                  autoFocus
                  onChange={(next) => update({ baseUrl: next })}
                  hint="任意 OpenAI 兼容端点，末尾不需要 /chat/completions。"
                />
                <Field
                  label="API Key"
                  testId="settings-api-key"
                  value={draft.apiKey}
                  placeholder="sk-…"
                  mono
                  onChange={(next) => update({ apiKey: next })}
                  hint={`以明文保存到 ${configPath()}，只有你自己的账户能读。`}
                />
                <Field
                  label="模型"
                  testId="settings-model"
                  value={draft.model}
                  placeholder="gpt-4o-mini"
                  mono
                  onChange={(next) => update({ model: next })}
                  onEnter={() => void save()}
                />
                <Field
                  label="上下文上限 (Tokens)"
                  testId="settings-context-window"
                  value={draft.contextWindow ? String(draft.contextWindow) : ''}
                  placeholder="128000"
                  mono
                  onChange={(next) => {
                    const clean = next.replace(/\D/g, '')
                    update({ contextWindow: clean ? parseInt(clean, 10) : undefined })
                  }}
                  hint="模型最大上下文 Token 数（例如 128000、200000、1000000）。用于输入框下方遥测栏精确计算会话占比。"
                />
                <Checkbox
                  testId="settings-supports-images"
                  checked={Boolean(draft.supportsImages)}
                  onChange={(checked) => update({ supportsImages: checked })}
                  label="支持图片输入 (Vision)"
                  hint="启用多模态图片输入。勾选后输入框可添加并发送图片给视觉多模态大模型。"
                />

                {overrides.length ? (
                  <text style={{ fontSize: 11.5, lineHeight: 16, color: C.accent }}>
                    当前环境变量 {overrides.join('、')} 优先级更高，运行时会用它们的值。
                  </text>
                ) : null}

                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    paddingTop: 2,
                  }}
                >
                  <Button label="保存" testId="settings-save" primary onClick={() => void save()} disabled={busy} />
                  <Button label="测试连接" testId="settings-test" onClick={() => void test()} disabled={busy} />
                  <div style={{ flexGrow: 1 }} />
                  {status ? (
                    <text
                      style={{
                        fontSize: 11.5,
                        lineHeight: 16,
                        color: error ? C.accent : C.success,
                        flexShrink: 1,
                      }}
                    >
                      {error ?? status}
                    </text>
                  ) : null}
                </div>
              </>
            ) : (
              <>
                <InfoRow label="当前项目" value={store.project} />
                <InfoRow
                  label="索引"
                  value={
                    store.workspaceInfo.scanning
                      ? '索引中…'
                      : `${store.workspaceInfo.files} 个文件 · ${store.workspaceInfo.dirs} 个目录`
                  }
                />
                <InfoRow
                  label="项目数"
                  value={`${store.projects.length} 个项目 · 当前项目 ${store.projectThreads.length} 个会话`}
                />
                <InfoRow
                  label="文件访问"
                  value="Agent 只能读取与修改当前项目目录内的文件，所有路径都会被解析回项目根目录。"
                />
                <div style={{ height: 1, backgroundColor: C.border, marginTop: 4, marginBottom: 4 }} />
                <InfoRow
                  label="配置文件"
                  value={configPath()}
                />
                <InfoRow
                  label="工具"
                  value="list_files · read_file · search_files · write_file · edit_file · run_command"
                />
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
