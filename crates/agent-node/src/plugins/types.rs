use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    Builtin,
    Workspace,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
    /// 插件自述的访问类别：`"read"` = 作者担保只读；缺失/其他一律写。
    ///
    /// 沙箱只做**归一化**（`"read"` → `"read"`，其余 → `"write"`），
    /// 采信与否则由宿主决定（见 `ScriptToolAccess`）。
    #[serde(default)]
    pub access: String,
    /// 作者声明该工具会合法地长时间等待（例如等待用户作答）。
    #[serde(default)]
    pub long_running: bool,
}

/// 插件声明的一段**提示词片段**（P2 · SP-44）。
///
/// 这是插件"给模型加话"的唯一入口：它进 `PromptPlan` 时带 `plugin:<id>` 来源，
/// 于是埋点能回答"这段提示词是谁加的"——而不是只看到一个总数莫名膨胀。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPromptDeclaration {
    /// 片段标识（同一插件内唯一即可）
    pub id: String,
    pub content: String,
}

/// 插件声明的一个**技能**（P2 · SP-45）。
///
/// 插件有两种写法，两条路都通到这里：功能式 `api.registerSkill({...})`，
/// 以及对象式 `export default { skills: [...] }`。收下之后它就是技能三来源之一
/// （目录 / 插件 / 内置，见 `SkillManager::scan_skills_with`），
/// 并带上 `plugin:<id>` 来源——于是"这条技能是谁带来的"在提示词字节报表里看得见。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSkillDeclaration {
    /// 技能名（同一个插件内唯一；合并时同名先到先得，见 `SkillManager`）
    pub name: String,
    /// Optional source file path, resolved by the host inside the plugin/workspace roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub description: String,
    /// 正文。**空正文的技能不装**：它进提示词只会白占字节，而"技能"的意义就是那段规范。
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,
}

impl PluginSkillDeclaration {
    /// 转成扫描结果里的一条技能（`plugin_id` / `plugin_name` 由此**第一次**真正被填上）。
    ///
    /// `scope = "plugin"`：界面据此把插件技能与用户自己的文件技能分开显示
    /// （`skill.list` 的两个既有分支正是按 `scope == "builtin"` 分流的）。
    pub fn to_summary(&self, plugin_id: &str, plugin_name: &str) -> crate::skills::SkillSummary {
        crate::skills::SkillSummary {
            id: format!("plugin:{plugin_id}:{}", self.name),
            name: self.name.clone(),
            description: self.description.clone(),
            body: self.body.clone(),
            // 插件技能没有独立的文件路径：正文来自插件模块的声明，不是磁盘上的 .md
            path: self.path.clone().unwrap_or_default(),
            base_directory: self
                .path
                .as_deref()
                .and_then(|path| std::path::Path::new(path).parent())
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
            scope: "plugin".to_string(),
            enabled: true,
            plugin_name: Some(plugin_name.to_string()),
            plugin_id: Some(plugin_id.to_string()),
            is_file_skill: Some(false),
            user_invocable: self.user_invocable.or(Some(true)),
            when_to_use: self.when_to_use.clone(),
            unknown_frontmatter_keys: Vec::new(),
        }
    }
}

/// 插件声明的一条**钩子注册**（P3a · SP-50）。
///
/// 声明面是 `api.on(point, handler, { actions, order })`（对象形态
/// `export default { hooks: [...] }` 也行）。**声明即授权**：`actions` 里没有的动作，
/// 运行期即使插件返回了也不生效（并记理由）——否则"我声明了 observe、实际却拦下了"就没法解释。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHookDeclaration {
    pub point: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolInfo {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    pub is_write: bool,
}

/// 动作的调用意图。**由调用方声明**，宿主据此拒绝“用只读意图执行写动作”。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginActionIntent {
    Read,
    Write,
}

impl PluginActionIntent {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// 后端插件声明的**界面动作**。
///
/// 这是插件给前端插件用的确定性入口：不是模型工具（不进工具目录、不参与审批），
/// 而是由 SSR 宿主经 `plugin.invoke` 调用的动作。
///
/// `mode` 是**后端自己的声明**，前端 manifest 的 `read` 只是“是否首屏预取”的意图；
/// 后端在 `intent=read` 时拒绝 `mode=write`，所以前端改标只读也绕不过去。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginActionDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub description: String,
    /// `read` / `write`；沙箱里只认 `read`，其余一律按 write（失败安全）。
    pub mode: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

impl PluginActionDeclaration {
    pub fn is_read(&self) -> bool {
        self.mode.trim().eq_ignore_ascii_case("read")
    }
}

/// 后端插件在工厂期声明的**一条动态 HTTP 路由**。
///
/// 路由由 TS 侧自己匹配（支持 `:param` 与 `*rest`），宿主只把整个请求转进来。
/// 因此加接口不需要改 Rust：`ada.route({ method, path, handler })` 就够。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRouteDeclaration {
    /// 人类可读的稳定名字，默认 `METHOD path`。
    pub name: String,
    /// `GET` / `POST` / ... / `*`。
    pub method: String,
    pub path: String,
}

/// 动态路由的归一化 HTTP 响应。
///
/// `body_kind` 只有 `text` 与 `base64` 两种：前者按 UTF-8 直出，后者由宿主解码。
/// 头名统一小写；同名多值（典型是 `set-cookie`）保留为数组。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRouteResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_body_kind")]
    pub body_kind: String,
}

fn default_body_kind() -> String {
    "text".to_string()
}

/// `plugin.invoke` 的失败分类。RPC 层据此映射错误码，前端桥接据此选 HTTP 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginActionError {
    NotFound(String),
    NotReady(String),
    Denied(String),
    Invalid(String),
    Timeout,
    TooLarge,
    Failed(String),
}

impl std::fmt::Display for PluginActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(message) => write!(f, "{message}"),
            Self::NotReady(message) => write!(f, "{message}"),
            Self::Denied(message) => write!(f, "{message}"),
            Self::Invalid(message) => write!(f, "{message}"),
            Self::Timeout => f.write_str("插件动作执行超时"),
            Self::TooLarge => f.write_str("插件动作返回值超过大小上限"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for PluginActionError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnostic {
    pub plugin_id: String,
    pub level: String, // "info" | "warn" | "error"
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPluginContributions {
    #[serde(default)]
    pub tools: Vec<PluginToolDeclaration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    #[serde(default)]
    pub contributions: LoadedPluginContributions,
    pub declarative: bool,
    pub status: String, // "ready" | "not-ready" | "broken" | "conflict"
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    pub plugin: LoadedPlugin,
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub file_path: String,
    pub scope: String, // "builtin" | "workspace" | "global"
    pub enabled: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub tools: Vec<PluginToolInfo>,
    pub skills: Vec<crate::skills::SkillSummary>,
    pub prompts: Vec<serde_json::Value>,
    /// 该插件**真的注册上**的钩子点位（P4）：`plugin.list` 由装配期的活体视图填。
    ///
    /// 静态目录扫描给的永远是空——钩子要在注册期过分级/能力开关才有定论。
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_package: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub size_bytes: u64,
    pub updated_at: u64,
}

/// 插件能力开关。
///
/// 🔴 **每一位都必须有生产消费者**（`cargo xtask verify-wiring` 的 check L2 盯着它）：
/// 界面把这些位渲染成"关掉后会发生什么"，没有消费者的位就是一句谎话——
/// 用户关掉它、什么都不会发生，而界面还在承诺。
///
/// 曾经存在的 `allowPlanModeHooks`（"plan 模式下不跑钩子"）**已删**：Rust 引擎不知道
/// 会话处于哪个 mode（`AgentMode` 只被解析并投影给界面，没有任何执行侧分支），
/// 所以它连"要判什么"都不存在。要做得先把 mode 接进引擎，届时连开关一起加。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilities {
    pub allow_system_prompt_replace: bool,
    pub allow_text_rewrite: bool,
    pub allow_thread_delete_block: bool,
    pub allow_compaction_replace: bool,
    pub allow_third_party_hooks: bool,
    pub allow_builtin_shadow: bool,
    pub hook_timeout_ms: u64,
}

impl Default for PluginCapabilities {
    fn default() -> Self {
        Self {
            allow_system_prompt_replace: true,
            allow_text_rewrite: true,
            allow_thread_delete_block: true,
            allow_compaction_replace: true,
            allow_third_party_hooks: true,
            allow_builtin_shadow: true,
            hook_timeout_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPluginCapabilitiesDto {
    pub capabilities: PluginCapabilities,
    pub invalid: Vec<String>,
    pub overrides: HashMap<String, serde_json::Value>,
}
