/**
 * AGENT BASE 线协议方法与事件常量定义 (L0 + L1 + L2)
 * 由 spec/proto/base.json 与 ada-coding.ext.json 单源规范对齐。
 */

export const PROTOCOL_VERSION = '1.0'

// 会话与连接 (Session)
export const SESSION_INITIALIZE = 'session.initialize'
export const SESSION_SNAPSHOT = 'session.snapshot'

// UI 外壳控制 (UI)
export const UI_SET_SHELL = 'ui.setShell'
export const UI_OPEN_TAB = 'ui.openTab'
export const UI_CLOSE_TAB = 'ui.closeTab'
export const UI_ACTIVE_THREAD = 'ui.activeThread'
export const UI_ACTIVE_PROJECT = 'ui.activeProject'

// 文件系统 (FS)
export const FS_ROOTS = 'fs.roots'
export const FS_LIST = 'fs.list'
export const FS_MKDIR = 'fs.mkdir'
export const FS_READ_BASE64 = 'fs.read_base64'

// 线程生命周期 (Thread)
export const THREAD_FOCUS = 'thread.focus'
export const THREAD_CREATE = 'thread.create'
export const THREAD_DELETE = 'thread.delete'
export const THREAD_SEND = 'thread.send'
export const THREAD_ABORT = 'thread.abort'
export const THREAD_UPDATE = 'thread.update'
export const THREAD_COMPACT = 'thread.compact'
export const THREAD_SET_MODE = 'thread.setMode'
export const THREAD_SET_WORKSPACE = 'thread.setWorkspace'
export const THREAD_EDIT_AND_RESEND = 'thread.editAndResend'
export const THREAD_RETRY = 'thread.retry'

// 审批与问答 (Approval & Question)
export const APPROVAL_DECIDE = 'approval.decide'
export const QUESTION_ANSWER = 'question.answer'

// 指令队列 (Queue)
export const QUEUE_CLEAR = 'queue.clear'
export const QUEUE_PROMOTE = 'queue.promote'
export const QUEUE_REMOVE = 'queue.remove'

// 子智能体调度 (Subagent)
export const SUBAGENT_PROFILE_LIST = 'subagentProfile.list'
export const SUBAGENT_PROFILE_SET_ENABLED = 'subagentProfile.setEnabled'
export const SUBAGENT_PROFILE_DELETE = 'subagentProfile.delete'

// 工作区与配置 (Workspace & Config)
// W5-T2：`workspace.set` / `config.update` 已删除（无 dispatch 臂、前端从未调用）
export const WORKSPACE_ADD = 'workspace.add'
export const WORKSPACE_REMOVE = 'workspace.remove'
export const WORKSPACE_ENTRIES = 'workspace.entries'
export const WORKSPACE_OPEN_PUBLIC = 'workspace.openPublic'

export const CONFIG_GET = 'config.get'
export const CONFIG_PRESETS = 'config.presets'
export const CONFIG_SET_PROVIDER = 'config.setProvider'
export const CONFIG_CHECK_PROVIDER = 'config.checkProvider'
export const CONFIG_SET_APPROVAL = 'config.setApproval'
export const CONFIG_SET_EFFORT = 'config.setEffort'

// 模型供应商管理 (Provider)
export const PROVIDER_LIST = 'provider.list'
export const PROVIDER_SAVE = 'provider.save'
export const PROVIDER_DELETE = 'provider.delete'
export const PROVIDER_SET_ACTIVE = 'provider.setActive'
export const PROVIDER_FETCH_MODELS = 'provider.fetchModels'

// 插件管理 (Plugin)
export const PLUGIN_LIST = 'plugin.list'
export const PLUGIN_CAPABILITIES_SET = 'plugin.capabilities.set'
export const PLUGIN_CONFIG_SET = 'plugin.config.set'
export const PLUGIN_SECRET_SET = 'plugin.secret.set'
export const PLUGIN_SET_ENABLED = 'plugin.setEnabled'
export const PLUGIN_DELETE = 'plugin.delete'
export const PLUGIN_CREATE_TEMPLATE = 'plugin.createTemplate'
export const PLUGIN_BUILTIN_CATALOG = 'plugin.builtinCatalog'

// 提示词管理 (Prompt)
export const PROMPT_LIST = 'prompt.list'
export const PROMPT_SET_ENABLED = 'prompt.setEnabled'
export const PROMPT_CREATE = 'prompt.create'
export const PROMPT_UPDATE = 'prompt.update'
export const PROMPT_DELETE = 'prompt.delete'

// 技能管理 (Skill)
export const SKILL_LIST = 'skill.list'
export const SKILL_SET_ENABLED = 'skill.setEnabled'
export const SKILL_CREATE = 'skill.create'
export const SKILL_DELETE = 'skill.delete'

// 改动与检查点回滚 (Change & Rollback)
export const CHANGE_COUNT = 'change.count'
export const CHANGE_LIST = 'change.list'
export const CHANGE_REVERT_CARD = 'change.revertCard'
export const CHANGE_REVERT_CHECKPOINT = 'change.revertCheckpoint'
export const CHANGE_REVERT_FILE = 'change.revertFile'
export const CHANGE_REVERT_ALL = 'change.revertAll'

// 调试与统计 (Debug & Stats)
export const DEBUG_TRACE = 'debug.trace'
export const DEBUG_LOG_CLEAR = 'debug.log.clear'
export const DEBUG_HOST_INFO = 'debug.hostInfo'
export const STATS_PROMPT_CHARS = 'stats.promptChars'

// 服务端推送事件主题 (Events)
export const EVT_STATE_SNAPSHOT = 'evt.state.snapshot'
export const EVT_MESSAGE_DELTA = 'evt.message.delta'
export const EVT_CARD_UPDATED = 'evt.card.updated'
