//! 装配期收集**插件工具**（P1 · SP-16）。
//!
//! # 这一步把"插件"从"列表"变成"能力"
//!
//! 到 SP-15 之前，插件工具已经能被调用，但没有任何地方**把它们装进 catalog**——
//! 界面列得出来、模型却看不见。这里补的就是那一段：扫描磁盘插件 → 装载 → 看它导出什么 →
//! 把**名字已在注册表里**的那些包成 [`PluginTool`]。
//!
//! # 三条口径
//!
//! 1. **只认注册表**（决策 1 的建议值）：名字不在注册表里的导出**不装**，并产
//!    `plugin.skipped{tool_not_registered}` 诊断——让"插件写了工具却没生效"有据可查，
//!    而不是静默忽略（AGENTS §15 的那个坑）。
//! 2. **失败安全**：某个插件装载失败只跳过它自己，不影响其它插件与内置工具
//!    （与 `tools_for_toolkits` 对"未知工具包"炸掉装配**刻意不同**：工具包是本项目自己的
//!    声明，插件是第三方输入）。
//! 3. **装配期只做一次**：装载是"能跑起来吗"的判据（`load` 会真的执行一遍插件装载流程），
//!    所以这里顺手也把"这个插件根本装不起来"挡在了启动期。

use std::path::Path;
use std::sync::Arc;

use agent_base::ports::{
    LoadRequest, ModuleHandle, PluginTrust, ScriptBudget, ScriptCtx, ScriptHost, ScriptSource, Tool,
};
use agent_base::testing::NeverCancel;

use crate::plugins::{PluginManager, PluginSkillDeclaration};
use crate::scripts::PluginTool;

/// 一个被跳过的插件工具声明（装配期发现的问题，交给调用方报诊断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPluginTool {
    pub plugin_id: String,
    pub export: String,
    /// 为什么没装：`tool_not_registered` / `unregistered_plugin`（内置插件不走脚本）等。
    pub reason: String,
}

/// 装配结果。
#[derive(Default)]
pub struct PluginToolAssembly {
    /// 装进 catalog 的工具：——id 是**来源标注**，装配与埋点都用它归因。
    pub tools: Vec<(String, Arc<dyn Tool>)>,
    /// 看见但没装的声明（**要报出来**，别静默跳过）。
    pub skipped: Vec<SkippedPluginTool>,
    /// 插件声明的**提示词片段**（SP-44）：(plugin_id, 片段)。没有就是空。
    pub prompt_fragments: Vec<(String, agent_base::ports::ScriptPromptFragment)>,
    /// 插件声明的**技能**（SP-45）：(plugin_id, 插件名, 声明)。没有就是空。
    ///
    /// 收在这里而不是发明新的扫描路径：技能与工具、提示词片段一样，都是"插件装载期
    /// 才拿得到"的东西——三条都从同一次 `load` 出来，就不会出现"工具装了、技能没装"。
    pub skills: Vec<(String, String, PluginSkillDeclaration)>,
    /// 装载成功的插件**句柄**（P3a·SP-50）：钩子装配要按句柄调处理器。
    ///
    /// 只给句柄而不是在这里注册钩子：分级校验与注册顺序是 `hooks` 模块的事，
    /// 装配收集器不该替它做决定（那样"谁能拦"就有了第二份判据）。
    pub handles: Vec<(String, PluginTrust, ModuleHandle)>,
    /// 真的装载成功的插件数（诊断用）。
    pub plugins_loaded: usize,
}

/// 扫磁盘插件并把可用的工具装好。
///
/// 只处理 `scope != "builtin"` 的插件：内置插件的工具是**编译进来的正则实现**
/// （`agent_toolkit::plugin_tools`），不走脚本沙箱——把内置插件也当脚本装，
/// 会凭空多出一条执行路径。
pub async fn collect_plugin_tools(
    plugin_mgr: &PluginManager,
    host: &Arc<dyn ScriptHost>,
    workspace: &Path,
    budget: ScriptBudget,
) -> PluginToolAssembly {
    let mut out = PluginToolAssembly::default();
    let ws_str = workspace.to_string_lossy().to_string();

    // 内置插件没有脚本可装载（下面按 `scope == "builtin"` 全跳过），但它们的**技能**
    // 必须和磁盘插件技能一起进装配——否则插件卡上写着"1 个技能"，模型那边一个字都看不到。
    // 启停口径同磁盘插件：停用的插件连技能一起不装。
    out.skills = crate::plugins::builtins::builtin_plugin_skills(&plugin_mgr.load_disabled_plugins());

    for item in plugin_mgr.load_plugins(Some(&ws_str)) {
        if item.scope == "builtin" {
            continue;
        }
        if !item.enabled {
            out.skipped.push(SkippedPluginTool {
                plugin_id: item.id.clone(),
                export: String::new(),
                reason: "plugin_disabled".to_string(),
            });
            continue;
        }
        // `file_path` 是扫描期认定的脚本路径；没有它就没法装载（例如插件是个目录包）
        if item.file_path.trim().is_empty() {
            out.skipped.push(SkippedPluginTool {
                plugin_id: item.id.clone(),
                export: String::new(),
                reason: "no_script_entry".to_string(),
            });
            continue;
        }

        let ctx = ScriptCtx {
            budget,
            cancel: &NeverCancel,
            thread_id: "",
            origin_id: Some(item.id.as_str()),
        };

        // P4·SP-71：能力声明里的**认不出的 token** 在这里如实报出来。
        //
        // 为什么装配层也要读一遍（装载期已经读过）：装载期那一遍只影响"授予了什么"，
        // 而"你写错了声明"是**作者必须看到**的信息——不报出来他会一直以为 `nett` 生效了。
        // 解析函数是同一个（`needs::parse_needs`），所以两处不会说不一样的话。
        if let Ok(source) = std::fs::read_to_string(&item.file_path) {
            let declared = crate::scripts::needs::parse_needs(&source);
            for token in declared.unknown {
                out.skipped.push(SkippedPluginTool {
                    plugin_id: item.id.clone(),
                    export: String::new(),
                    reason: format!("unknown_need:{token}"),
                });
            }
        }

        let handle = match host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(std::path::PathBuf::from(&item.file_path)),
                    workspace: Some(workspace.to_path_buf()),
                },
                &ctx,
            )
            .await
        {
            Ok(h) => h,
            Err(e) => {
                // **失败安全**：一个插件装不起来，不影响别的插件与内置工具
                out.skipped.push(SkippedPluginTool {
                    plugin_id: item.id.clone(),
                    export: String::new(),
                    reason: format!("plugin_load_failed:{e:?}"),
                });
                continue;
            }
        };
        out.plugins_loaded += 1;
        // P3a·SP-50：句柄一并交出去（钩子装配要按句柄调处理器）。
        // 磁盘插件一律是**第三方**档：本仓没有"标记受信"的机制，不能靠目录名猜
        // （猜错的后果是把控制面的拦下权交给一个第三方插件）。
        out.handles.push((
            item.id.clone(),
            PluginTrust::ThirdParty,
            handle.clone(),
        ));

        // P2·SP-44：插件声明的提示词片段一并收下（带 plugin_id，进 PromptPlan 时才能归因）
        for f in host.prompt_fragments(&handle).unwrap_or_default() {
            out.prompt_fragments.push((item.id.clone(), f));
        }

        // P2·SP-45：插件声明的技能同理（带 plugin_id + 插件名，合并进技能扫描结果时贴归因）
        for s in host.skills(&handle).unwrap_or_default() {
            out.skills.push((
                item.id.clone(),
                item.name.clone(),
                PluginSkillDeclaration {
                    name: s.name,
                    description: s.description,
                    path: s.path,
                    body: s.body,
                    when_to_use: s.when_to_use,
                    user_invocable: s.user_invocable,
                },
            ));
        }

        for declaration in host.tool_declarations(&handle).unwrap_or_default() {
            let export = declaration.name.clone();
            let tool = if agent_toolkit::find_tool_descriptor(&export).is_some() {
                PluginTool::new(&export, host.clone(), handle.clone(), item.id.clone())
            } else {
                PluginTool::from_declaration(&declaration, host.clone(), handle.clone(), item.id.clone())
            };
            match tool {
                Ok(mut tool) => {
                    tool = tool.with_budget(budget);
                    out.tools.push((item.id.clone(), Arc::new(tool) as Arc<dyn Tool>));
                }
                Err(_) => {
                    // 决策 1 的口径：未注册的工具名不装，但要**说清楚**
                    out.skipped.push(SkippedPluginTool {
                        plugin_id: item.id.clone(),
                        export,
                        reason: "tool_not_registered".to_string(),
                    });
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
        // 内置插件（插件工坊）的技能是常驻的，别混进"这条插件声明了几条"的断言
        fn plugin_only(
            asm: &PluginToolAssembly,
        ) -> Vec<&(String, String, crate::plugins::PluginSkillDeclaration)> {
            asm.skills
                .iter()
                .filter(|(id, _, _)| !id.starts_with("builtin:"))
                .collect()
        }

    use super::*;
    use crate::scripts::TsScriptHost;

    fn temp_ws(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("a_da_asm_{tag}_{}_{}", std::process::id(), nanos));
        std::fs::create_dir_all(crate::plugins::PluginManager::agent_extensions_dir(&dir.to_string_lossy()))
            .expect("建临时工作区失败");
        dir
    }

    fn write_package(
        workspace: &std::path::Path,
        package: &str,
        tools: &[&str],
        prompts: &[&str],
        skills: &[&str],
        hooks: &[&str],
        factory_body: &str,
    ) -> std::path::PathBuf {
        let package_root = crate::plugins::PluginManager::agent_extensions_dir(&workspace.to_string_lossy())
            .join(package);
        std::fs::create_dir_all(&package_root).expect("创建插件包目录失败");
        let metadata = serde_json::json!({
            "name": package,
            "description": format!("{package} extension"),
            "tools": { "count": tools.len(), "names": tools },
            "systemPrompt": { "count": prompts.len(), "names": prompts },
            "skills": { "count": skills.len(), "names": skills },
            "hooks": { "count": hooks.len(), "names": hooks },
        });
        let source = format!(
            "const metadata = {}; export default function(ada: any) {{ ada.metadata = metadata; {} }}",
            metadata, factory_body
        );
        let entry = package_root.join("index.ts");
        std::fs::write(&entry, source).expect("写插件包入口失败");
        entry
    }

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("a_da_asm_home_{tag}_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Plugin-declared tools are dynamically registered with a conservative descriptor.
    #[tokio::test]
    async fn test_collects_only_registered_names_and_reports_the_rest() {
        let ws = temp_ws("mix");
        write_package(
            &ws,
            "p",
            &["read_file", "my_custom_tool"],
            &["factory-prompt"],
            &["factory-skill"],
            &["afterTurn"],
            r#"
              ada.registerTool({
                name: 'read_file', description: '注册表里有', parameters: { type: 'object', properties: {} },
                execute: async () => ({ ok: true, output: 'from-plugin' })
              });
              ada.registerTool({
                name: 'my_custom_tool', description: '插件自定义工具', parameters: { type: 'object', properties: {} },
                execute: async () => ({ ok: true, output: 'custom-output' })
              });
              ada.registerSystemPrompt({ id: 'factory-prompt', content: 'factory prompt fragment' });
              ada.registerSkill({ name: 'factory-skill', description: 'factory skill', body: 'factory skill body' });
              ada.on('turn_end', async (event: any, ctx: any) => ({ action: 'note', note: `${ctx.point}:${event.step}` }));
            "#,
        );

        let mgr = PluginManager::with_home(temp_home("mix"));
        let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
        let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;

        let names: Vec<String> = asm.tools.iter().map(|(_, t)| t.descriptor().name.clone()).collect();
        assert_eq!(names, vec!["read_file".to_string(), "my_custom_tool".to_string()]);
        assert_eq!(asm.plugins_loaded, 1);
        assert_eq!(asm.prompt_fragments.len(), 1, "factory system prompt registration should be assembled");
        assert_eq!(plugin_only(&asm).len(), 1, "factory skill registration should be assembled");
        assert_eq!(asm.handles.len(), 1, "factory ada.on should be collected as a hook declaration");
        let custom = asm.tools.iter().find(|(_, tool)| tool.descriptor().name == "my_custom_tool").unwrap();
        assert!(!custom.1.descriptor().is_readonly(), "unknown plugin tools fail closed as write tools");
        assert_eq!(custom.1.descriptor().approval, agent_base::domain::ApprovalPolicy::Always);

        let cancel = agent_base::testing::NeverCancel;
        let sink = agent_base::testing::RecordingSink::new();
        let scope = agent_base::testing::MockScope::new(ws.to_string_lossy().to_string());
        let call = agent_base::domain::ToolCall {
            id: "dynamic-call".to_string(),
            name: "my_custom_tool".to_string(),
            args: serde_json::json!({}),
        };
        let ctx = agent_base::ports::ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &sink,
            thread_id: "dynamic-thread",
            hooks: None,
        };
        let receipt = custom.1.execute(&call, &ctx).await;
        assert!(receipt.ok(), "custom declared tool should execute: {receipt:?}");
        assert_eq!(receipt.output, "custom-output");

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 没装起来的插件只是被跳过——不影响别的插件与内置工具（**装配不炸**）。
    #[tokio::test]
    async fn test_broken_plugin_is_skipped_without_breaking_assembly() {
        let ws = temp_ws("broken");
        let dir = crate::plugins::PluginManager::agent_extensions_dir(&ws.to_string_lossy());
        let bad_dir = dir.join("bad");
        std::fs::create_dir_all(&bad_dir).expect("建坏插件目录失败");
        std::fs::write(bad_dir.join("index.ts"), "export default function(ada: any) {\n").expect("写坏插件失败");
        write_package(
            &ws,
            "good",
            &["write_file"],
            &[],
            &[],
            &[],
            "ada.registerTool({ name: 'write_file', description: 'ok', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
        );

        let mgr = PluginManager::with_home(temp_home("broken"));
        let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
        let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;

        let names: Vec<String> = asm.tools.iter().map(|(_, t)| t.descriptor().name.clone()).collect();
        assert_eq!(names, vec!["write_file".to_string()], "好的那个必须照装：{names:?}");
        assert!(
            asm.skipped
                .iter()
                .any(|s| s.reason.starts_with("plugin_load_failed")),
            "装不起来的那个必须如实报出来：{:?}",
            asm.skipped
        );

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 内置插件**不走脚本沙箱**：它们的工具是编译进来的 Rust 实现。
    #[tokio::test]
    async fn test_builtin_plugins_are_not_loaded_as_scripts() {
        let ws = temp_ws("builtin");
        let mgr = PluginManager::with_home(temp_home("builtin"));
        let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
        let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;
        assert!(asm.tools.is_empty(), "没有磁盘插件时不该装出任何工具");
        assert_eq!(asm.plugins_loaded, 0);
        let _ = std::fs::remove_dir_all(&ws);
    }

/// SP-44：插件声明的**提示词片段**必须被收下，并带住 `plugin_id`（进提示词才能归因）。
#[tokio::test]
async fn test_plugin_prompt_fragments_are_collected_with_plugin_id() {
    let ws = temp_ws("prompts");
    write_package(
        &ws, "p", &["read_file"], &["house-style"], &[], &[],
        "ada.registerSystemPrompt({ id: 'house-style', content: '本项目一律用 tabs。' }); ada.registerTool({ name: 'read_file', description: 'x', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
    );

    let mgr = PluginManager::with_home(temp_home("prompts"));
    let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
    let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;

    assert_eq!(asm.tools.len(), 1, "工具照收");
    assert_eq!(asm.prompt_fragments.len(), 1, "提示词片段必须被收下：{:?}", asm.skipped);
    let (plugin_id, frag) = &asm.prompt_fragments[0];
    assert!(
        plugin_id.contains("p"),
        "片段必须带得住 plugin_id（否则进提示词无法归因）：{plugin_id}"
    );
    assert_eq!(frag.id, "house-style");
    assert_eq!(frag.content, "本项目一律用 tabs。");

    let _ = std::fs::remove_dir_all(&ws);
}

/// 没声明提示词片段的插件就是空——**空不是错误**（不该因此跳过插件）。
#[tokio::test]
async fn test_plugin_without_prompts_yields_no_fragments() {
    let ws = temp_ws("noprompts");
    write_package(
        &ws, "p", &["read_file"], &[], &[], &[],
        "ada.registerTool({ name: 'read_file', description: 'x', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
    );

    let mgr = PluginManager::with_home(temp_home("noprompts"));
    let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
    let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;
    assert!(asm.prompt_fragments.is_empty());
    assert_eq!(asm.tools.len(), 1, "没有提示词片段不影响工具照装");
    let _ = std::fs::remove_dir_all(&ws);
}

/// SP-45：插件声明的**技能**必须被收下，并带住 `plugin_id` / 插件名（合并进技能扫描
/// 结果时贴归因）。工具与技能出自**同一次装载**，所以不会出现"工具装了、技能没装"。
#[tokio::test]
async fn test_plugin_skills_are_collected_with_plugin_id() {
    let ws = temp_ws("skills");
    write_package(
        &ws, "p", &["read_file"], &[], &["deploy"], &[],
        "ada.registerSkill({ name: 'deploy', description: '发布', body: '先跑测试', whenToUse: '发布时' }); ada.registerTool({ name: 'read_file', description: 'x', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
    );

    let mgr = PluginManager::with_home(temp_home("skills"));
    let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
    let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;

    assert_eq!(asm.tools.len(), 1, "工具照装：{:?}", asm.skipped);
    let from_plugins = plugin_only(&asm);
    assert_eq!(from_plugins.len(), 1, "技能必须被收下：{:?}", asm.skills);
    let (plugin_id, plugin_name, decl) = from_plugins[0];
    assert!(!plugin_id.is_empty(), "必须带得住 plugin_id（否则无法归因）");
    assert!(!plugin_name.is_empty(), "界面要显示是哪个插件带来的");
    assert_eq!(decl.name, "deploy");
    assert_eq!(decl.when_to_use.as_deref(), Some("发布时"));

    // 转成扫描结果的形状：id 带插件前缀、来源标 plugin（界面据此分流显示）
    let summary = decl.to_summary(plugin_id, plugin_name);
    assert_eq!(summary.id, format!("plugin:{plugin_id}:deploy"));
    assert_eq!(summary.scope, "plugin");
    assert_eq!(summary.plugin_id.as_deref(), Some(plugin_id.as_str()));

    let _ = std::fs::remove_dir_all(&ws);
}

/// 没有技能声明的插件就是空——**空不是错误**（不该因此跳过插件）。
#[tokio::test]
async fn test_plugin_without_skills_yields_no_skills() {
    let ws = temp_ws("npskills");
    write_package(
        &ws, "p", &["read_file"], &[], &[], &[],
        "ada.registerTool({ name: 'read_file', description: 'x', parameters: { type: 'object', properties: {} }, execute: async () => ({ ok: true, output: 'ok' }) });",
    );

    let mgr = PluginManager::with_home(temp_home("npskills"));
    let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
    let asm = collect_plugin_tools(&mgr, &host, &ws, ScriptBudget::default()).await;
    assert!(
        plugin_only(&asm).is_empty(),
        "这条插件没声明技能，就一条都不该有：{:?}",
        asm.skills
    );
    // 唯一那条来自内置插件工坊（它自带"怎么写插件"的技能，与本条插件无关）
    assert_eq!(asm.skills.len(), 1, "应只剩内置插件工坊的技能：{:?}", asm.skills);
    assert!(asm.skills[0].0.starts_with("builtin:"));
    assert_eq!(asm.tools.len(), 1, "没有技能声明不影响工具照装");
    let _ = std::fs::remove_dir_all(&ws);
}
}
