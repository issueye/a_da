use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ts_engine::{compiler::transpile_ts_module, PureTsRuntime};

use super::types::PluginToolDeclaration;
use crate::tools::ToolResult;

pub struct PluginSandbox;

impl PluginSandbox {
    /// 读取并转译插件的 TypeScript/JavaScript 源码为沙箱闭包代码
    async fn load_and_transpile(plugin_path: &Path) -> Result<String> {
        let source = tokio::fs::read_to_string(plugin_path)
            .await
            .map_err(|e| anyhow::anyhow!("读取插件文件失败 ({}): {}", plugin_path.display(), e))?;
        let js_code = transpile_ts_module(&source, plugin_path.to_str())
            .map_err(|e| anyhow::anyhow!("转译插件 TypeScript 失败 ({}): {}", plugin_path.display(), e))?;
        Ok(js_code)
    }

    /// 在纯 Rust 进程内微内核沙箱中检查插件暴露的工具列表
    pub async fn inspect_plugin(plugin_path: &Path, workspace: &Path) -> Result<Vec<PluginToolDeclaration>> {
        let js_code = Self::load_and_transpile(plugin_path).await?;
        let runtime = PureTsRuntime::with_workspace(Some(workspace.to_path_buf()));

        let plugin_file_json = serde_json::to_string(&plugin_path.to_string_lossy())?;
        let ws_json = serde_json::to_string(&workspace.to_string_lossy())?;

        // 1. 装载插件模块定义
        let init_script = format!(
            r#"
            (function() {{
                const module = {{ exports: {{}} }};
                const exports = module.exports;
                const __filename = {plugin_file};
                const __dirname = path.dirname(__filename);

                {js_code}

                globalThis.__ada_current_plugin = module.exports.default || module.exports;
            }})();
            "#,
            plugin_file = plugin_file_json,
            js_code = js_code
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
                    if (typeof desc === 'function') {{
                        const api = {{
                            trace: (msg) => console.log(msg),
                            workspace: ws,
                            registerTool: (tool) => {{ toolsRaw.push(tool); }},
                            registerSkill: () => {{}},
                            registerPrompt: () => {{}},
                        }};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, error: "插件执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object') {{
                        const raw = desc.tools || [];
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
                    return JSON.stringify({{ ok: true, tools: tools }});
                }} catch (err) {{
                    return JSON.stringify({{ ok: false, error: String(err && err.message ? err.message : err) }});
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
            Ok(tools)
        } else {
            let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("检查插件失败");
            anyhow::bail!("{}", err);
        }
    }

    /// 在纯 Rust 进程内微内核沙箱中执行插件工具
    pub async fn call_tool(
        plugin_path: &Path,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
    ) -> Result<ToolResult> {
        let js_code = Self::load_and_transpile(plugin_path).await?;
        let runtime = PureTsRuntime::with_workspace(Some(workspace.to_path_buf()));

        let plugin_file_json = serde_json::to_string(&plugin_path.to_string_lossy())?;
        let ws_json = serde_json::to_string(&workspace.to_string_lossy())?;
        let tool_name_json = serde_json::to_string(tool_name)?;

        // 1. 装载插件模块定义
        let init_script = format!(
            r#"
            (function() {{
                const module = {{ exports: {{}} }};
                const exports = module.exports;
                const __filename = {plugin_file};
                const __dirname = path.dirname(__filename);

                {js_code}

                globalThis.__ada_current_plugin = module.exports.default || module.exports;
            }})();
            "#,
            plugin_file = plugin_file_json,
            js_code = js_code
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
                            registerSkill: () => {{}},
                            registerPrompt: () => {{}},
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
                    return JSON.stringify({{ ok: false, output: String(err && err.message ? err.message : err) }});
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

        let res: ToolResult = serde_json::from_value(resp)
            .unwrap_or_else(|e| ToolResult::error(format!("反序列化工具输出失败: {e}")));

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
}
