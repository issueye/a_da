use super::types::{
    LoadedPlugin, LoadedPluginContributions, PluginItem, PluginManifest, PluginScope,
    PluginSkillDeclaration, PluginToolDeclaration, PluginToolInfo,
};

/// 内置插件的一条**技能**声明。
///
/// 为什么内置插件也需要它：内置插件没有 `index.ts`，跑不了沙箱，也没有磁盘上的
/// `skills/<name>/SKILL.md`。而技能是"教模型什么时候用这个插件、怎么写不踩坑"的唯一载体——
/// 缺了它，插件卡上只剩几个工具名。于是像 `tools` 一样在定义里静态声明一份**单一真源**：
/// 卡片（[`get_builtin_plugin_items`]）与装配（`scripts::collect_plugin_tools`）都读它。
///
/// ⚠️ 这**不是**给磁盘插件用的形态：第三方插件的技能必须走
/// `skills/<name>/SKILL.md`（`docs/SKILL_SPEC.md`），脚本里 `registerSkill` 已移除。
pub struct BuiltinPluginSkill {
    pub name: &'static str,
    pub description: &'static str,
    pub when_to_use: Option<&'static str>,
    pub body: &'static str,
}

impl BuiltinPluginSkill {
    /// 转成插件技能声明（与磁盘插件 `skills/<name>/SKILL.md` 解析出来的结构一致）。
    pub fn declaration(&self) -> PluginSkillDeclaration {
        PluginSkillDeclaration {
            name: self.name.to_string(),
            path: None,
            description: self.description.to_string(),
            body: self.body.to_string(),
            when_to_use: self.when_to_use.map(str::to_string),
            user_invocable: Some(true),
        }
    }
}

pub struct BuiltinPluginDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub tools: &'static [(&'static str, &'static str)], // (name, description)
    /// 该插件自带的技能（内置插件没有脚本，只能在这里声明）
    pub skills: &'static [BuiltinPluginSkill],
}

pub const BUILTIN_PLUGINS: &[BuiltinPluginDefinition] = &[
    BuiltinPluginDefinition {
        id: "code-outline",
        name: "代码大纲与结构提取 (code-outline)",
        description: "提取类、接口、函数与结构体骨架签名，快速理解大型代码文件架构。",
        tools: &[
            ("code_outline", "提取代码文件中的关键类、函数、接口骨架签名及行号索引。"),
        ],
        skills: &[],
    },
    BuiltinPluginDefinition {
        id: "project-inspector",
        name: "项目侦测与依赖分析 (project-inspector)",
        description: "自动探查项目技术栈、主入口、构建系统与环境依赖。",
        tools: &[
            ("project_inspect", "探测当前工作区的技术栈类型、构建工具与关键配置文件路径。"),
        ],
        skills: &[],
    },
    BuiltinPluginDefinition {
        id: "test-runner",
        name: "测试运行与结果归因 (test-runner)",
        description: "自动化执行项目单测，解析输出并归因定位失败用例。",
        tools: &[
            ("run_tests", "自动化运行项目测试用例，解析测试通过率与失败错误栈。"),
        ],
        skills: &[],
    },
    BuiltinPluginDefinition {
        id: "batch-ops",
        name: "批量文件读写与精准替换 (batch-ops)",
        description: "高效进行多文件同时写入与统一规则批量替换。",
        tools: &[
            ("batch_replace", "在多个指定文件中按统一规则执行模式搜索与文本批量替换。"),
            ("batch_write", "一次性原子写入或更新多个目标代码文件。"),
        ],
        skills: &[],
    },
    BuiltinPluginDefinition {
        id: "plugin-forge",
        name: "插件工坊 (plugin-forge)",
        description: "在工作区里直接生成可加载的插件包骨架，并附一份「怎么写插件」的技能。",
        tools: &[
            ("create_plugin", "生成可加载的插件包骨架（index.ts + README + 技能），metadata 已按注册项对齐。"),
        ],
        skills: &[BuiltinPluginSkill {
            name: "plugin-forge",
            description: "a-da 插件怎么写：包结构、四类贡献、metadata 对齐、能力声明与常见失败",
            when_to_use: Some("新增插件、修改已有插件、或排查「插件写了却不生效」时"),
            body: r#"# 插件工坊（plugin-forge）

## 什么时候读它
- 要**新增**一个插件能力（工具 / 提示词 / 技能 / 钩子）：先用 `create_plugin` 生成骨架，再只改 `execute`。
- 要**改**已有插件、或排查「插件没生效」：对照下面「metadata」「常见失败」两节。

## 目录与发现规则
- 只认包根：`<root>/<id>/index.ts`（或 `index.js`），且必须 `export default` 一个工厂函数。
  `<root>` 是工作区 `<ws>/.ada/agent-coding/extensions`，或全局 `~/.a-da/agent-coding/extensions`。
- 插件标识就是目录名：工作区插件是 `workspace:<id>`，全局是 `global:<id>`。改目录名 = 换一个插件。
- 散落的 `foo.ts`、更深的 `src/index.ts` 都不会被发现（扫描只看包根）。

## 生成骨架
`create_plugin({ id: "todo-guard", name: "待办守卫", description: "拦截没有验收标准的 todo" })`
`id` 只允许小写字母 / 数字 / 连字符；目录已存在必须显式 `overwrite: true` 才覆盖。
生成后在「插件与能力中心」确认启用——贡献在**装配期**结算，改完要重新加载插件才生效。

## 三类脚本贡献 + 一类磁盘贡献（技能）
- `ada.registerTool({ name, label?, description, parameters, execute })` —— 给模型一个工具。
- `ada.registerSystemPrompt({ id, content })` —— 加一段系统提示词（带 `plugin:<id>` 归因）；正文为空会被丢掉。
- `ada.on(event, handler, { actions, order })` —— 注册事件处理器。**声明即授权**：`actions` 里没有的动作运行期不生效。
  事件名必须是 `event_*`：`event_before_tool_call`（可用 observe/note/deny/replace）、`event_after_tool_call`、
  `event_before_turn`、`event_after_turn`、`event_before_context`、`event_before_provider_request`、
  `event_before_provider_headers`、`event_after_provider_response`。
- **技能不在脚本里注册**：它是磁盘上的目录 `skills/<name>/SKILL.md`（Agent Skills 规范，
  见 `docs/SKILL_SPEC.md`），宿主装配期直接扫描。frontmatter 必须有 `name`（**等于目录名**，
  1-64 个小写字母/数字/连字符）与 `description`（1-1024，写清"做什么 + 什么时候用"）；
  可选 `license` / `compatibility` / `metadata` / `allowed-tools`。技能目录下可放
  `references/`、`scripts/`、`assets/`，相对路径以技能目录为根。
  ⚠️ 旧的 `ada.registerSkill({...})` 已移除：调用会被忽略并记 `skill_spec_violation:*` 诊断。

## metadata：要让宿主看见，且与脚本注册逐项对齐
- 写法是**工厂里第一句** `ada.metadata = { ... }`：宿主在 `await desc(api)` 之后读 `api.metadata`。
- **名称 / 描述**是宿主加载的必填项；**作者 / 版本**在发布到插件市场（plugin-hub）时必填：
  插件库会从包根 `index.ts` 静态提取 metadata，缺 `author` / `version` 会直接拒绝发布，
  且 `version` 必须与发布版本号一致。
- 写成 `export const metadata` 无效：转译把 `export default fn` 译成
  `module.exports = exports.default = fn`，挂在 exports 上的东西一起被换掉 →
  装载期报「插件包入口缺少 metadata 对象」。
- `tools` / `systemPrompt` / `events` 的 `count` 与 `names` 是**量出来的**：
  必须与注册**逐项、按顺序**一致；多注册一条就要同步改，否则装载期直接拒。
- `skills` 是**声明性**的（宿主以目录扫描为准），不参与上面那条一致性校验。

## execute 的两个坑
- 形参：**一个**形参是 `(args)`，**两个**形参是 `(callId, args)`。别按习惯写 `(args, ctx)`。
- 返回值：字符串 / `{ ok, output, details }` / 抛异常，三种都认；抛异常时错误文案带文件与行号。

## 能力声明（源码首部 40 行内的注释）
`// @ada-needs fs,net`
- **有指令就只有列出的那些**（`net` 不附带 `fs`）；没有指令则按宿主地板（fs 开、net 关）。
- 可用值：`fs`（读写）、`fs=read`、`fs=write`、`net`（或 `fetch`）、`none`。
- 认不出的 token 不授予，并在装配期报 `unknown_need:<token>`。

## 常见失败
- 「缺少 metadata 对象」→ 没在工厂里写 `ada.metadata = ...`。
- 「metadata tools.count=N，实际为 M」→ 注册项与 metadata 不同步。
- 插件在、模型看不到工具 → 插件被停用，或贡献还没重载。
- 工具永远调不到 → 名字含空格 / 中文（`create_plugin` 会当场拒），或 `execute` 形参写错。
- 技能没生效（诊断里 `skill_spec_violation:*`）→ 检查三件事：入口是不是
  `skills/<name>/SKILL.md`（不是散装 `.md`）、frontmatter 的 `name` 是否**等于目录名**、
  `description` 是否非空且 ≤1024。
- 改了 `index.ts` 没变化 → 贡献是装配期结算的：重新加载插件 / 新开一轮。

## 别做的事
- 别在插件里直接用 `fs` / `fetch` 却漏了 `@ada-needs`——沙箱里它们是 undefined。
- 别为「看起来完整」给 `events.count` 写 1；它是量出来的。"#,
        }],
    },
    BuiltinPluginDefinition {
        id: "ponytail",
        name: "马尾调度管理器 (ponytail)",
        description: "提示词、技能生命周期管理与上下文任务调度。",
        tools: &[],
        skills: &[],
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
                        // 内置插件的工具是编译进基座的实现，读写性来自注册表描述符；
                        // 这里的 `access` 只是卡片投影的占位（没有脚本声明可采信）。
                        access: "write".to_string(),
                        long_running: false,
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
                // 内置插件没有脚本，技能只能来自静态声明——卡片与装配读的是同一份
                // （`PluginSkillDeclaration::to_summary` 会给它 `plugin:<full_id>:<name>` 的 id，
                //  于是界面能标出"这条技能是哪个插件带来的"）
                skills: if enabled {
                    def.skills
                        .iter()
                        .map(|s| s.declaration().to_summary(&full_id, def.name))
                        .collect()
                } else {
                    // 停用 = 贡献一条都不参与装配，卡片上也别挂着"看起来还在生效"的技能
                    Vec::new()
                },
                prompts: Vec::new(),
                // 内置插件的事件不在这里凭空填：它们由事件运行时注册后才有定论，
                // 静态清单一律为空（界面读装配期的活体视图）
                events: Vec::new(),
                is_package: Some(false),
                error: None,
                size_bytes: 0,
                updated_at: 1727740800000,
            }
        })
        .collect()
}

/// 内置插件声明的技能（**已按启停过滤**），供装配期与磁盘插件技能合流。
///
/// 为什么必须有这一步：`scripts::collect_plugin_tools` 会跳过所有 `scope == "builtin"` 的条目
/// （内置插件没有脚本可装载），于是它们的技能会**永远进不了提示词**——
/// 界面上插件卡写着"1 个技能"，模型那边一个字都看不到。这条函数就是补上那一段的真源出口。
///
/// 顺序与 `BUILTIN_PLUGINS` 一致（官方技能排在第三方插件技能之前），便于 prompt 字节报表对账。
pub fn builtin_plugin_skills(
    disabled_ids: &std::collections::HashSet<String>,
) -> Vec<(String, String, PluginSkillDeclaration)> {
    let mut out = Vec::new();
    for def in BUILTIN_PLUGINS {
        let full_id = format!("builtin:{}", def.id);
        if disabled_ids.contains(&full_id) || disabled_ids.contains(def.id) {
            continue;
        }
        for skill in def.skills {
            out.push((full_id.clone(), def.name.to_string(), skill.declaration()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// 插件工坊必须自带技能：没有它，模型只知道有个 `create_plugin` 工具，
    /// 不知道插件怎么写（包结构 / metadata / 能力声明 / execute 形参）。
    #[test]
    fn test_plugin_forge_ships_a_usable_skill() {
        let items = get_builtin_plugin_items(&HashSet::new());
        let forge = items
            .iter()
            .find(|i| i.id == "builtin:plugin-forge")
            .expect("插件工坊必须在内置插件清单里");

        assert_eq!(forge.skills.len(), 1, "插件工坊应带且仅带 1 条技能");
        let skill = &forge.skills[0];
        assert_eq!(skill.name, "plugin-forge");
        assert_eq!(skill.scope, "plugin", "插件技能要能被界面单独标出来");
        assert_eq!(skill.plugin_id.as_deref(), Some("builtin:plugin-forge"));
        assert_eq!(skill.plugin_name.as_deref(), Some(forge.name.as_str()));

        // 技能正文要覆盖真正会踩的坑（少一条，模型就会照着猜）
        for expect in [
            "create_plugin",
            "ada.metadata",
            "@ada-needs",
            "count",
            "execute",
            "index.ts",
        ] {
            assert!(
                skill.body.contains(expect),
                "技能正文缺少关键点 `{expect}`：\n{}",
                skill.body
            );
        }
        assert!(
            skill.when_to_use.is_some(),
            "技能要写清「什么时候用」，否则模型不知道该不该翻它"
        );
    }

    /// 停用插件 → 它的技能一起下线（界面与提示词同口径，不能一边显示一边还在花字节）。
    #[test]
    fn test_disabled_builtin_plugin_loses_its_skill() {
        let mut disabled = HashSet::new();
        disabled.insert("builtin:plugin-forge".to_string());

        let items = get_builtin_plugin_items(&disabled);
        let forge = items.iter().find(|i| i.id == "builtin:plugin-forge").unwrap();
        assert!(!forge.enabled);
        assert!(forge.skills.is_empty(), "停用的插件不该还挂着技能在卡片上");

        assert!(
            builtin_plugin_skills(&disabled).is_empty(),
            "停用的插件技能也不许进装配"
        );
        assert_eq!(
            builtin_plugin_skills(&HashSet::new()).len(),
            1,
            "启用时应恰好 1 条（插件工坊的唯一技能）"
        );
    }
}
