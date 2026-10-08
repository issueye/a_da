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
pub const FS_READ_BASE64: &str = "fs.read_base64";

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

// 供应商与多协议模型管理
pub const PROVIDER_LIST: &str = "provider.list";
pub const PROVIDER_SAVE: &str = "provider.save";
pub const PROVIDER_DELETE: &str = "provider.delete";
pub const PROVIDER_SET_ACTIVE: &str = "provider.setActive";
pub const PROVIDER_FETCH_MODELS: &str = "provider.fetchModels";

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
pub const THREAD_RETRY: &str = "thread.retry";
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

/// 全量协议方法列表（L0 核心 + L2 产品扩展）
pub const ALL_METHODS: &[&str] = &[
    SESSION_INITIALIZE,
    SESSION_SNAPSHOT,
    UI_SET_SHELL,
    UI_OPEN_TAB,
    UI_CLOSE_TAB,
    UI_ACTIVE_THREAD,
    UI_ACTIVE_PROJECT,
    FS_ROOTS,
    FS_LIST,
    FS_MKDIR,
    FS_READ_BASE64,
    THREAD_FOCUS,
    THREAD_CREATE,
    THREAD_DELETE,
    THREAD_SEND,
    THREAD_ABORT,
    THREAD_UPDATE,
    THREAD_COMPACT,
    THREAD_SET_MODE,
    THREAD_SET_WORKSPACE,
    THREAD_EDIT_AND_RESEND,
    THREAD_RETRY,
    APPROVAL_DECIDE,
    QUESTION_ANSWER,
    WORKSPACE_SET,
    WORKSPACE_ADD,
    WORKSPACE_REMOVE,
    WORKSPACE_ENTRIES,
    WORKSPACE_OPEN_PUBLIC,
    WORKSPACE_RESCAN,
    CONFIG_UPDATE,
    CONFIG_GET,
    CONFIG_PRESETS,
    CONFIG_SET_PROVIDER,
    CONFIG_CHECK_PROVIDER,
    CONFIG_SET_APPROVAL,
    CONFIG_SET_EFFORT,
    PROVIDER_LIST,
    PROVIDER_SAVE,
    PROVIDER_DELETE,
    PROVIDER_SET_ACTIVE,
    PROVIDER_FETCH_MODELS,
    DEBUG_TRACE,
    DEBUG_LOG_CLEAR,
    DEBUG_HOST_INFO,
    STATS_PROMPT_CHARS,
    PLUGIN_BUILTIN_CATALOG,
    CHANGE_COUNT,
    CHANGE_LIST,
    CHANGE_REVERT_CARD,
    CHANGE_REVERT_CHECKPOINT,
    CHANGE_REVERT_FILE,
    CHANGE_REVERT_ALL,
    PLUGIN_LIST,
    PLUGIN_CAPABILITIES_SET,
    PLUGIN_CONFIG_SET,
    PLUGIN_SECRET_SET,
    PLUGIN_SET_ENABLED,
    PLUGIN_DELETE,
    PLUGIN_CREATE_TEMPLATE,
    PROMPT_LIST,
    PROMPT_SET_ENABLED,
    PROMPT_CREATE,
    PROMPT_UPDATE,
    PROMPT_DELETE,
    SKILL_LIST,
    SKILL_SET_ENABLED,
    SKILL_CREATE,
    SKILL_DELETE,
    SUBAGENT_PROFILE_LIST,
    SUBAGENT_PROFILE_SET_ENABLED,
    SUBAGENT_PROFILE_DELETE,
    SUBAGENT_RESUME,
    QUEUE_CLEAR,
    QUEUE_PROMOTE,
    QUEUE_REMOVE,
];

pub const ALL_EVENTS: &[&str] = &[
    EVT_STATE_SNAPSHOT,
    EVT_MESSAGE_DELTA,
    EVT_CARD_UPDATED,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn test_methods_and_events_have_no_duplicates() {
        let mut method_set = HashSet::new();
        for &m in ALL_METHODS {
            assert!(method_set.insert(m), "发现重复协议方法: {}", m);
        }
        assert_eq!(ALL_METHODS.len(), 76);

        let mut event_set = HashSet::new();
        for &e in ALL_EVENTS {
            assert!(event_set.insert(e), "发现重复事件主题: {}", e);
        }
        assert_eq!(ALL_EVENTS.len(), 3);
    }

    #[test]
    fn test_spec_consistency_across_rust_and_json_spec() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root_dir = manifest_dir.parent().unwrap().parent().unwrap();

        let base_spec_path = root_dir.join("spec/proto/base.json");
        let ext_spec_path = root_dir.join("spec/proto/ada-coding.ext.json");

        assert!(base_spec_path.exists(), "spec/proto/base.json 必须存在");
        assert!(ext_spec_path.exists(), "spec/proto/ada-coding.ext.json 必须存在");

        let base_raw = std::fs::read_to_string(&base_spec_path).unwrap();
        let base_val: serde_json::Value = serde_json::from_str(&base_raw).unwrap();

        let ext_raw = std::fs::read_to_string(&ext_spec_path).unwrap();
        let ext_val: serde_json::Value = serde_json::from_str(&ext_raw).unwrap();

        let mut spec_methods = HashSet::new();
        for item in base_val["methods"].as_array().unwrap() {
            spec_methods.insert(item["name"].as_str().unwrap().to_string());
        }
        for item in ext_val["methods"].as_array().unwrap() {
            spec_methods.insert(item["name"].as_str().unwrap().to_string());
        }

        let rust_methods: HashSet<String> = ALL_METHODS.iter().map(|s| s.to_string()).collect();

        // 验证 spec 定义的方法集合与 Rust 常量集合严格相等（INV-11 协议单源）
        assert_eq!(
            spec_methods, rust_methods,
            "spec 与 Rust 常量存在集合差异！孤儿臂或无臂常量不为 0"
        );

        // 验证 TS 客户端类型也包含这 76 个方法
        let ts_methods_path = manifest_dir.join("client-ts/methods.ts");
        assert!(ts_methods_path.exists(), "client-ts/methods.ts 必须存在");
        let ts_content = std::fs::read_to_string(&ts_methods_path).unwrap();
        for &m in ALL_METHODS {
            assert!(
                ts_content.contains(&format!("'{}'", m)),
                "TS client-ts 缺失协议方法常量: {}",
                m
            );
        }
    }
}


