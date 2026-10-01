/// 协议版本
pub const PROTOCOL_VERSION: &str = "1.0";

// 会话与连接
pub const SESSION_INITIALIZE: &str = "session.initialize";
pub const SESSION_SNAPSHOT: &str = "session.snapshot";

// UI 外壳
pub const UI_SET_SHELL: &str = "ui.setShell";
pub const UI_OPEN_TAB: &str = "ui.openTab";
pub const UI_CLOSE_TAB: &str = "ui.closeTab";
pub const UI_ACTIVE_THREAD: &str = "ui.activeThread";
pub const UI_ACTIVE_PROJECT: &str = "ui.activeProject";

// 文件系统操作
pub const FS_ROOTS: &str = "fs.roots";
pub const FS_LIST: &str = "fs.list";
pub const FS_MKDIR: &str = "fs.mkdir";

// 线程与会话
pub const THREAD_FOCUS: &str = "thread.focus";
pub const THREAD_CREATE: &str = "thread.create";
pub const THREAD_DELETE: &str = "thread.delete";
pub const THREAD_SEND: &str = "thread.send";
pub const THREAD_ABORT: &str = "thread.abort";
pub const THREAD_UPDATE: &str = "thread.update";

// 审批与问答
pub const APPROVAL_DECIDE: &str = "approval.decide";
pub const QUESTION_ANSWER: &str = "question.answer";

// 工作区与配置
pub const WORKSPACE_SET: &str = "workspace.set";
pub const WORKSPACE_ADD: &str = "workspace.add";
pub const WORKSPACE_REMOVE: &str = "workspace.remove";
pub const WORKSPACE_ENTRIES: &str = "workspace.entries";
pub const CONFIG_UPDATE: &str = "config.update";
pub const CONFIG_GET: &str = "config.get";
pub const CONFIG_PRESETS: &str = "config.presets";
pub const CONFIG_SET_PROVIDER: &str = "config.setProvider";
pub const CONFIG_CHECK_PROVIDER: &str = "config.checkProvider";
pub const CONFIG_SET_APPROVAL: &str = "config.setApproval";
pub const CONFIG_SET_EFFORT: &str = "config.setEffort";

// 调试与检查点
pub const DEBUG_TRACE: &str = "debug.trace";
pub const DEBUG_LOG_CLEAR: &str = "debug.log.clear";
pub const DEBUG_HOST_INFO: &str = "debug.hostInfo";
pub const STATS_PROMPT_CHARS: &str = "stats.promptChars";
pub const PLUGIN_BUILTIN_CATALOG: &str = "plugin.builtinCatalog";
pub const CHANGE_COUNT: &str = "change.count";
pub const CHANGE_LIST: &str = "change.list";
pub const CHANGE_REVERT_CARD: &str = "change.revertCard";
pub const CHANGE_REVERT_CHECKPOINT: &str = "change.revertCheckpoint";
pub const CHANGE_REVERT_FILE: &str = "change.revertFile";
pub const CHANGE_REVERT_ALL: &str = "change.revertAll";

// 插件管理
pub const PLUGIN_LIST: &str = "plugin.list";
pub const PLUGIN_CAPABILITIES_SET: &str = "plugin.capabilities.set";
pub const PLUGIN_CONFIG_SET: &str = "plugin.config.set";
pub const PLUGIN_SECRET_SET: &str = "plugin.secret.set";
pub const PLUGIN_SET_ENABLED: &str = "plugin.setEnabled";
pub const PLUGIN_DELETE: &str = "plugin.delete";
pub const PLUGIN_CREATE_TEMPLATE: &str = "plugin.createTemplate";

// 提示词管理
pub const PROMPT_LIST: &str = "prompt.list";
pub const PROMPT_SET_ENABLED: &str = "prompt.setEnabled";
pub const PROMPT_CREATE: &str = "prompt.create";
pub const PROMPT_UPDATE: &str = "prompt.update";
pub const PROMPT_DELETE: &str = "prompt.delete";

// 技能与子智能体
pub const SKILL_LIST: &str = "skill.list";
pub const SKILL_SET_ENABLED: &str = "skill.setEnabled";
pub const SKILL_CREATE: &str = "skill.create";
pub const SKILL_DELETE: &str = "skill.delete";
pub const SUBAGENT_PROFILE_LIST: &str = "subagentProfile.list";
pub const SUBAGENT_PROFILE_SET_ENABLED: &str = "subagentProfile.setEnabled";
pub const SUBAGENT_PROFILE_DELETE: &str = "subagentProfile.delete";

// 增强线程、队列与工作区
pub const THREAD_COMPACT: &str = "thread.compact";
pub const THREAD_SET_MODE: &str = "thread.setMode";
pub const THREAD_SET_WORKSPACE: &str = "thread.setWorkspace";
pub const THREAD_EDIT_AND_RESEND: &str = "thread.editAndResend";
pub const SUBAGENT_RESUME: &str = "subagent.resume";
pub const WORKSPACE_OPEN_PUBLIC: &str = "workspace.openPublic";
pub const WORKSPACE_RESCAN: &str = "workspace.rescan";
pub const QUEUE_CLEAR: &str = "queue.clear";
pub const QUEUE_PROMOTE: &str = "queue.promote";
pub const QUEUE_REMOVE: &str = "queue.remove";

// 服务端推送事件主题常量
pub const EVT_STATE_SNAPSHOT: &str = "evt.state.snapshot";
pub const EVT_MESSAGE_DELTA: &str = "evt.message.delta";
pub const EVT_CARD_UPDATED: &str = "evt.card.updated";

