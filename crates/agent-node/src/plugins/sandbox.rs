use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ada_ts_engine::{compiler::transpile_ts_module_cached, PureTsRuntime};

use super::types::{
    PluginHookDeclaration, PluginPromptDeclaration, PluginSkillDeclaration, PluginToolDeclaration,
};
use agent_toolkit::ToolResult;

/// 一次插件检查的完整结果（工具 + 提示词片段 + 技能 + 钩子声明）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginInspection {
    pub tools: Vec<PluginToolDeclaration>,
    pub prompts: Vec<PluginPromptDeclaration>,
    /// P2·SP-45：插件声明的技能（没声明就是空数组——**空不是错误**）
    pub skills: Vec<PluginSkillDeclaration>,
    /// P3a·SP-50：插件声明的钩子注册（同上）
    pub hooks: Vec<PluginHookDeclaration>,
}

pub struct PluginSandbox;

impl PluginSandbox {
    /// 读取并转译插件的 TypeScript/JavaScript 源码为沙箱闭包代码
    async fn load_and_transpile(plugin_path: &Path) -> Result<String> {
        let source = tokio::fs::read_to_string(plugin_path)
            .await
            .map_err(|e| anyhow::anyhow!("读取插件文件失败 ({}): {}", plugin_path.display(), e))?;
        // 带缓存的转译（P1·SP-17）：这条路径**每次调用**都会走一遍，而转译是纯函数——
        // 同一个插件被模型调 10 次，老实现白转 9 次。缓存键含内容哈希，改了文件必然重算。
        let js_code = transpile_ts_module_cached(&source, plugin_path.to_str())
            .map_err(|e| anyhow::anyhow!("转译插件 TypeScript 失败 ({}): {}", plugin_path.display(), e))?;
        Ok(js_code)
    }

    /// 在纯 Rust 进程内微内核沙箱中检查插件暴露的工具列表
    ///
    /// 走**全量**环境（CLI/本地脚本口径）。插件路径请用
    /// [`Self::inspect_plugin_with_needs`]——那才是"未声明即不注入"。
    pub async fn inspect_plugin(plugin_path: &Path, workspace: &Path) -> Result<Vec<PluginToolDeclaration>> {
        Self::inspect_plugin_with_needs(plugin_path, workspace, ada_ts_engine::Needs::everything()).await
    }

    /// 同 [`Self::inspect_plugin`]，但按声明的注入清单建沙箱（SP-13）。
    pub async fn inspect_plugin_with_needs(
        plugin_path: &Path,
        workspace: &Path,
        needs: ada_ts_engine::Needs,
    ) -> Result<Vec<PluginToolDeclaration>> {
        Ok(Self::inspect_plugin_full(plugin_path, workspace, needs).await?.tools)
    }

    /// 同 []，但把**提示词片段声明**也带回来（SP-44）。
    pub async fn inspect_plugin_full(
        plugin_path: &Path,
        workspace: &Path,
        needs: ada_ts_engine::Needs,
    ) -> Result<PluginInspection> {
        let js_code = Self::load_and_transpile(plugin_path).await?;
        let runtime = PureTsRuntime::with_environment_and_console(
            Some(workspace.to_path_buf()),
            ada_ts_engine::Limits::default(),
            needs,
            ada_ts_engine::ConsoleMode::Capture,
        );

        let plugin_file_json = serde_json::to_string(&plugin_path.to_string_lossy())?;
        let ws_json = serde_json::to_string(&workspace.to_string_lossy())?;

        // 1. 装载插件模块定义
        // 🔴 **前导脚本压成一行**（P4·SP-73）：插件代码必须从第 1 行开始，
        // 否则报错里的行号会整体偏移（作者照着改会改错地方）。
        // 同行拼接是安全的：插件首行若是行注释，被吃掉的只是那一行注释自身；
        // 若是块注释，它自带收尾。
        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;\n}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );

        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            anyhow::bail!("插件语法或装载错误: {e}");
        }

        // 2. 挂载检查函数
        let inspect_script = format!(
            r#"
            globalThis.__ada_inspect_tools = async function() {{
                try {{
                    const desc = globalThis.__ada_current_plugin;
                    if (!desc) {{
                        return JSON.stringify({{ ok: false, error: "插件未导出有效对象或函数 (export default)" }});
                    }}
                    const ws = {ws_json};
                    let toolsRaw = [];
                    let promptsRaw = [];
                    let skillsRaw = [];
                    let hooksRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {{
                            trace: (msg) => console.log(msg),
                            workspace: ws,
                            registerTool: (tool) => {{ toolsRaw.push(tool); }},
                            // P2·SP-45：不再是空壳——插件可以用它声明技能
                            registerSkill: (s) => {{ skillsRaw.push(s); }},
                            // P2·SP-44：不再是空壳——插件可以用它声明提示词片段
                            registerPrompt: (p) => {{ promptsRaw.push(p); }},
                            // P3a·SP-50：钩子注册（处理器不进 JSON，只收声明）
                            registerHook: (h) => {{ hooksRaw.push(h); }},
                        }};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, error: "插件执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object') {{
                        const raw = desc.tools || [];
                        const rawPrompts = desc.prompts || [];
                        const rawSkills = desc.skills || [];
                        const rawHooks = desc.hooks || [];
                        for (let i = 0; i < rawPrompts.length; i++) {{ promptsRaw.push(rawPrompts[i]); }}
                        for (let i = 0; i < rawSkills.length; i++) {{ skillsRaw.push(rawSkills[i]); }}
                        for (let i = 0; i < rawHooks.length; i++) {{ hooksRaw.push(rawHooks[i]); }}
                        for (let i = 0; i < raw.length; i++) {{
                            const t = raw[i];
                            const inst = typeof t === 'function' ? await t(ws || process.cwd()) : t;
                            if (inst) toolsRaw.push(inst);
                        }}
                    }} else {{
                        return JSON.stringify({{ ok: false, error: "插件导出类型不支持" }});
                    }}

                    const tools = [];
                    for (let i = 0; i < toolsRaw.length; i++) {{
                        const inst = toolsRaw[i];
                        if (!inst || typeof inst !== 'object') continue;
                        tools.push({{
                            name: inst.name,
                            label: inst.label,
                            description: inst.description || '',
                            parameters: inst.parameters || {{}}
                        }});
                    }}
                    const prompts = [];
                    for (let i = 0; i < promptsRaw.length; i++) {{
                        const p = promptsRaw[i];
                        if (!p || typeof p !== 'object') continue;
                        if (typeof p.content !== 'string' || !p.content.trim()) continue;
                        prompts.push({{ id: String(p.id || ('prompt_' + i)), content: p.content }});
                    }}
                    // P2·SP-45：技能声明。**没有名字或没有正文的丢掉**——它们进提示词
                    // 只会白占字节（技能的意义就是那段正文），留着反而让界面出现空壳条目。
                    const skills = [];
                    for (let i = 0; i < skillsRaw.length; i++) {{
                        const s = skillsRaw[i];
                        if (!s || typeof s !== 'object') continue;
                        const name = typeof s.name === 'string' ? s.name.trim() : '';
                        const body = typeof s.body === 'string' ? s.body.trim() : '';
                        if (!name || !body) continue;
                        skills.push({{
                            name: name,
                            description: typeof s.description === 'string' ? s.description : '',
                            body: body,
                            whenToUse: typeof s.whenToUse === 'string' ? s.whenToUse
                                : (typeof s.when_to_use === 'string' ? s.when_to_use : null),
                            userInvocable: typeof s.userInvocable === 'boolean' ? s.userInvocable
                                : (typeof s.user_invocable === 'boolean' ? s.user_invocable : null),
                        }});
                    }}
                    // P3a·SP-50：钩子注册的**声明**（处理器是函数，不进 JSON——
                    // 它在调用期按点位名重新找出来执行）。
                    const hooks = [];
                    for (let i = 0; i < hooksRaw.length; i++) {{
                        const h = hooksRaw[i];
                        if (!h || typeof h !== 'object') continue;
                        const point = typeof h.point === 'string' ? h.point.trim() : '';
                        if (!point) continue;
                        const rawActions = Array.isArray(h.actions) ? h.actions
                            : (typeof h.action === 'string' ? [h.action] : []);
                        const actions = [];
                        for (let j = 0; j < rawActions.length; j++) {{
                            if (typeof rawActions[j] === 'string') actions.push(rawActions[j]);
                        }}
                        hooks.push({{
                            point: point,
                            actions: actions,
                            order: typeof h.order === 'number' ? h.order : 0,
                        }});
                    }}
                    return JSON.stringify({{ ok: true, tools: tools, prompts: prompts, skills: skills, hooks: hooks }});
                }} catch (err) {{
                    // 🔴 带上 `stack`（P4·SP-73）：只拿 `err.message` 的话，报错不带文件与行号。
                    const detail = err && err.stack ? String(err.stack) : String(err);
                    return JSON.stringify({{ ok: false, error: detail }});
                }}
            }};
            "#,
            ws_json = ws_json
        );

        if let Err(e) = runtime.eval_ts(inspect_script, Some("ada_inspect.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化插件检查器失败: {e}");
        }

        // 3. 执行工具检查并设置 15 秒超时
        let inspect_fut = runtime.call_async_fn("__ada_inspect_tools", "{}");
        let resp_str = match tokio::time::timeout(Duration::from_secs(15), inspect_fut).await {
            Ok(res) => {
                runtime.terminate();
                res.map_err(|e| anyhow::anyhow!("执行插件 inspect 失败: {e}"))?
            }
            Err(_) => {
                runtime.terminate();
                anyhow::bail!("检查插件超时（超过 15 秒）");
            }
        };

        let resp: serde_json::Value = serde_json::from_str(&resp_str)?;
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            let tools: Vec<PluginToolDeclaration> = serde_json::from_value(
                resp.get("tools").cloned().unwrap_or(serde_json::json!([])),
            )?;
            // P2·SP-44：提示词声明也一并带回来（没声明就是空数组，不是错误）
            let prompts: Vec<PluginPromptDeclaration> = serde_json::from_value(
                resp.get("prompts").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            // P2·SP-45：技能声明同理（空数组 = 没声明，不是错误）
            let skills: Vec<PluginSkillDeclaration> = serde_json::from_value(
                resp.get("skills").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            // P3a·SP-50：钩子注册同理
            let hooks: Vec<PluginHookDeclaration> = serde_json::from_value(
                resp.get("hooks").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            Ok(PluginInspection {
                tools,
                prompts,
                skills,
                hooks,
            })
        } else {
            let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("检查插件失败");
            anyhow::bail!("{}", err);
        }
    }

    /// 调用插件在某个点位上注册的**钩子**（P3a · SP-50）。
    ///
    /// 与 [`Self::call_tool_budgeted`] 同构（同一套沙箱、同一套限额），差别只在调度：
    /// 按**点位名**找处理器、入参是载荷、出参是动作。刻意**不复用**工具那条路：
    /// 工具是按 export 名调用、结果要包成 `ToolResult`，两件事混在一起会让
    /// "钩子返回的东西被当成工具回执"这种错很难查。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_hook_budgeted(
        plugin_path: &Path,
        point: &str,
        payload: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
        needs: ada_ts_engine::Needs,
        limits: ada_ts_engine::Limits,
    ) -> Result<serde_json::Value> {
        let js_code = Self::load_and_transpile(plugin_path).await?;
        let runtime = PureTsRuntime::with_environment_and_console(
            Some(workspace.to_path_buf()),
            limits,
            needs,
            ada_ts_engine::ConsoleMode::Capture,
        );

        let plugin_file_json = serde_json::to_string(&plugin_path.to_string_lossy())?;
        let ws_json = serde_json::to_string(&workspace.to_string_lossy())?;
        let point_json = serde_json::to_string(point)?;

        // 1. 装载插件模块定义
        // 🔴 **前导脚本压成一行**（P4·SP-73）：插件代码必须从第 1 行开始，
        // 否则报错里的行号会整体偏移（作者照着改会改错地方）。
        // 同行拼接是安全的：插件首行若是行注释，被吃掉的只是那一行注释自身；
        // 若是块注释，它自带收尾。
        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;\n}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );
        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            return Ok(serde_json::json!({ "error": format!("插件装载失败: {e}") }));
        }

        // 2. 挂载钩子调度器
        let call_setup = format!(
            r#"
            globalThis.__ada_call_hook = async function(payload) {{
                try {{
                    const desc = globalThis.__ada_current_plugin;
                    if (!desc) {{
                        return JSON.stringify({{ error: "插件未导出有效对象或函数" }});
                    }}
                    const ws = {ws_json};
                    const targetPoint = {point_json};
                    const eventPayload = payload && payload.__ada_hook_event !== undefined
                        ? payload.__ada_hook_event
                        : payload;
                    const hookCtx = Object.assign(
                        {{ point: targetPoint, workspace: ws }},
                        payload && payload.__ada_hook_ctx !== undefined ? payload.__ada_hook_ctx : {{}},
                    );
                    let hooksRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {{
                            trace: (msg) => console.log(msg),
                            workspace: ws,
                            registerTool: () => {{}},
                            registerSkill: () => {{}},
                            registerPrompt: () => {{}},
                            registerHook: (h) => {{ hooksRaw.push(h); }},
                        }};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ error: "插件初始化执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object') {{
                        const raw = desc.hooks || [];
                        for (let i = 0; i < raw.length; i++) {{ hooksRaw.push(raw[i]); }}
                    }}

                    let target = null;
                    for (let i = 0; i < hooksRaw.length; i++) {{
                        const h = hooksRaw[i];
                        if (h && h.point === targetPoint) {{ target = h; break; }}
                    }}
                    if (!target) {{
                        return JSON.stringify({{ error: "插件未声明点位: " + targetPoint }});
                    }}
                    if (typeof target.handler !== 'function') {{
                        return JSON.stringify({{ error: "点位 " + targetPoint + " 未实现 handler" }});
                    }}

                    // 🔴 入参**已经是对象**：`call_async_fn` 会把传进去的 JSON 串解析成 JS 值
                    // （与工具那条路一致）。再 `JSON.parse` 一次会得到
                    // "expected value at line 1 column 2" 这种跟插件毫不相干的错。
                    // 也**不要**在这里 `const payload = payload || {}`：同名的 const 会
                    // 在 TDZ 里遮蔽参数，Boa 报 "access of uninitialized binding"。
                    // Match pi's extension shape: handlers receive (event, ctx).
                    // The context is data-only and cannot call host capabilities.
                    const res = await target.handler(eventPayload, hookCtx);
                    if (res === null || res === undefined) {{
                        return JSON.stringify({{}});
                    }}
                    if (typeof res === 'string') {{
                        return JSON.stringify({{ action: res }});
                    }}
                    if (typeof res === 'object') {{
                        const out = {{}};
                        if (typeof res.action === 'string') out.action = res.action;
                        if (typeof res.reason === 'string') out.reason = res.reason;
                        if (typeof res.note === 'string') out.note = res.note;
                        if (res.data !== undefined) out.data = res.data;
                        return JSON.stringify(out);
                    }}
                    return JSON.stringify({{}});
                }} catch (err) {{
                    // 🔴 带上 `stack`（P4·SP-73）：只拿 `err.message` 的话，报错不带文件与行号。
                    const detail = err && err.stack ? String(err.stack) : String(err);
                    return JSON.stringify({{ error: detail }});
                }}
            }};
            "#,
            ws_json = ws_json,
            point_json = point_json
        );
        if let Err(e) = runtime.eval_ts(call_setup, Some("ada_hook_caller.js")).await {
            runtime.terminate();
            return Ok(serde_json::json!({ "error": format!("初始化钩子调度器失败: {e}") }));
        }

        let payload_str = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
        let call_fut = runtime.call_async_fn("__ada_call_hook", payload_str);
        let resp_str = match tokio::time::timeout(Duration::from_secs(timeout_s), call_fut).await {
            Ok(res) => {
                runtime.terminate();
                match res {
                    Ok(s) => s,
                    Err(e) => return Ok(serde_json::json!({ "error": format!("钩子执行失败: {e}") })),
                }
            }
            Err(_) => {
                runtime.terminate();
                anyhow::bail!("脚本执行超时（{} 秒）", timeout_s);
            }
        };

        Ok(serde_json::from_str(&resp_str)
            .unwrap_or_else(|_| serde_json::json!({ "error": resp_str })))
    }

    /// 在纯 Rust 进程内微内核沙箱中执行插件工具
    ///
    /// 走**全量**环境（CLI/本地脚本口径）。插件路径请用 [`Self::call_tool_with_needs`]。
    pub async fn call_tool(
        plugin_path: &Path,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
    ) -> Result<ToolResult> {
        Self::call_tool_with_needs(
            plugin_path,
            tool_name,
            args,
            workspace,
            timeout_s,
            ada_ts_engine::Needs::everything(),
        )
        .await
    }

    /// 同 [`Self::call_tool`]，但按声明的注入清单建沙箱（SP-13）。
    ///
    /// 🔴 这是插件路径该用的那个：`needs.net == false` 时沙箱里**连 `fetch` 与
    /// `__native_fetch_async` 都不存在**（见 `Needs` 的说明）。
    pub async fn call_tool_with_needs(
        plugin_path: &Path,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
        needs: ada_ts_engine::Needs,
    ) -> Result<ToolResult> {
        Self::call_tool_budgeted(
            plugin_path,
            tool_name,
            args,
            workspace,
            timeout_s,
            needs,
            ada_ts_engine::Limits::default(),
        )
        .await
    }

    /// 同上，且**由调用方给进程内限额**（SP-11：预算逐项收紧）。
    ///
    /// 墙钟超时只能兜住"等待型"的卡顿；同步死循环要靠进程内指令上限才必然返回
    /// （见 `ada-ts-engine::Limits`）。两者都来自**同一个预算**，所以由这一层一起收。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_tool_budgeted(
        plugin_path: &Path,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
        needs: ada_ts_engine::Needs,
        limits: ada_ts_engine::Limits,
    ) -> Result<ToolResult> {
        let js_code = Self::load_and_transpile(plugin_path).await?;
        let runtime = PureTsRuntime::with_environment_and_console(
            Some(workspace.to_path_buf()),
            limits,
            needs,
            ada_ts_engine::ConsoleMode::Capture,
        );

        let plugin_file_json = serde_json::to_string(&plugin_path.to_string_lossy())?;
        let ws_json = serde_json::to_string(&workspace.to_string_lossy())?;
        let tool_name_json = serde_json::to_string(tool_name)?;

        // 1. 装载插件模块定义
        // 🔴 **前导脚本压成一行**（P4·SP-73）：插件代码必须从第 1 行开始，
        // 否则报错里的行号会整体偏移（作者照着改会改错地方）。
        // 同行拼接是安全的：插件首行若是行注释，被吃掉的只是那一行注释自身；
        // 若是块注释，它自带收尾。
        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;\n}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );

        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            return Ok(ToolResult::error(format!("插件装载失败: {e}")));
        }

        // 2. 挂载工具调度执行器
        let call_setup = format!(
            r#"
            globalThis.__ada_call_tool = async function(args) {{
                try {{
                    const desc = globalThis.__ada_current_plugin;
                    if (!desc) {{
                        return JSON.stringify({{ ok: false, output: "插件未导出有效对象或函数" }});
                    }}
                    const ws = {ws_json};
                    const targetName = {tool_name_json};
                    let toolsRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {{
                            trace: (msg) => console.log(msg),
                            workspace: ws,
                            registerTool: (tool) => {{ toolsRaw.push(tool); }},
                            // 调用路径只找工具：技能/提示词/钩子在这里**收下就丢**是刻意的
                            // （声明清单在 `__ada_inspect_tools` 那条路上探明并挂在句柄上，
                            // 这里再收一遍只会多一份对不上的副本）。但它们**不能不存在**——
                            // 插件工厂调了任何一个都会抛，工具就再也找不到了
                            // （实测过一次：少了 `registerHook`，带钩子的插件连工具都调不动）。
                            registerSkill: () => {{}},
                            registerPrompt: () => {{}},
                            registerHook: () => {{}},
                        }};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, output: "插件初始化执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object') {{
                        const raw = desc.tools || [];
                        for (let i = 0; i < raw.length; i++) {{
                            const t = raw[i];
                            const inst = typeof t === 'function' ? await t(ws || process.cwd()) : t;
                            if (inst) toolsRaw.push(inst);
                        }}
                    }}

                    let toolInst = null;
                    for (let i = 0; i < toolsRaw.length; i++) {{
                        const inst = toolsRaw[i];
                        if (inst && inst.name === targetName) {{
                            toolInst = inst;
                            break;
                        }}
                    }}

                    if (!toolInst) {{
                        return JSON.stringify({{ ok: false, output: "插件中未找到工具: " + targetName }});
                    }}

                    if (typeof toolInst.execute !== 'function') {{
                        return JSON.stringify({{ ok: false, output: "工具 " + targetName + " 未实现 execute 方法" }});
                    }}

                    let rawRes;
                    if (toolInst.execute.length === 1) {{
                        try {{
                            rawRes = await toolInst.execute(args || {{}});
                        }} catch (_) {{
                            rawRes = await toolInst.execute('call_sandbox', args || {{}});
                        }}
                    }} else {{
                        rawRes = await toolInst.execute('call_sandbox', args || {{}});
                    }}

                    if (rawRes === null || rawRes === undefined) {{
                        return JSON.stringify({{ ok: true, output: "" }});
                    }}

                    if (typeof rawRes === 'string') {{
                        return JSON.stringify({{ ok: true, output: rawRes }});
                    }}

                    if (typeof rawRes === 'object') {{
                        const out = {{
                            ok: typeof rawRes.ok === 'boolean' ? rawRes.ok : true,
                            output: typeof rawRes.output === 'string' ? rawRes.output : JSON.stringify(rawRes.output ?? rawRes),
                        }};
                        if (typeof rawRes.patch === 'string') out.patch = rawRes.patch;
                        if (rawRes.details !== undefined) out.details = rawRes.details;
                        if (typeof rawRes.terminate === 'boolean') out.terminate = rawRes.terminate;
                        return JSON.stringify(out);
                    }}

                    return JSON.stringify({{ ok: true, output: String(rawRes) }});
                }} catch (err) {{
                    // 🔴 让错误**继续往外抛**（P4·SP-73）：宿主侧才拿得到 Boa 那份带文件与
                    // 行号的完整文案。在这里压成 `err.message` 的话，作者看到的只有
                    // "undefinedName is not defined"ï¼没有文件、没有行号，正是 SP-73 要修的。
                    throw err;
                }}
            }};
            "#,
            ws_json = ws_json,
            tool_name_json = tool_name_json
        );

        if let Err(e) = runtime.eval_ts(call_setup, Some("ada_caller.js")).await {
            runtime.terminate();
            return Ok(ToolResult::error(format!("初始化插件调度器失败: {e}")));
        }

        let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
        let call_fut = runtime.call_async_fn("__ada_call_tool", args_str);

        let resp_str = match tokio::time::timeout(Duration::from_secs(timeout_s), call_fut).await {
            Ok(res) => {
                runtime.terminate();
                match res {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolResult::error(format!("插件工具执行失败: {e}"))),
                }
            }
            Err(_) => {
                runtime.terminate();
                anyhow::bail!("插件沙箱执行超时（超过 {} 秒）", timeout_s);
            }
        };

        let resp: serde_json::Value = serde_json::from_str(&resp_str)
            .unwrap_or_else(|_| serde_json::json!({ "ok": false, "output": resp_str }));

        let mut res: ToolResult = serde_json::from_value(resp)
            .unwrap_or_else(|e| ToolResult::error(format!("反序列化工具输出失败: {e}")));

        // P1·SP-14：把这次调用捕获到的控制台输出**结构化**带回给调用方。
        //
        // 放在 `details` 里（而不是拼进 `output`）：`output` 是给模型看的，
        // 而控制台输出是给**排查**看的——混进去会污染模型上下文。截断标记与原始字节数
        // 一并带上，免得"看起来只有两行"被误当成"插件只打了两行"。
        let console = runtime.take_console();
        if !console.is_empty() {
            let mut details = res.details.take().unwrap_or_else(|| serde_json::json!({}));
            if let Some(obj) = details.as_object_mut() {
                obj.insert(
                    "console".to_string(),
                    serde_json::json!({
                        "stdout": console.stdout,
                        "stderr": console.stderr,
                        "stdoutBytes": console.stdout_bytes,
                        "stderrBytes": console.stderr_bytes,
                        "truncated": console.truncated,
                    }),
                );
            }
            res.details = Some(details);
        }

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sandbox_inspect_plugin() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_inspect_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("test_plugin.ts");

        let plugin_code = r#"
            import { join } from 'node:path';

            interface ToolConfig {
                prefix: string;
            }

            export default {
                name: "test-plugin",
                description: "测试插件描述",
                tools: [
                    (ws: string) => ({
                        name: "greet_tool",
                        label: "问候工具",
                        description: "返回问候语",
                        parameters: {
                            type: "object",
                            properties: {
                                name: { type: "string" }
                            },
                            required: ["name"]
                        },
                        execute: async (callId: string, args: any) => {
                            return { ok: true, output: `Hello, ${args.name}!` };
                        }
                    }),
                    {
                        name: "static_tool",
                        description: "静态工具",
                        parameters: { type: "object" },
                        execute: async () => {
                            return { ok: true, output: "static ok" };
                        }
                    }
                ]
            };
        "#;

        std::fs::write(&plugin_file, plugin_code).unwrap();

        let tools = PluginSandbox::inspect_plugin(&plugin_file, &temp_dir)
            .await
            .expect("inspect_plugin 应该成功");

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "greet_tool");
        assert_eq!(tools[0].label.as_deref(), Some("问候工具"));
        assert_eq!(tools[0].description, "返回问候语");
        assert_eq!(tools[1].name, "static_tool");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// SP-45：技能声明**两条写法都要收下**（功能式 `registerSkill` / 对象式 `skills`）。
    ///
    /// 顺带钉住"空壳不算技能"：没有正文或没有名字的声明被丢掉——留着只会让界面
    /// 出现一条占着位置却什么也不说的技能。
    #[tokio::test]
    async fn test_sandbox_inspect_collects_skill_declarations() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_skill_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("skill_plugin.ts");

        let plugin_code = r#"
            export default function (api: any) {
                api.registerSkill({
                    name: "deploy-runbook",
                    description: "发布流程",
                    whenToUse: "当用户要发布时",
                    body: "1. 跑测试\n2. 打标签"
                });
                api.registerSkill({ name: "no-body", description: "没有正文，应被丢掉" });
                api.registerSkill({ body: "没有名字，应被丢掉" });
            }
        "#;

        std::fs::write(&plugin_file, plugin_code).unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &plugin_file,
            &temp_dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("inspect 应成功");

        assert_eq!(inspected.skills.len(), 1, "只该留下那一条完整声明：{:?}", inspected.skills);
        let s = &inspected.skills[0];
        assert_eq!(s.name, "deploy-runbook");
        assert_eq!(s.when_to_use.as_deref(), Some("当用户要发布时"));
        assert!(s.body.contains("跑测试"));
        assert!(inspected.tools.is_empty(), "这个插件没有工具");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// 对象式写法（`export default { skills: [...] }`）同样要收下。
    #[tokio::test]
    async fn test_sandbox_inspect_collects_object_form_skills() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_skill_obj_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("obj_skill_plugin.ts");

        std::fs::write(
            &plugin_file,
            "export default { skills: [ { name: 'style', description: '风格', body: '一律用 tabs' } ] }\n",
        )
        .unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &plugin_file,
            &temp_dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("inspect 应成功");

        assert_eq!(inspected.skills.len(), 1);
        assert_eq!(inspected.skills[0].name, "style");
        assert_eq!(inspected.skills[0].body, "一律用 tabs");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_sandbox_call_tool_e2e() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_call_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("calc_plugin.ts");

        let plugin_code = r#"
            import { join } from 'node:path';
            const fs = require('node:fs/promises');

            export default {
                name: "calc-plugin",
                description: "计算插件",
                tools: [
                    (ws: string) => ({
                        name: "write_sum",
                        description: "计算两数之和并写入工作区文件",
                        parameters: { type: "object" },
                        execute: async (callId: string, args: { a: number, b: number, filename: string }) => {
                            const sum = args.a + args.b;
                            await fs.writeFile(args.filename, `Sum is ${sum}`);
                            return {
                                ok: true,
                                output: `已计算并保存: ${sum}`,
                                details: { sum: sum }
                            };
                        }
                    })
                ]
            };
        "#;

        std::fs::write(&plugin_file, plugin_code).unwrap();

        let args = serde_json::json!({
            "a": 18,
            "b": 24,
            "filename": "calc_out.txt"
        });

        let result = PluginSandbox::call_tool(
            &plugin_file,
            "write_sum",
            args,
            &temp_dir,
            10,
        )
        .await
        .expect("call_tool 应该成功");

        assert!(result.ok, "工具执行应为 ok: true");
        assert!(result.output.contains("42"), "输出应当包含 42: {}", result.output);
        assert_eq!(result.details.unwrap()["sum"], 42);

        // 验证文件是否实际写入沙箱工作区
        let written_file = temp_dir.join("calc_out.txt");
        assert!(written_file.exists(), "工作区内文件应被成功创建");
        let content = std::fs::read_to_string(written_file).unwrap();
        assert_eq!(content, "Sum is 42");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// P4·SP-73：插件抛错时，报错要**指得回文件**（带栈与位置）。
    ///
    /// 🔴 这里刻意**不**断言行号等于源文件的行号：那需要 source map（转译后的 JS 与
    /// 原 TS 的行并非一一对应，oxc 的 codegen 会重排），属 SP-73 未做的另一半，见
    /// `docs/unfinished-features.md`。今天能保证的是"文件 + 位置 + 栈"都在，
    /// 而且**不再被压成一句没有出处的消息**（那才是作者最抓瞎的形态）。
    #[tokio::test]
    async fn test_runtime_error_points_at_the_plugin_file() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_line_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("boom.ts");
        let src = "export default {\n\n  tools: [ { name: 'boom', description: 'x', \
                   execute: async () => undefinedName() } ]\n};\n";
        std::fs::write(&plugin_file, src).unwrap();

        let res = PluginSandbox::call_tool(
            &plugin_file,
            "boom",
            serde_json::json!({}),
            &temp_dir,
            10,
        )
        .await
        .expect("调用不该直接失败");

        assert!(!res.ok, "未定义的名字必须报错：{res:?}");
        assert!(
            res.output.contains("boom.ts:"),
            "要带文件:行:列 的位置形式（不再是一句没有出处的消息）：{}",
            res.output
        );
        assert!(
            res.output.contains("at execute"),
            "要带栈（哪一层炸的）：{}",
            res.output
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_sandbox_tool_not_found() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_err_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("empty_plugin.ts");

        let plugin_code = r#"
            export default {
                name: "empty",
                tools: []
            };
        "#;

        std::fs::write(&plugin_file, plugin_code).unwrap();

        let res = PluginSandbox::call_tool(
            &plugin_file,
            "non_exist_tool",
            serde_json::json!({}),
            &temp_dir,
            10,
        )
        .await
        .expect("不应直接崩溃，而应返回错误结果");

        assert!(!res.ok, "不存在的工具应该返回 ok: false");
        assert!(res.output.contains("未找到工具"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_sandbox_web_search_pattern() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_search_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("web_search_mock.ts");

        let plugin_code = r#"
            let CONFIGURED_PROXY = '';
            export function parseResults(html: string) {
                return [{ title: "Found: " + html }];
            }

            export default function (api: any) {
                api.registerTool({
                    name: 'web_search_mock',
                    label: 'Mock Search',
                    description: 'Mocking search',
                    parameters: {
                        type: 'object',
                        properties: {
                            query: { type: 'string' },
                            proxy: { type: 'string' }
                        }
                    },
                    async execute(_callId: string, args: any, signal: any) {
                        const timeout = AbortSignal.timeout(5000);
                        const composite = signal ? AbortSignal.any([signal, timeout]) : timeout;
                        const proxy = args?.proxy || CONFIGURED_PROXY || 'direct';
                        const items = parseResults(args.query);
                        return {
                            ok: true,
                            output: `${items[0].title} via ${proxy}`,
                            details: { count: items.length, proxy }
                        };
                    }
                });

                api.registerTool({
                    name: 'configure_web_search_proxy',
                    label: 'Configure Proxy',
                    description: 'Configure proxy',
                    parameters: { type: 'object' },
                    async execute(_callId: string, args: any) {
                        if (args?.proxy !== undefined) {
                            CONFIGURED_PROXY = args.proxy;
                            return { ok: true, output: `Set proxy: ${args.proxy}` };
                        }
                        return { ok: true, output: `Current proxy: ${CONFIGURED_PROXY || 'none'}` };
                    }
                });
            }
        "#;

        std::fs::write(&plugin_file, plugin_code).unwrap();

        let tools = PluginSandbox::inspect_plugin(&plugin_file, &temp_dir).await.expect("inspect 应成功");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "web_search_mock");
        assert_eq!(tools[1].name, "configure_web_search_proxy");

        // 验证直连执行
        let res = PluginSandbox::call_tool(
            &plugin_file,
            "web_search_mock",
            serde_json::json!({ "query": "Rust Lang" }),
            &temp_dir,
            10,
        )
        .await
        .expect("call_tool 应成功");

        assert!(res.ok, "执行应成功，当前输出: {}", res.output);
        assert_eq!(res.output, "Found: Rust Lang via direct");

        // 验证动态设置代理并生效
        let res_proxy = PluginSandbox::call_tool(
            &plugin_file,
            "web_search_mock",
            serde_json::json!({ "query": "Rust Lang", "proxy": "http://127.0.0.1:7890" }),
            &temp_dir,
            10,
        )
        .await
        .expect("call_tool 带代理应成功");
        assert!(res_proxy.ok);
        assert_eq!(res_proxy.output, "Found: Rust Lang via http://127.0.0.1:7890");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
    /// P3a·SP-50：钩子调用往返（沙箱侧）。钉住三个**真实踩过**的坑：
    ///
    /// 1. 入参：`call_async_fn` 已经把 JSON 串解析成 JS 对象，插件里再 `JSON.parse`
    ///    会得到 "expected value at line 1 column 2"；
    /// 2. 调度：`const payload = payload || {}` 会在 TDZ 里遮蔽同名参数，Boa 报
    ///    "access of uninitialized binding"；
    /// 3. **工具调用路径的 api 必须也带 `registerHook`**：插件工厂调了它就抛，
    ///    于是"带钩子的插件连工具都调不动"——这条用例的后半段专门守着它。
    #[tokio::test]
    async fn test_sandbox_call_hook_round_trip_and_tool_still_works() {
        let dir = std::env::temp_dir().join(format!("a_da_hook_rt_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guard.ts");
        std::fs::write(
            &path,
            "export default function (api: any) {\n\
               api.registerHook({\n\
                 point: 'beforeToolCall',\n\
                 actions: ['deny'],\n\
                 handler: async (payload: any, ctx: any) => {\n\
                   if (String((payload.args && payload.args.cmd) || '').includes('rm -rf')) {\n\
                     return { action: 'deny', reason: '会删掉整个工作区' };\n\
                   }\n\
                   return { note: String(ctx.point) + ':' + String(ctx.workspace || '') };\n\
                 },\n\
               });\n\
               api.registerTool({ name: 'git_status', description: '插件版 git 状态', \
                 execute: async () => ({ ok: true, output: '插件真的跑了' }) });\n\
             }\n",
        )
        .unwrap();

        // ① 拦下：拿到结构化动作与理由
        let deny = PluginSandbox::call_hook_budgeted(
            &path,
            "beforeToolCall",
            serde_json::json!({ "tool": "git_status", "args": { "cmd": "rm -rf /" } }),
            &dir,
            10,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("钩子调用不应直接失败");
        assert_eq!(deny.get("action").and_then(|v| v.as_str()), Some("deny"), "{deny}");
        assert_eq!(
            deny.get("reason").and_then(|v| v.as_str()),
            Some("会删掉整个工作区")
        );

        // ② 无意见：返回空对象（等价于 Pass），不是错误
        let pass = PluginSandbox::call_hook_budgeted(
            &path,
            "beforeToolCall",
            serde_json::json!({ "tool": "git_status", "args": { "cmd": "git status" } }),
            &dir,
            10,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("钩子调用不应直接失败");
        assert!(pass.get("action").is_none(), "无意见不该编一个动作出来：{pass}");

        // ③ 同一个插件的**工具**照样能调（少了 `registerHook` 时这一步会红）
        assert_eq!(
            pass.get("note").and_then(|v| v.as_str()),
            Some(format!("beforeToolCall:{}", dir.to_string_lossy()).as_str()),
            "pi-style ctx should be injected as the second handler argument: {pass}"
        );

        let tool = PluginSandbox::call_tool(
            &path,
            "git_status",
            serde_json::json!({}),
            &dir,
            10,
        )
        .await
        .expect("工具调用不应直接失败");
        assert!(tool.ok, "带钩子的插件，工具必须照常可调：{}", tool.output);
        assert_eq!(tool.output, "插件真的跑了");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
