use super::types::{
    LoadedPlugin, LoadedPluginContributions, PluginItem, PluginManifest, PluginScope,
    PluginToolDeclaration, PluginToolInfo,
};

pub struct BuiltinPluginDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub tools: &'static [(&'static str, &'static str)], // (name, description)
}

pub const BUILTIN_PLUGINS: &[BuiltinPluginDefinition] = &[
    BuiltinPluginDefinition {
        id: "git-tools",
        name: "Git 变更与协作工具 (git-tools)",
        description: "提供结构化 Git 状态、安全限长 Diff 提取与近期提交历史检索能力，辅助精准掌握版本改动。",
        tools: &[
            ("git_status", "获取当前 Git 工作区的状态信息，包含当前分支、暂存修改、未暂存修改与未跟踪文件。"),
            ("git_diff", "获取工作区或特定文件的 Git Diff 变更内容。"),
            ("git_log", "获取近期提交历史列表与简要描述。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "code-outline",
        name: "代码大纲与结构提取 (code-outline)",
        description: "提取类、接口、函数与结构体骨架签名，快速理解大型代码文件架构。",
        tools: &[
            ("code_outline", "提取代码文件中的关键类、函数、接口骨架签名及行号索引。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "project-inspector",
        name: "项目侦测与依赖分析 (project-inspector)",
        description: "自动探查项目技术栈、主入口、构建系统与环境依赖。",
        tools: &[
            ("project_inspect", "探测当前工作区的技术栈类型、构建工具与关键配置文件路径。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "test-runner",
        name: "测试运行与结果归因 (test-runner)",
        description: "自动化执行项目单测，解析输出并归因定位失败用例。",
        tools: &[
            ("run_tests", "自动化运行项目测试用例，解析测试通过率与失败错误栈。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "batch-ops",
        name: "批量文件读写与精准替换 (batch-ops)",
        description: "高效进行多文件同时写入与统一规则批量替换。",
        tools: &[
            ("batch_replace", "在多个指定文件中按统一规则执行模式搜索与文本批量替换。"),
            ("batch_write", "一次性原子写入或更新多个目标代码文件。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "decision",
        name: "决策评估与准入门禁 (decision)",
        description: "提供结构化概率决策自评与基于 diff/文件/文本的代码变动准入门禁评估能力。",
        tools: &[
            ("decide", "基于加权规则与线索对二选一、多选或评分问题进行结构化决策判断。"),
            ("check_gate", "对工作区 git diff、指定文件或文本内容执行严格准入门禁规则检查。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "approval-guard",
        name: "审批安全守卫 (approval-guard)",
        description: "提供前置操作拦截、高危指令阻断与审批策略管控。",
        tools: &[],
    },
    BuiltinPluginDefinition {
        id: "ask-user",
        name: "向用户提问 (ask-user)",
        description: "向用户发起单选、多选按钮交互或自由文本输入提问。",
        tools: &[
            ("ask_user", "在交互界面向用户发起单选选择或补充说明提问并阻塞等待答复。"),
        ],
    },
    BuiltinPluginDefinition {
        id: "ponytail",
        name: "马尾调度管理器 (ponytail)",
        description: "提示词、技能生命周期管理与上下文任务调度。",
        tools: &[],
    },
];

pub fn get_builtin_plugin_items(disabled_ids: &std::collections::HashSet<String>) -> Vec<PluginItem> {
    BUILTIN_PLUGINS
        .iter()
        .map(|def| {
            let full_id = format!("builtin:{}", def.id);
            let enabled = !disabled_ids.contains(&full_id) && !disabled_ids.contains(def.id);

            let tools_decl: Vec<PluginToolDeclaration> = def
                .tools
                .iter()
                .map(|(name, desc)| {
                    let param_schema = agent_toolkit::find_tool_descriptor(name)
                        .map(|d| d.schema.clone())
                        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
                    PluginToolDeclaration {
                        name: name.to_string(),
                        label: Some(name.to_string()),
                        description: desc.to_string(),
                        parameters: param_schema,
                    }
                })
                .collect();

            let tools_info: Vec<PluginToolInfo> = def
                .tools
                .iter()
                .map(|(name, desc)| {
                    let descriptor_opt = agent_toolkit::find_tool_descriptor(name);
                    let param_schema = descriptor_opt
                        .map(|d| d.schema.clone())
                        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
                    let is_write = descriptor_opt
                        .map(|d| !d.is_readonly())
                        .unwrap_or(false);
                    PluginToolInfo {
                        name: name.to_string(),
                        description: desc.to_string(),
                        parameters: Some(param_schema),
                        is_write,
                    }
                })
                .collect();

            let manifest = PluginManifest {
                id: full_id.clone(),
                name: def.name.to_string(),
                description: def.description.to_string(),
                version: Some("1.0.0".to_string()),
                author: Some("a-da 官方团队".to_string()),
                scope: Some(PluginScope::Builtin),
            };

            let loaded_plugin = LoadedPlugin {
                manifest: manifest.clone(),
                contributions: LoadedPluginContributions {
                    tools: tools_decl,
                    config_schema: None,
                },
                declarative: true,
                status: "ready".to_string(),
                diagnostics: Vec::new(),
            };

            PluginItem {
                plugin: loaded_plugin,
                id: full_id.clone(),
                name: def.name.to_string(),
                file_name: format!("{}.ts", def.id),
                file_path: format!("(builtin):{}", def.id),
                scope: "builtin".to_string(),
                enabled,
                status: "ready".to_string(),
                version: Some("1.0.0".to_string()),
                diagnostics: Vec::new(),
                tools: tools_info,
                skills: Vec::new(),
                prompts: Vec::new(),
                is_package: Some(false),
                error: None,
                size_bytes: 0,
                updated_at: 1727740800000,
            }
        })
        .collect()
}
