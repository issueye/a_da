use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ada_ts_engine::{compiler::transpile_ts_module_cached, PureTsRuntime};

use super::types::{
    PluginActionDeclaration, PluginHookDeclaration, PluginPromptDeclaration,
    PluginRouteDeclaration, PluginSkillDeclaration, PluginToolDeclaration,
};
use agent_toolkit::ToolResult;

/// TS 侧动态路由运行时：注册归一化 + 匹配 + HTTP 响应归一化。
///
/// 这段 JS 在**检查期与调用期各注入一次**（两处必须同一套匹配规则，否则
/// “检查时说有、调用时说没有”这种错最难查）。宿主只做请求转发与响应落盘，
/// 新增一条接口不需要再改 Rust。
const ROUTE_RUNTIME_JS: &str = r#"
globalThis.__ada_route_runtime = (function () {
  var METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];

  function normalize(route, order) {
    if (!route || typeof route !== 'object') throw new Error('route 需要对象');
    var method = typeof route.method === 'string' ? route.method.trim().toUpperCase() : 'GET';
    if (method !== '*' && METHODS.indexOf(method) < 0) throw new Error('不支持的路由方法: ' + method);
    var path = typeof route.path === 'string' ? route.path.trim() : '';
    if (!path) path = '/';
    if (path.length > 1024) throw new Error('路由路径过长（上限 1024）');
    if (path.indexOf('?') >= 0 || path.indexOf('#') >= 0) throw new Error('路由路径不能包含查询串或片段');
    if (path.charAt(0) !== '/') path = '/' + path;
    while (path.length > 1 && path.charAt(path.length - 1) === '/') path = path.slice(0, -1);
    var parts = path.split('/').filter(function (part) { return part.length > 0; });
    for (var i = 0; i < parts.length; i++) {
      if (parts[i] === '.' || parts[i] === '..') throw new Error('路由路径不能包含 . 或 .. 段');
    }
    if (typeof route.handler !== 'function') throw new Error('路由 ' + method + ' ' + path + ' 缺少 handler 函数');
    var name = typeof route.name === 'string' && route.name.trim() ? route.name.trim() : (method + ' ' + path);
    return { name: name, method: method, path: path, handler: route.handler, order: order };
  }

  function compile(route) {
    var segments = [];
    var score = 0;
    var parts = route.path.split('/').filter(function (part) { return part.length > 0; });
    for (var i = 0; i < parts.length; i++) {
      var part = parts[i];
      if (part.charAt(0) === ':') {
        if (part.length < 2) throw new Error('路由参数段缺少名字: ' + route.path);
        segments.push({ kind: 'param', name: part.slice(1) });
        score += 1;
      } else if (part.charAt(0) === '*') {
        if (part.length < 2) throw new Error('路由通配段缺少名字: ' + route.path);
        if (i !== parts.length - 1) throw new Error('路由通配段必须是最后一段: ' + route.path);
        segments.push({ kind: 'wild', name: part.slice(1) });
      } else {
        var value = part;
        try { value = decodeURIComponent(part); } catch (_) {}
        segments.push({ kind: 'static', value: value });
        score += 3;
      }
    }
    route.segments = segments;
    route.score = score;
    return route;
  }

  function compileAll(raw) {
    var routes = [];
    var seen = {};
    for (var i = 0; i < raw.length; i++) {
      var route = compile(normalize(raw[i], i));
      var key = route.method + ' ' + route.path;
      if (seen[key]) throw new Error('重复的路由: ' + key);
      seen[key] = true;
      routes.push(route);
    }
    routes.sort(function (a, b) {
      if (b.score !== a.score) return b.score - a.score;
      if (b.segments.length !== a.segments.length) return b.segments.length - a.segments.length;
      return a.order - b.order;
    });
    return routes;
  }

  function matchOne(route, method, path) {
    if (route.method !== '*' && route.method !== method) return null;
    var parts = path.split('/').filter(function (part) { return part.length > 0; });
    var params = {};
    var index = 0;
    for (var i = 0; i < route.segments.length; i++) {
      var segment = route.segments[i];
      if (segment.kind === 'wild') {
        params[segment.name] = parts.slice(index).join('/');
        index = parts.length;
        break;
      }
      if (index >= parts.length) return null;
      if (segment.kind === 'static') {
        if (parts[index] !== segment.value) return null;
      } else {
        params[segment.name] = parts[index];
      }
      index += 1;
    }
    if (index !== parts.length) return null;
    return { route: route, params: params };
  }

  function match(routes, method, path) {
    method = String(method || 'GET').toUpperCase();
    path = String(path || '/');
    if (path.charAt(0) !== '/') path = '/' + path;
    var matched = null;
    for (var i = 0; i < routes.length && !matched; i++) {
      matched = matchOne(routes[i], method, path);
    }
    // HEAD 没单独声明时按 GET 处理（HTTP 语义），响应体由宿主截掉。
    if (!matched && method === 'HEAD') {
      for (var j = 0; j < routes.length && !matched; j++) {
        matched = matchOne(routes[j], 'GET', path);
      }
    }
    return matched;
  }

  function response(options) {
    if (!options || typeof options !== 'object') throw new Error('response 需要对象');
    return {
      __ada_http: true,
      status: options.status,
      headers: options.headers,
      body: options.body,
      bodyKind: options.bodyKind,
      contentType: options.contentType,
    };
  }

  function finalize(value) {
    if (value && value.__ada_http === true) {
      var status = Number.isInteger(value.status) ? value.status : 200;
      if (status < 200 || status > 599) throw new Error('HTTP 状态码超出范围: ' + status);
      var headers = value.headers && typeof value.headers === 'object' ? value.headers : {};
      var kind = value.bodyKind === 'base64' ? 'base64' : 'text';
      var body = value.body === undefined || value.body === null ? '' : value.body;
      if (kind === 'base64') {
        if (typeof body !== 'string') throw new Error('base64 响应体必须是字符串');
      } else if (typeof body !== 'string') {
        body = JSON.stringify(body);
      }
      return {
        status: status,
        headers: headers,
        body: body,
        bodyKind: kind,
        contentType: typeof value.contentType === 'string' ? value.contentType : undefined,
      };
    }
    return {
      status: 200,
      headers: {},
      body: JSON.stringify(value === undefined ? null : value),
      bodyKind: 'text',
      contentType: 'application/json; charset=utf-8',
    };
  }

  function json(data, status, headers) {
    return response({
      status: status === undefined ? 200 : status,
      headers: headers || {},
      body: JSON.stringify(data === undefined ? null : data),
      bodyKind: 'text',
      contentType: 'application/json; charset=utf-8',
    });
  }

  function text(body, status, headers) {
    return response({
      status: status === undefined ? 200 : status,
      headers: headers || {},
      body: body === undefined || body === null ? '' : String(body),
      bodyKind: 'text',
      contentType: 'text/plain; charset=utf-8',
    });
  }

  function html(body, status, headers) {
    return response({
      status: status === undefined ? 200 : status,
      headers: headers || {},
      body: body === undefined || body === null ? '' : String(body),
      bodyKind: 'text',
      contentType: 'text/html; charset=utf-8',
    });
  }

  function redirect(location, status) {
    return response({
      status: status === undefined ? 303 : status,
      headers: { Location: String(location === undefined ? '/' : location) },
      body: '',
      bodyKind: 'text',
      contentType: 'text/plain; charset=utf-8',
    });
  }

  return {
    normalize: normalize,
    compileAll: compileAll,
    match: match,
    response: response,
    finalize: finalize,
    json: json,
    text: text,
    html: html,
    redirect: redirect,
  };
})();
"#;

/// 一次插件检查的完整结果（工具 + 提示词片段 + 技能 + 钩子声明）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginInspection {
    pub metadata: Option<serde_json::Value>,
    pub tools: Vec<PluginToolDeclaration>,
    pub prompts: Vec<PluginPromptDeclaration>,
    /// **恒为空**（2026-10-19 起技能改为 `skills/<name>/SKILL.md` 目录约定，不再由脚本声明）。
    ///
    /// 保留字段是为了兼容既有反序列化路径与测试；新代码不要往里填。
    pub skills: Vec<PluginSkillDeclaration>,
    /// 被忽略的 `registerSkill(...)` 调用（技能名）。
    ///
    /// 旧 API 已移除：这里如实记下来，装配层据此报 `skill_spec_violation:*`——
    /// 静默忽略会让作者以为"技能声明成功了"，而界面上根本没有它。
    pub ignored_skills: Vec<String>,
    /// P3a·SP-50：插件声明的钩子注册（同上）
    pub hooks: Vec<PluginHookDeclaration>,
    /// 面向界面的确定性动作（`ada.registerAction`）。空数组 = 没声明。
    pub actions: Vec<PluginActionDeclaration>,
    /// 动态 HTTP 路由（`ada.route`）。空数组 = 没声明。
    pub routes: Vec<PluginRouteDeclaration>,
}

pub struct PluginSandbox;

/// `ada` API 的表面能力开关。
///
/// 🔴 **为什么需要这个东西**：插件工厂会被**五个入口**重新执行一遍（检查 / 工具 /
/// 动作 / 路由 / 钩子）。过去五处各写一份 `api` 字面量，于是"某处少给了一个入口"
/// 就成了一批**只在特定场景才炸**的 bug：插件在工厂里调了 `ada.route(...)`，
/// 检查期过、动态路由期也过，一点界面按钮就报
/// `插件初始化执行异常: ... is not a function`
/// （本轮实测：`plugins/unified-demo` 的动作就是这样挂的）。
///
/// 现在只有这一份表面：每个入口只打开自己需要的收集器，其余入口给 **no-op**
/// —— 存在但不生效。插件工厂不会因为"某个入口不存在"而抛错，而"到底声明了什么"
/// 仍然只有检查期那一份是真相来源。
#[derive(Clone, Copy, Default)]
struct ApiSurface {
    tools: bool,
    actions: bool,
    routes: bool,
    hooks: bool,
    prompts: bool,
    /// 技能已改为目录约定（`skills/<name>/SKILL.md`）：这里只把调用记成诊断。
    skill_diagnostics: bool,
    /// 是否把插件的 `metadata` 透出去（只有检查期需要）。
    metadata: bool,
}

impl ApiSurface {
    /// 检查期：**全收集**——它是"这个插件到底声明了什么"的唯一真相来源。
    const INSPECT: Self = Self {
        tools: true,
        actions: true,
        routes: true,
        hooks: true,
        prompts: true,
        skill_diagnostics: true,
        metadata: true,
    };
}

/// 一行收集器：开着的往数组里推，关着的给 no-op。
fn collector_line(enabled: bool, name: &str, push: &str) -> String {
    if enabled {
        format!("                {name}: (item) => {{ {push}; }},\n")
    } else {
        format!("                {name}: () => {{}},\n")
    }
}

/// 钩子收集器（`ada.on`）。
///
/// 单独抽出来是因为它比一行 no-op 复杂得多（事件别名 + 动作声明），而检查期与
/// 钩子调用期**必须**用同一套别名表——两边不一致的话，会出现"检查说有这个点位、
/// 调的时候找不到"。
const HOOK_ON_JS: &str = r#"                on: (event, handler, options = {}) => {
                    const aliases = {
                        turn_start: 'beforeTurn', turn_end: 'afterTurn',
                        tool_call: 'beforeToolCall', tool_execution_start: 'beforeToolCall',
                        tool_execution_end: 'afterToolCall', tool_result: 'afterToolCall',
                        agent_start: 'beforeAgentStart', agent_end: 'afterAgentEnd',
                        before_provider_request: 'beforeProviderRequest',
                        before_provider_headers: 'beforeProviderHeaders',
                        after_provider_response: 'afterProviderResponse',
                        context: 'beforeContext',
                    };
                    const point = aliases[event] || event;
                    // 写错就报错：静默跳过会让作者一直以为钩子装上了。
                    if (typeof handler !== 'function') throw new Error('ada.on requires a handler');
                    const inferredActions = point === 'beforeToolCall'
                        ? ['observe', 'note', 'deny', 'replace']
                        : ['observe', 'note'];
                    hooksRaw.push({
                        point,
                        apiEvent: event,
                        handler,
                        actions: Array.isArray(options.actions) ? options.actions : inferredActions,
                        order: typeof options.order === 'number' ? options.order : 0,
                    });
                },
"#;

impl PluginSandbox {
    /// 生成 `ada` API 字面量（五个入口共用，见 [`ApiSurface`]）。
    ///
    /// 🔴 调用方必须保证两件事，否则生成的 API 会在执行期炸：
    /// 1. 作用域里有 `ws`；
    /// 2. **已注入** [`ROUTE_RUNTIME_JS`]（`response` / `json` / `text` / `html` /
    ///    `redirect` 都从 `globalThis.__ada_route_runtime` 取；没注入时取属性就会抛
    ///    TypeError，那正是这次要修的那类错）。
    fn api_literal(surface: ApiSurface) -> String {
        let mut out = String::from("{\n");
        out.push_str("                trace: (msg) => console.log(msg),\n");
        out.push_str("                workspace: ws,\n");
        out.push_str(if surface.metadata {
            "                metadata: metadata || undefined,\n"
        } else {
            "                metadata: undefined,\n"
        });
        out.push_str(&collector_line(surface.tools, "registerTool", "toolsRaw.push(item)"));
        out.push_str(&collector_line(
            surface.actions,
            "registerAction",
            "actionsRaw.push(item)",
        ));
        out.push_str(&collector_line(surface.routes, "route", "routesRaw.push(item)"));
        out.push_str(&collector_line(
            surface.routes,
            "registerRoute",
            "routesRaw.push(item)",
        ));
        for helper in ["response", "json", "text", "html", "redirect"] {
            out.push_str(&format!(
                "                {helper}: globalThis.__ada_route_runtime.{helper},\n"
            ));
        }
        out.push_str(&collector_line(
            surface.prompts,
            "registerSystemPrompt",
            "promptsRaw.push(item)",
        ));
        out.push_str(&collector_line(
            surface.prompts,
            "registerPrompt",
            "promptsRaw.push(item)",
        ));
        out.push_str(if surface.skill_diagnostics {
            "                registerSkill: (skill) => { ignoredSkills.push(skill && skill.name ? String(skill.name) : '<unnamed>'); },\n"
        } else {
            "                registerSkill: () => {},\n"
        });
        out.push_str(if surface.hooks {
            HOOK_ON_JS
        } else {
            "                on: () => {},\n"
        });
        out.push_str("            }");
        out
    }
}


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
        let require_factory = plugin_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "index.ts" | "index.js"));

        // 1. 装载插件模块定义
        // 🔴 **前导脚本压成一行**（P4·SP-73）：插件代码必须从第 1 行开始，
        // 否则报错里的行号会整体偏移（作者照着改会改错地方）。
        // 同行拼接是安全的：插件首行若是行注释，被吃掉的只是那一行注释自身；
        // 若是块注释，它自带收尾。
        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_module_metadata = module.exports.metadata || (module.exports.default && module.exports.default.metadata) || null;\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;\n}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );

        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            anyhow::bail!("插件语法或装载错误: {e}");
        }
        if let Err(e) = runtime.eval_ts(ROUTE_RUNTIME_JS, Some("ada_route_runtime.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化动态路由运行时失败: {e}");
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
                    const requireFactory = {require_factory};
                    let metadata = globalThis.__ada_module_metadata || (desc && (typeof desc === 'object' || typeof desc === 'function') ? desc.metadata : undefined);
                    let toolsRaw = [];
                    let promptsRaw = [];
                    let ignoredSkills = [];
                    let hooksRaw = [];
                    let actionsRaw = [];
                    let routesRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {api};
                        try {{
                            await desc(api);
                            metadata = api.metadata || metadata;
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, error: "插件执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object' && !requireFactory) {{
                        const raw = desc.tools || [];
                        const rawPrompts = desc.systemPrompts || desc.prompts || [];
                        const rawSkills = desc.skills || [];
                        const rawHooks = desc.hooks || [];
                        for (let i = 0; i < rawPrompts.length; i++) promptsRaw.push(rawPrompts[i]);
                        // 对象形态的 `skills` 数组也是旧 API：记下来报诊断，不再收
                        for (let i = 0; i < rawSkills.length; i++) {{
                            ignoredSkills.push(rawSkills[i] && rawSkills[i].name ? String(rawSkills[i].name) : '<unnamed>');
                        }}
                        for (let i = 0; i < rawHooks.length; i++) hooksRaw.push(rawHooks[i]);
                        for (let i = 0; i < raw.length; i++) {{
                            const item = raw[i];
                            const tool = typeof item === 'function' ? await item(ws || process.cwd()) : item;
                            if (tool) toolsRaw.push(tool);
                        }}
                    }} else {{
                        return JSON.stringify({{ ok: false, error: "Extension Factory must be a default-exported function" }});
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
                    // 2026-10-19：技能改为目录约定（`skills/<name>/SKILL.md`），
                    // 沙箱**不再产出技能声明**——目录扫描在装配层做（`skills::spec`）。
                    const skills = [];
                    // P3a·SP-50：钩子注册的**声明**（处理器是函数，不进 JSON——
                    // 它在调用期按点位名重新找出来执行）。
                    const hooks = [];
                    const hooksByPoint = new Map();
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
                        const order = typeof h.order === 'number' ? h.order : 0;
                        const existing = hooksByPoint.get(point);
                        if (existing) {{
                            for (const action of actions) if (!existing.actions.includes(action)) existing.actions.push(action);
                            existing.order = Math.min(existing.order, order);
                        }} else {{
                            const declaration = {{ point, actions, order }};
                            hooksByPoint.set(point, declaration);
                            hooks.push(declaration);
                        }}
                    }}
                    const actions = [];
                    const actionNames = new Set();
                    for (let i = 0; i < actionsRaw.length; i++) {{
                        const inst = actionsRaw[i];
                        if (!inst || typeof inst !== 'object') continue;
                        const name = typeof inst.name === 'string' ? inst.name.trim() : '';
                        if (!name) {{
                            return JSON.stringify({{ ok: false, error: "registerAction requires a non-empty name" }});
                        }}
                        if (actionNames.has(name)) {{
                            return JSON.stringify({{ ok: false, error: "重复的动作名: " + name }});
                        }}
                        actionNames.add(name);
                        actions.push({{
                            name,
                            label: typeof inst.label === 'string' ? inst.label : undefined,
                            description: typeof inst.description === 'string' ? inst.description : '',
                            mode: inst.mode === 'read' ? 'read' : 'write',
                            parameters: inst.parameters || {{ type: 'object', properties: {{}} }},
                        }});
                    }}
                    const compiledRoutes = globalThis.__ada_route_runtime.compileAll(routesRaw);
                    // 检查结果的顺序按**声明顺序**（metadata.routes 的 names 也是这个口径）；
                    // 匹配顺序由运行时自己按静态/参数/通配排序，不受这里影响。
                    const declaredRoutes = compiledRoutes.slice().sort((a, b) => a.order - b.order);
                    const routes = [];
                    for (let i = 0; i < declaredRoutes.length; i++) {{
                        routes.push({{
                            name: declaredRoutes[i].name,
                            method: declaredRoutes[i].method,
                            path: declaredRoutes[i].path,
                        }});
                    }}
                    return JSON.stringify({{ ok: true, metadata: metadata || null, tools: tools, prompts: prompts, skills: skills, ignoredSkills: ignoredSkills, hooks: hooks, actions: actions, routes: routes }});
                }} catch (err) {{
                    // 🔴 带上 `stack`（P4·SP-73）：只拿 `err.message` 的话，报错不带文件与行号。
                    // 反过来只取 `stack` 又会丢掉 Boa 的消息行（只剩帧），所以两者都留。
                    const detail = err && err.message
                        ? String(err.message) + (err.stack ? "\n" + String(err.stack) : "")
                        : (err && err.stack ? String(err.stack) : String(err));
                    return JSON.stringify({{ ok: false, error: detail }});
                }}
            }};
            "#,
            api = Self::api_literal(ApiSurface::INSPECT),
            ws_json = ws_json,
            require_factory = require_factory
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
            // 旧的 `registerSkill` 调用：记下来，交给装配层报诊断
            let ignored_skills: Vec<String> = resp
                .get("ignoredSkills")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            // P3a·SP-50：钩子注册同理
            let hooks: Vec<PluginHookDeclaration> = serde_json::from_value(
                resp.get("hooks").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            // 面向界面的动作声明（空数组 = 没声明）
            let actions: Vec<PluginActionDeclaration> = serde_json::from_value(
                resp.get("actions").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            // 动态 HTTP 路由声明（空数组 = 没声明）
            let routes: Vec<PluginRouteDeclaration> = serde_json::from_value(
                resp.get("routes").cloned().unwrap_or(serde_json::json!([])),
            )
            .unwrap_or_default();
            let metadata = resp.get("metadata").cloned().filter(|value| !value.is_null());
            if plugin_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "index.ts" | "index.js"))
            {
                let metadata = metadata
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("插件包入口缺少 metadata 对象"))?;
                Self::validate_package_metadata(
                    metadata,
                    &tools,
                    &prompts,
                    &skills,
                    &hooks,
                    &actions,
                    &routes,
                )?;
            }
            Ok(PluginInspection {
                metadata,
                tools,
                prompts,
                skills,
                ignored_skills,
                hooks,
                actions,
                routes,
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
    fn validate_package_metadata(
        metadata: &serde_json::Value,
        tools: &[PluginToolDeclaration],
        prompts: &[PluginPromptDeclaration],
        _skills: &[PluginSkillDeclaration],
        hooks: &[PluginHookDeclaration],
        actions: &[PluginActionDeclaration],
        routes: &[PluginRouteDeclaration],
    ) -> Result<()> {
        let object = metadata
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("插件 metadata 必须是对象"))?;
        for key in ["name", "description"] {
            if object
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                anyhow::bail!("插件 metadata 缺少非空 `{key}`");
            }
        }
        Self::validate_metadata_group(metadata, "tools", tools.len(), tools.iter().map(|v| v.name.as_str()))?;
        Self::validate_metadata_group(metadata, "systemPrompt", prompts.len(), prompts.iter().map(|v| v.id.as_str()))?;
        // `skills` **不再参与一致性校验**（2026-10-19）：技能是磁盘约定
        // `skills/<name>/SKILL.md`，而这里跑在沙箱里、看不到目录——拿"脚本声明的技能数"
        // （现在恒为 0）去比，只会把每个带技能的插件判成 broken。
        // 真正的对账在装配层：目录扫描结果 + `skill_spec_violation:*` 诊断。
        if let Some(group) = metadata.get("skills") {
            if !group.is_object() {
                anyhow::bail!("插件 metadata `skills` 必须是对象（可省略）");
            }
        }
        Self::validate_metadata_group(metadata, "hooks", hooks.len(), hooks.iter().map(|v| v.point.as_str()))?;
        // 动作组是**可选**的：没有动作的旧插件不必补字段；一旦注册了动作就必须对账，
        // 与 tools / systemPrompt / hooks 同一条失败安全口径。
        if !actions.is_empty() {
            Self::validate_metadata_group(
                metadata,
                "actions",
                actions.len(),
                actions.iter().map(|v| v.name.as_str()),
            )?;
        } else if let Some(group) = metadata.get("actions") {
            if !group.is_object() {
                anyhow::bail!("插件 metadata `actions` 必须是对象（可省略）");
            }
            Self::validate_metadata_group(metadata, "actions", 0, std::iter::empty())?;
        }
        // 路由组同样是可选的：动态路由本来就可以只写在 `ada.route(...)` 里。
        // 一旦 metadata 写了 `routes`，count / names 仍要逐项对账。
        if routes.is_empty() {
            if let Some(group) = metadata.get("routes") {
                if !group.is_object() {
                    anyhow::bail!("插件 metadata `routes` 必须是对象（可省略）");
                }
                Self::validate_metadata_group(metadata, "routes", 0, std::iter::empty())?;
            }
        } else if metadata.get("routes").is_some() {
            Self::validate_metadata_group(
                metadata,
                "routes",
                routes.len(),
                routes.iter().map(|v| v.name.as_str()),
            )?;
        }
        Ok(())
    }

    fn validate_metadata_group<'a>(
        metadata: &serde_json::Value,
        key: &str,
        actual_count: usize,
        actual_names: impl Iterator<Item = &'a str>,
    ) -> Result<()> {
        let group = metadata
            .get(key)
            .and_then(|value| value.as_object())
            .ok_or_else(|| anyhow::anyhow!("插件 metadata 缺少 `{key}` 对象"))?;
        let declared_count = group
            .get("count")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| anyhow::anyhow!("插件 metadata `{key}.count` 必须是整数"))? as usize;
        if declared_count != actual_count {
            anyhow::bail!("插件 metadata `{key}.count`={declared_count}，实际为 {actual_count}");
        }
        let names = group
            .get("names")
            .or_else(|| group.get("items"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow::anyhow!("插件 metadata `{key}.names` 必须是字符串数组"))?;
        let expected: Vec<&str> = actual_names.collect();
        let declared: Vec<&str> = names.iter().filter_map(|value| value.as_str()).collect();
        if declared.len() != names.len() || declared != expected {
            anyhow::bail!("插件 metadata `{key}.names` 与实际声明不一致");
        }
        Ok(())
    }

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
        let require_factory = plugin_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "index.ts" | "index.js"));

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
        // 路由助手（`ada.text` / `json` / `html` / `redirect`）都从这份运行时取。
        // 不注入的话，插件在工厂里调 `ada.text(...)` 会撞 `undefined.xxx`——
        // 这正是"动作/工具/钩子路径少给入口"那类错的另一半。
        if let Err(e) = runtime.eval_ts(ROUTE_RUNTIME_JS, Some("ada_route_runtime.js")).await {
            runtime.terminate();
            return Ok(serde_json::json!({ "error": format!("初始化动态路由运行时失败: {e}") }));
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
                    const requireFactory = {require_factory};
                    const eventPayload = payload && payload.__ada_hook_event !== undefined
                        ? payload.__ada_hook_event
                        : payload;
                    const hookCtx = Object.assign(
                        {{ point: targetPoint, workspace: ws }},
                        payload && payload.__ada_hook_ctx !== undefined ? payload.__ada_hook_ctx : {{}},
                    );
                    const registrations = [];
                    hookCtx.registerTool = (tool) => {{
                        if (!tool || typeof tool.name !== 'string' || typeof tool.executeExport !== 'string') {{
                            return {{ ok: false, reason: 'registerTool requires name and executeExport' }};
                        }}
                        registrations.push({{
                            kind: 'tool',
                            name: tool.name,
                            description: typeof tool.description === 'string' ? tool.description : '',
                            parameters: tool.parameters || {{ type: 'object', properties: {{}} }},
                            executeExport: tool.executeExport,
                        }});
                        return {{ ok: true }};
                    }};
                    hookCtx.registerSystemPrompt = (prompt) => {{
                        if (!prompt || typeof prompt.id !== 'string' || typeof prompt.content !== 'string') {{
                            return {{ ok: false, reason: 'registerSystemPrompt requires id and content' }};
                        }}
                        registrations.push({{ kind: 'systemPrompt', id: prompt.id, content: prompt.content }});
                        return {{ ok: true }};
                    }};
                    hookCtx.registerSkill = (skill) => {{
                        if (!skill || typeof skill.name !== 'string' || typeof skill.description !== 'string' || typeof skill.path !== 'string') {{
                            return {{ ok: false, reason: 'registerSkill requires name, description and path' }};
                        }}
                        registrations.push({{
                            kind: 'skill',
                            name: skill.name,
                            description: skill.description,
                            path: skill.path,
                            body: typeof skill.body === 'string' ? skill.body : undefined,
                            whenToUse: typeof skill.whenToUse === 'string' ? skill.whenToUse : undefined,
                            userInvocable: typeof skill.userInvocable === 'boolean' ? skill.userInvocable : undefined,
                        }});
                        return {{ ok: true }};
                    }};
                    // 钩子内**没有** registerHook：动态注册目前整条链没有消费者
                    // （`take_registrations` 只被测试调用），留着只会让作者以为注册成功。
                    // 需要新增钩子请在工厂期用 `ada.on` 声明。
                    let hooksRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {api};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ error: "插件初始化执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object' && !requireFactory) {{
                        for (const hook of (desc.hooks || [])) hooksRaw.push(hook);
                    }} else {{
                        return JSON.stringify({{ error: "Extension Factory must be a default-exported function" }});
                    }}

                    const targets = hooksRaw.filter((h) => h && h.point === targetPoint)
                        .sort((a, b) => (a.order || 0) - (b.order || 0));
                    if (targets.length === 0) {{
                        return JSON.stringify({{ error: "插件未声明点位: " + targetPoint }});
                    }}

                    // 🔴 入参**已经是对象**：`call_async_fn` 会把传进去的 JSON 串解析成 JS 值
                    // （与工具那条路一致）。再 `JSON.parse` 一次会得到
                    // "expected value at line 1 column 2" 这种跟插件毫不相干的错。
                    // 也**不要**在这里 `const payload = payload || {}`：同名的 const 会
                    // 在 TDZ 里遮蔽参数，Boa 报 "access of uninitialized binding"。
                    let finalResult = {{}};
                    const notes = [];
                    for (const target of targets) {{
                        if (typeof target.handler !== 'function') continue;
                        let event = eventPayload && typeof eventPayload === 'object'
                            ? Object.assign({{}}, eventPayload)
                            : {{ value: eventPayload }};
                        if (target.apiEvent === 'tool_call') {{
                            event = Object.assign({{}}, event, {{ toolName: event.tool, input: event.args }});
                        }}
                        const beforeInput = target.apiEvent === 'tool_call' ? JSON.stringify(event.input) : undefined;
                        // pi-compatible handlers receive (event, ctx). ctx is data-only
                        // except for the explicit, validated registration methods.
                        const res = await target.handler(event, hookCtx);
                        if (target.apiEvent === 'tool_call' && JSON.stringify(event.input) !== beforeInput) {{
                            finalResult = {{ action: 'replace', data: {{ args: event.input }} }};
                        }}
                        if (res && typeof res === 'object') {{
                            if (res.block === true && !finalResult.action) {{
                                finalResult = {{ action: res.terminate === true ? 'abort' : 'deny', reason: res.reason }};
                            }} else if (typeof res.action === 'string' && !finalResult.action) {{
                                finalResult = res;
                            }}
                            if (typeof res.note === 'string' && res.note.trim()) notes.push(res.note);
                        }} else if (typeof res === 'string' && !finalResult.action) {{
                            finalResult = {{ action: res }};
                        }}
                    }}
                    if (notes.length > 0 && finalResult.note === undefined) finalResult.note = notes.join('\\n');
                    if (Object.keys(finalResult).length > 0) {{
                        const data = finalResult.data && typeof finalResult.data === 'object' ? Object.assign({{}}, finalResult.data) : {{}};
                        if (registrations.length > 0) data.registrations = registrations;
                        if (Object.keys(data).length > 0) finalResult.data = data;
                        return JSON.stringify(finalResult);
                    }}
                    if (registrations.length > 0) return JSON.stringify({{ data: {{ registrations }} }});
                    if (notes.length > 0) return JSON.stringify({{ action: 'note', note: notes.join('\\n') }});
                    return JSON.stringify({{}});
                }} catch (err) {{
                    // 🔴 带上 `stack`（P4·SP-73）：只拿 `err.message` 的话，报错不带文件与行号。
                    const detail = err && err.stack ? String(err.stack) : String(err);
                    return JSON.stringify({{ error: detail }});
                }}
            }};
            "#,
            ws_json = ws_json,
            point_json = point_json,
            api = Self::api_literal(ApiSurface { hooks: true, ..Default::default() }),
            require_factory = require_factory
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

    /// 执行一个**面向界面**的插件动作（`ada.registerAction`）。
    ///
    /// 与工具/钩子同构（同一套沙箱与预算），但语义不同：
    /// - 不经过模型、不产生工具卡、不需要审批；
    /// - `intent=read` 时**在沙箱内**拒绝 `mode=write` 的动作——前端把写动作标成只读也绕不过；
    /// - 返回值统一成 `{ok,data}` / `{ok:false,error,kind}`，由 RPC 层映射错误码。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_action_budgeted(
        plugin_path: &Path,
        action_name: &str,
        input: serde_json::Value,
        context: serde_json::Value,
        intent: &str,
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
        let action_json = serde_json::to_string(action_name)?;
        let intent_json = serde_json::to_string(intent)?;
        let require_factory = plugin_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "index.ts" | "index.js"));

        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );
        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            anyhow::bail!("插件装载失败: {e}");
        }
        // 路由助手（`ada.text` / `json` / …）来自这份运行时；动作路径同样要能调它们，
        // 否则插件在工厂里准备响应常量就会撞 `undefined.xxx`。
        if let Err(e) = runtime.eval_ts(ROUTE_RUNTIME_JS, Some("ada_route_runtime.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化动态路由运行时失败: {e}");
        }

        let call_setup = format!(
            r#"
            globalThis.__ada_call_action = async function(payload) {{
                try {{
                    const desc = globalThis.__ada_current_plugin;
                    if (!desc) {{
                        return JSON.stringify({{ ok: false, error: "插件未导出有效对象或函数", kind: "failed" }});
                    }}
                    const ws = {ws_json};
                    const target = {action_json};
                    const intent = {intent_json};
                    const requireFactory = {require_factory};
                    let actionsRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {api};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, error: "插件初始化执行异常: " + (e && e.stack ? String(e.stack) : String(e)), kind: "failed" }});
                        }}
                    }} else {{
                        return JSON.stringify({{ ok: false, error: "Extension Factory must be a default-exported function", kind: "failed" }});
                    }}

                    let found = null;
                    for (let i = 0; i < actionsRaw.length; i++) {{
                        const action = actionsRaw[i];
                        if (action && typeof action === 'object' && action.name === target) {{
                            found = action;
                            break;
                        }}
                    }}
                    if (!found) {{
                        return JSON.stringify({{ ok: false, error: "插件中未找到动作: " + target, kind: "not_found" }});
                    }}
                    const mode = found.mode === 'read' ? 'read' : 'write';
                    if (intent === 'read' && mode !== 'read') {{
                        return JSON.stringify({{ ok: false, error: "动作 " + target + " 声明为 write，不能用 read 意图调用", kind: "denied" }});
                    }}
                    if (typeof found.execute !== 'function') {{
                        return JSON.stringify({{ ok: false, error: "动作 " + target + " 未实现 execute 方法", kind: "failed" }});
                    }}

                    let result;
                    try {{
                        result = await found.execute(payload.input || {{}}, payload.context || {{}});
                    }} catch (e) {{
                        return JSON.stringify({{ ok: false, error: e && e.stack ? String(e.stack) : String(e), kind: "failed" }});
                    }}
                    if (result && typeof result === 'object' && typeof result.ok === 'boolean') {{
                        if (!result.ok) {{
                            return JSON.stringify({{
                                ok: false,
                                error: typeof result.error === 'string' ? result.error : "动作失败",
                                hint: typeof result.hint === 'string' ? result.hint : undefined,
                                kind: "failed",
                            }});
                        }}
                        return JSON.stringify({{ ok: true, data: result.data !== undefined ? result.data : result }});
                    }}
                    return JSON.stringify({{ ok: true, data: result === undefined ? null : result }});
                }} catch (err) {{
                    const detail = err && err.stack ? String(err.stack) : String(err);
                    return JSON.stringify({{ ok: false, error: detail, kind: "failed" }});
                }}
            }};
            "#,
            ws_json = ws_json,
            action_json = action_json,
            intent_json = intent_json,
            api = Self::api_literal(ApiSurface { actions: true, ..Default::default() }),
            require_factory = require_factory
        );
        if let Err(e) = runtime.eval_ts(call_setup, Some("ada_action_caller.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化动作调度器失败: {e}");
        }

        let payload = serde_json::json!({ "input": input, "context": context });
        let call_fut = runtime.call_async_fn(
            "__ada_call_action",
            serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        );
        let resp_str = match tokio::time::timeout(Duration::from_secs(timeout_s), call_fut).await {
            Ok(res) => {
                runtime.terminate();
                res.map_err(|e| anyhow::anyhow!("动作执行失败: {e}"))?
            }
            Err(_) => {
                runtime.terminate();
                anyhow::bail!("动作执行超时（{} 秒）", timeout_s);
            }
        };

        serde_json::from_str(&resp_str)
            .map_err(|e| anyhow::anyhow!("动作返回值不是合法 JSON: {e}; {resp_str}"))
    }

    /// 调用插件声明的**动态 HTTP 路由**。
    ///
    /// 与 [`Self::call_action_budgeted`] 同构：每次新建沙箱、重新执行工厂、收集
    /// `ada.route(...)`，再按 method/path 匹配。路由表不落 Rust，所以插件加一条
    /// 接口只需要改自己的 `index.ts`。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_route_budgeted(
        plugin_path: &Path,
        method: &str,
        path: &str,
        request: serde_json::Value,
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
        let require_factory = plugin_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "index.ts" | "index.js"));

        let init_script = format!(
            "(function() {{ const module = {{ exports: {{}} }}; const exports = module.exports; \
             const __filename = {plugin_file}; const __dirname = path.dirname(__filename);{js_code}\n\
             globalThis.__ada_current_plugin = module.exports.default || module.exports;}})();",
            plugin_file = plugin_file_json,
            js_code = js_code.trim_start_matches(['\n', '\r'])
        );
        if let Err(e) = runtime.eval_ts(init_script, plugin_path.to_str()).await {
            runtime.terminate();
            anyhow::bail!("插件装载失败: {e}");
        }
        if let Err(e) = runtime.eval_ts(ROUTE_RUNTIME_JS, Some("ada_route_runtime.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化动态路由运行时失败: {e}");
        }

        let call_setup = format!(
            r#"
            globalThis.__ada_call_route = async function(payload) {{
                try {{
                    const desc = globalThis.__ada_current_plugin;
                    if (!desc) {{
                        return JSON.stringify({{ ok: false, error: "插件未导出有效对象或函数", kind: "failed" }});
                    }}
                    const rt = globalThis.__ada_route_runtime;
                    const ws = {ws_json};
                    const requireFactory = {require_factory};
                    let routesRaw = [];
                    if (typeof desc === 'function') {{
                        const api = {api};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, error: "插件初始化执行异常: " + (e && e.stack ? String(e.stack) : String(e)), kind: "failed" }});
                        }}
                    }} else {{
                        return JSON.stringify({{ ok: false, error: "Extension Factory must be a default-exported function", kind: "failed" }});
                    }}

                    const routes = rt.compileAll(routesRaw);
                    const matched = rt.match(routes, payload.method, payload.path);
                    if (!matched) {{
                        return JSON.stringify({{
                            ok: false,
                            kind: "not_found",
                            error: "路由不存在: " + String(payload.method || 'GET') + " " + String(payload.path || '/'),
                        }});
                    }}
                    const request = Object.assign({{}}, payload.request || {{}}, {{
                        method: payload.method,
                        path: payload.path,
                        params: matched.params,
                    }});
                    const ctx = {{
                        workspace: ws,
                        now: request.now,
                        threadId: request.threadId === undefined ? null : request.threadId,
                    }};
                    let result;
                    try {{
                        result = await matched.route.handler(request, ctx);
                    }} catch (e) {{
                        return JSON.stringify({{ ok: false, error: e && e.stack ? String(e.stack) : String(e), kind: "failed" }});
                    }}
                    return JSON.stringify({{ ok: true, response: rt.finalize(result) }});
                }} catch (err) {{
                    const detail = err && err.stack ? String(err.stack) : String(err);
                    return JSON.stringify({{ ok: false, error: detail, kind: "failed" }});
                }}
            }};
            "#,
            ws_json = ws_json,
            api = Self::api_literal(ApiSurface { routes: true, ..Default::default() }),
            require_factory = require_factory
        );
        if let Err(e) = runtime.eval_ts(call_setup, Some("ada_route_caller.js")).await {
            runtime.terminate();
            anyhow::bail!("初始化路由调度器失败: {e}");
        }

        let payload = serde_json::json!({
            "method": method,
            "path": path,
            "request": request,
        });
        let call_fut = runtime.call_async_fn(
            "__ada_call_route",
            serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        );
        let resp_str = match tokio::time::timeout(Duration::from_secs(timeout_s), call_fut).await {
            Ok(res) => {
                runtime.terminate();
                res.map_err(|e| anyhow::anyhow!("路由执行失败: {e}"))?
            }
            Err(_) => {
                runtime.terminate();
                anyhow::bail!("路由执行超时（{} 秒）", timeout_s);
            }
        };

        serde_json::from_str(&resp_str)
            .map_err(|e| anyhow::anyhow!("路由返回值不是合法 JSON: {e}; {resp_str}"))
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
        let require_factory = plugin_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "index.ts" | "index.js"));

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
        // 工具路径同样要能有路由助手：插件工厂准备响应常量时会用 `ada.text(...)`，
        // 少这份运行时就会撞 `undefined.xxx`（与"少给入口"是同一类错的两半）。
        if let Err(e) = runtime.eval_ts(ROUTE_RUNTIME_JS, Some("ada_route_runtime.js")).await {
            runtime.terminate();
            return Ok(ToolResult::error(format!("初始化动态路由运行时失败: {e}")));
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
                    const requireFactory = {require_factory};
                    let toolsRaw = [];
                    if (typeof desc === 'function') {{
                        // 调用路径只找工具：动作/路由/钩子在这里**收下就丢**是刻意的
                        // （声明清单在检查期那条路上探明）。但入口**必须存在**——
                        // 少一个，插件工厂就会抛，连工具都调不动。
                        const api = {api};
                        try {{
                            await desc(api);
                        }} catch (e) {{
                            return JSON.stringify({{ ok: false, output: "插件初始化执行异常: " + (e && e.message ? e.message : e) }});
                        }}
                    }} else if (typeof desc === 'object' && !requireFactory) {{
                        const raw = desc.tools || [];
                        for (let i = 0; i < raw.length; i++) {{
                            const item = raw[i];
                            const tool = typeof item === 'function' ? await item(ws || process.cwd()) : item;
                            if (tool) toolsRaw.push(tool);
                        }}
                    }} else {{
                        return JSON.stringify({{ ok: false, output: "Extension Factory must be a default-exported function" }});
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
            tool_name_json = tool_name_json,
            api = Self::api_literal(ApiSurface { tools: true, ..Default::default() }),
            require_factory = require_factory
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

    /// 🔴 回归：**同一个插件工厂必须在每个入口上都能跑通**。
    ///
    /// 本轮实测的真实故障：`plugins/unified-demo` 在工厂里调 `ada.route(...)`，
    /// 检查期与动态路由期都正常，但一点界面按钮（动作路径）就报
    /// `插件初始化执行异常: ... is not a function` —— 因为动作路径的 `ada` 少了
    /// `route` / `text` 入口。这类"某处少给一个入口"的错只在特定场景出现，
    /// 所以五个入口必须共用同一份 API 表面（见 [`ApiSurface`]）。
    #[tokio::test]
    async fn test_factory_runs_on_action_and_route_entries_too() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_entries_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plugin_file = temp_dir.join("multi_entry.ts");

        // 工厂里同时用 registerAction 与 route/ada.text：任何入口只要少给一个，
        // 这个插件就会在该入口上抛 TypeError。
        let plugin_code = r#"
            export default function (ada: any) {
                ada.metadata = {
                    name: "multi-entry",
                    description: "动作与动态路由同在一个工厂里",
                    tools: { count: 0, names: [] },
                    systemPrompt: { count: 0, names: [] },
                    hooks: { count: 0, names: [] },
                    actions: { count: 1, names: ["ping"] },
                    routes: { count: 1, names: ["GET /ping"] },
                };
                ada.registerAction({
                    name: "ping",
                    label: "ping",
                    mode: "read",
                    async execute() { return { ok: true, data: { pong: 1 } }; },
                });
                ada.route({
                    method: "GET",
                    path: "/ping",
                    async handler() { return ada.text("pong", 200); },
                });
            }
        "#;
        std::fs::write(&plugin_file, plugin_code).unwrap();

        let needs = ada_ts_engine::Needs::everything();
        let limits = ada_ts_engine::Limits::default();

        // 1) 检查期：动作与路由都要被收集到
        let inspection = PluginSandbox::inspect_plugin_full(&plugin_file, &temp_dir, needs)
            .await
            .expect("检查期应当成功");
        assert_eq!(inspection.actions.len(), 1);
        assert_eq!(inspection.routes.len(), 1);

        // 2) 动作期（本轮故障现场）：工厂能跑完，动作真的执行
        let action = PluginSandbox::call_action_budgeted(
            &plugin_file,
            "ping",
            serde_json::json!({}),
            serde_json::json!({ "workspace": temp_dir.to_string_lossy() }),
            "read",
            &temp_dir,
            20,
            needs,
            limits,
        )
        .await
        .expect("动作调用本身不该失败");
        assert_eq!(action["ok"], serde_json::json!(true), "动作必须能跑：{action}");
        assert_eq!(action["data"]["pong"], serde_json::json!(1), "{action}");

        // 3) 路由期：handler 里的 `ada.text(...)` 也要拿得到
        let route = PluginSandbox::call_route_budgeted(
            &plugin_file,
            "GET",
            "/ping",
            serde_json::json!({}),
            &temp_dir,
            20,
            needs,
            limits,
        )
        .await
        .expect("路由调用本身不该失败");
        assert_eq!(route["ok"], serde_json::json!(true), "路由必须能跑：{route}");
        assert_eq!(route["response"]["status"], serde_json::json!(200), "{route}");
        assert_eq!(route["response"]["body"], serde_json::json!("pong"), "{route}");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// 2026-10-19：`registerSkill` 已移除（技能改为 `skills/<name>/SKILL.md` 目录约定）。
    ///
    /// 这里钉住**新的**行为：脚本里的注册调用**不产出技能**，但必须被**记下来**
    /// （`ignoredSkills`）——静默忽略会让作者以为技能声明成功了。
    #[tokio::test]
    async fn test_sandbox_inspect_reports_removed_register_skill_calls() {
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
                api.registerSkill({ name: "another" });
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

        assert!(inspected.skills.is_empty(), "技能不再从脚本声明：{:?}", inspected.skills);
        assert_eq!(
            inspected.ignored_skills,
            vec!["deploy-runbook".to_string(), "another".to_string()],
            "旧的 registerSkill 调用要被如实记下来（装配层据此报诊断）"
        );
        assert!(inspected.tools.is_empty(), "这个插件没有工具");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// 对象式写法（`export default { skills: [...] }`）同样是旧 API：不产出技能，只记名字。
    #[tokio::test]
    async fn test_sandbox_inspect_reports_removed_object_form_skills() {
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

        assert!(inspected.skills.is_empty());
        assert_eq!(inspected.ignored_skills, vec!["style".to_string()]);

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
    /// `docs/quality/unfinished-features.md`。今天能保证的是"文件 + 位置 + 栈"都在，
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
    /// 3. **工具调用路径的 api 必须也带钩子注册入口**：插件工厂调了它就抛，
    ///    于是"带钩子的插件连工具都调不动"——这条用例的后半段专门守着它。
    #[tokio::test]
    async fn test_sandbox_call_hook_round_trip_and_tool_still_works() {
        let dir = std::env::temp_dir().join(format!("a_da_hook_rt_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guard.ts");
        std::fs::write(
            &path,
            "export default function (api: any) {\n\
               api.on('beforeToolCall', async (payload: any, ctx: any) => {\n\
                 if (String((payload.args && payload.args.cmd) || '').includes('rm -rf')) {\n\
                   return { action: 'deny', reason: '会删掉整个工作区' };\n\
                 }\n\
                 return { note: String(ctx.point) + ':' + String(ctx.workspace || '') };\n\
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

        // ③ 同一个插件的**工具**照样能调（工厂里少了 `on` 时这一步会红）
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

        let registration_dir = std::env::temp_dir().join(format!("a_da_hook_register_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&registration_dir).unwrap();
        let registration_path = registration_dir.join("registrations.ts");
        std::fs::write(
            &registration_path,
            "export default { hooks: [{ point: 'afterTurn', actions: ['observe'], handler: async (_event: any, ctx: any) => { \
              ctx.registerTool({ name: 'dynamic_tool', description: 'dynamic', parameters: { type: 'object' }, executeExport: 'dynamic_tool' }); \
              ctx.registerSystemPrompt({ id: 'dynamic-prompt', content: 'Use the dynamic prompt.' }); \
              ctx.registerSkill({ name: 'dynamic-skill', description: 'dynamic skill', path: '.ada/agent-coding/skills/dynamic-skill.md' }); \
              return { action: 'observe', data: { point: ctx.point } }; } }] };\n",
        )
        .unwrap();
        let registrations_result = PluginSandbox::call_hook_budgeted(
            &registration_path,
            "afterTurn",
            serde_json::json!({ "step": 1 }),
            &registration_dir,
            10,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("registration hook should succeed");
        let registrations = registrations_result["data"]["registrations"]
            .as_array()
            .expect("registration requests should be returned");
        assert_eq!(registrations.len(), 3);
        assert_eq!(registrations[0]["kind"], "tool");
        assert_eq!(registrations[0]["executeExport"], "dynamic_tool");
        assert_eq!(registrations[1]["kind"], "systemPrompt");
        assert_eq!(registrations[2]["kind"], "skill");
        assert_eq!(registrations[2]["path"], ".ada/agent-coding/skills/dynamic-skill.md");
        let _ = std::fs::remove_dir_all(&registration_dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_package_index_requires_matching_metadata() {
        let dir = std::env::temp_dir().join(format!("a_da_metadata_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let index = dir.join("index.ts");
        std::fs::write(
            &index,
            r#"const metadata = {
              name: 'test', description: '测试插件',
              tools: { count: 1, names: ['test_tool'] },
              systemPrompt: { count: 1, names: ['增强命中'] },
              skills: { count: 1, names: ['test_skill'] },
              hooks: { count: 1, names: ['afterTurn'] }
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.registerTool({ name: 'test_tool', description: 'tool', parameters: {}, execute: async () => 'ok' });
              ada.registerSystemPrompt({ id: '增强命中', content: '命中提示' });
              ada.registerSkill({ name: 'test_skill', description: 'skill', path: 'skills/test_skill.md', body: 'skill body' });
              ada.on('turn_end', async (event, ctx) => ({ action: 'note', note: `${ctx.point}:${event.step}` }));
            }"#,
        )
        .unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &index,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("valid package metadata should pass");
        assert_eq!(inspected.metadata.as_ref().and_then(|m| m.get("name")).and_then(|v| v.as_str()), Some("test"));
        assert_eq!(inspected.tools[0].name, "test_tool");
        assert_eq!(inspected.prompts[0].id, "增强命中");
        // 技能不再由脚本声明（目录约定），旧的 registerSkill 只被记名字
        assert!(inspected.skills.is_empty());
        assert_eq!(inspected.ignored_skills, vec!["test_skill".to_string()]);
        assert_eq!(inspected.hooks[0].point, "afterTurn");

        let hook_result = PluginSandbox::call_hook_budgeted(
            &index,
            "afterTurn",
            serde_json::json!({ "step": 3 }),
            &dir,
            10,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("pi-style on registration should execute");
        assert_eq!(hook_result["action"], "note");
        assert_eq!(hook_result["note"], "afterTurn:3");

        std::fs::write(
            &index,
            "const metadata = { name: 'test', description: 'bad', tools: { count: 2, names: ['test_tool', 'extra'] }, systemPrompt: { count: 0, names: [] }, skills: { count: 0, names: [] }, hooks: { count: 0, names: [] } }; export default function (ada) { ada.metadata = metadata; ada.registerTool({ name: 'test_tool', description: 'tool', parameters: {}, execute: async () => 'ok' }); }",
        )
        .unwrap();
        let error = PluginSandbox::inspect_plugin_full(
            &index,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect_err("metadata count mismatch must fail");
        assert!(error.to_string().contains("metadata `tools.count`"));

        std::fs::write(
            &index,
            "export default { metadata: { name: 'legacy', description: 'object form', tools: { count: 0, names: [] }, systemPrompt: { count: 0, names: [] }, skills: { count: 0, names: [] }, hooks: { count: 0, names: [] } }, tools: [] }",
        )
        .unwrap();
        let error = PluginSandbox::inspect_plugin_full(
            &index,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect_err("package index must use the pi-style factory export");
        assert!(error.to_string().contains("Extension Factory"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 守门：`create_plugin`（插件工坊）生成的骨架必须**真的能装载**。
    ///
    /// 这条比"文件写出来了"强得多：它走的是包根路径（`index.ts`），会执行
    /// `validate_package_metadata`——模板里 metadata 与注册项一旦漂移（count/names 对不上、
    /// 工厂不是 default 导出、技能没有正文），这里直接红，而不是等用户在插件中心看到 broken。
    #[tokio::test]
    async fn test_plugin_forge_scaffold_loads_and_runs() {
        let dir = std::env::temp_dir().join(format!("a_da_forge_e2e_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let res = agent_toolkit::forge::scaffold_plugin(
            &dir,
            None,
            agent_toolkit::forge::ScaffoldRequest {
                id: "demo-guard",
                scope: "workspace",
                name: Some("演示守卫"),
                description: Some("演示用：拦住没有验收标准的提交"),
                tool: None,
                overwrite: false,
            },
        );
        assert!(res.ok, "{}", res.output);

        let entry = agent_toolkit::forge::workspace_extensions_dir(&dir)
            .join("demo-guard")
            .join("index.ts");
        assert!(entry.is_file(), "包根入口必须叫 index.ts：{}", entry.display());

        let insp = PluginSandbox::inspect_plugin_full(&entry, &dir, ada_ts_engine::Needs::everything())
            .await
            .expect("生成的插件必须通过装载期 metadata 校验");
        assert_eq!(insp.tools.len(), 1);
        assert_eq!(insp.tools[0].name, "demo_guard_hello");
        assert_eq!(insp.prompts.len(), 1);
        assert_eq!(insp.prompts[0].id, "demo-guard-usage");
        // 技能不再由脚本注册（2026-10-19）：它在磁盘上 `skills/<name>/SKILL.md`，
        // 由装配层扫描（`skills::spec`）。
        assert!(insp.skills.is_empty(), "脚本里不该再注册技能：{:?}", insp.skills);
        assert!(insp.ignored_skills.is_empty(), "骨架不该调用已移除的 registerSkill");
        let skill_md = agent_toolkit::forge::workspace_extensions_dir(&dir)
            .join("demo-guard")
            .join("skills")
            .join("demo-guard")
            .join("SKILL.md");
        assert!(skill_md.is_file(), "技能必须是 skills/<name>/SKILL.md：{}", skill_md.display());
        let manifest = crate::skills::spec::load_skill_package(&skill_md)
            .expect("生成的 SKILL.md 必须通过 SKILL_SPEC 校验");
        assert_eq!(manifest.name, "demo-guard", "frontmatter 的 name 必须等于目录名");
        assert!(!manifest.body.is_empty(), "正文不能为空");
        assert!(
            insp.hooks.is_empty(),
            "骨架不该偷偷注册钩子（metadata 里写的就是 0）"
        );

        // 示例工具真的能跑（工具名/参数 schema/execute 约定任何一处写错，这里就挂）
        let out = PluginSandbox::call_tool(
            &entry,
            "demo_guard_hello",
            serde_json::json!({ "text": "你好" }),
            &dir,
            10,
        )
        .await
        .expect("示例工具必须能执行");
        assert!(out.ok, "{}", out.output);
        assert!(out.output.contains("收到：你好"), "{}", out.output);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn actions_are_inspected_and_read_intent_cannot_run_write() {
        let dir = std::env::temp_dir().join(format!(
            "a_da_action_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("index.ts");
        std::fs::write(
            &entry,
            r#"
            // @ada-needs none
            const metadata = {
              name: 'action-demo',
              description: '动作测试',
              tools: { count: 1, names: ['echo_tool'] },
              systemPrompt: { count: 0, names: [] },
              hooks: { count: 0, names: [] },
              actions: { count: 2, names: ['status', 'refresh'] },
            };
            export default function (ada) {
              ada.metadata = metadata;
              ada.registerTool({
                name: 'echo_tool',
                description: '回显',
                parameters: { type: 'object', properties: {} },
                async execute(input) { return { ok: true, output: String(input.text || '') }; },
              });
              ada.registerAction({
                name: 'status',
                description: '读取状态',
                mode: 'read',
                async execute(input, ctx) {
                  return { ok: true, data: { echo: input.echo || null, workspace: ctx.workspace } };
                },
              });
              ada.registerAction({
                name: 'refresh',
                description: '写入状态',
                mode: 'write',
                async execute() {
                  return { ok: true, data: { refreshed: true } };
                },
              });
            }
            "#,
        )
        .unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &entry,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("action plugin should inspect");
        assert_eq!(inspected.actions.len(), 2);
        assert!(inspected.actions.iter().any(|a| a.name == "status" && a.is_read()));
        assert!(inspected.actions.iter().any(|a| a.name == "refresh" && !a.is_read()));

        // 工具调用路径会重新执行工厂；工厂里 registerAction 必须存在（空实现），
        // 否则带动作的插件连工具都调不动。
        let tool = PluginSandbox::call_tool(
            &entry,
            "echo_tool",
            serde_json::json!({ "text": "ok" }),
            &dir,
            5,
        )
        .await
        .expect("带动作的插件也必须能执行工具");
        assert!(tool.ok, "{}", tool.output);
        assert_eq!(tool.output, "ok");

        let ctx = serde_json::json!({ "workspace": dir.to_string_lossy(), "caller": "web", "now": 1 });
        let read = PluginSandbox::call_action_budgeted(
            &entry,
            "status",
            serde_json::json!({ "echo": "hi" }),
            ctx.clone(),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("read action should run");
        assert_eq!(read["ok"], true);
        assert_eq!(read["data"]["echo"], "hi");

        let denied = PluginSandbox::call_action_budgeted(
            &entry,
            "refresh",
            serde_json::json!({}),
            ctx.clone(),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("denied action should return a structured refusal");
        assert_eq!(denied["ok"], false);
        assert_eq!(denied["kind"], "denied");

        let written = PluginSandbox::call_action_budgeted(
            &entry,
            "refresh",
            serde_json::json!({}),
            ctx,
            "write",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("write action should run");
        assert_eq!(written["ok"], true);
        assert_eq!(written["data"]["refreshed"], true);

        let missing = PluginSandbox::call_action_budgeted(
            &entry,
            "missing",
            serde_json::json!({}),
            serde_json::json!({}),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("missing action should return a structured refusal");
        assert_eq!(missing["kind"], "not_found");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 仓库里的 task-plan 统一包：afterTodoUpdate 写状态，plan action 读回。
    #[tokio::test]
    async fn repo_task_plan_backend_writes_and_reads_the_plan() {
        let entry = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/task-plan/index.ts");
        let dir = std::env::temp_dir().join(format!(
            "a_da_task_plan_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &entry,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("task-plan backend should inspect");
        assert!(
            inspected.hooks.iter().any(|hook| hook.point == "afterTodoUpdate"),
            "hooks: {:?}",
            inspected.hooks
        );
        assert!(
            inspected.actions.iter().any(|action| action.name == "plan" && action.is_read()),
            "actions: {:?}",
            inspected.actions
        );

        let hook = PluginSandbox::call_hook_budgeted(
            &entry,
            "afterTodoUpdate",
            serde_json::json!({
                "__ada_hook_event": {
                    "todos": [
                        { "id": "t1", "title": "读列表", "status": "completed" },
                        { "id": "t2", "title": "写条目", "status": "in_progress" }
                    ],
                    "diff": ""
                },
                "__ada_hook_ctx": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "callId": "call-1",
                    "pluginId": "task-plan",
                    "point": "afterTodoUpdate",
                    "timeoutMs": 500
                }
            }),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("afterTodoUpdate should run");
        assert!(
            hook.get("error").is_none(),
            "hook should not fail: {hook:?}"
        );

        let plan = PluginSandbox::call_action_budgeted(
            &entry,
            "plan",
            serde_json::json!({ "threadId": "thread-1" }),
            serde_json::json!({ "workspace": dir.to_string_lossy(), "caller": "web", "now": 1 }),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("plan action should run");
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["data"]["total"], 2);
        assert_eq!(plan["data"]["done"], 1);
        assert_eq!(plan["data"]["active"], "写条目");
        assert_eq!(plan["data"]["steps"][1]["title"], "写条目");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 仓库里的 approval 统一包：beforeApproval 记 pending，afterApproval 累计。
    #[tokio::test]
    async fn repo_approval_backend_records_pending_and_decisions() {
        let entry = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/approval/index.ts");
        let dir = std::env::temp_dir().join(format!(
            "a_da_approval_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &entry,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("approval backend should inspect");
        assert!(inspected
            .hooks
            .iter()
            .any(|hook| hook.point == "beforeApproval"));
        assert!(inspected
            .hooks
            .iter()
            .any(|hook| hook.point == "afterApproval"));
        assert!(inspected
            .actions
            .iter()
            .any(|action| action.name == "pending" && action.is_read()));

        let hook_ctx = |call_id: &str| {
            serde_json::json!({
                "__ada_hook_ctx": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "callId": call_id,
                    "pluginId": "approval",
                    "point": "beforeApproval",
                    "timeoutMs": 500
                }
            })
        };
        let before = PluginSandbox::call_hook_budgeted(
            &entry,
            "beforeApproval",
            serde_json::json!({
                "__ada_hook_event": {
                    "callId": "call-1",
                    "tool": "run_command",
                    "args": { "command": "cargo test" },
                    "isWrite": true
                },
                "__ada_hook_ctx": hook_ctx("call-1")["__ada_hook_ctx"]
            }),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("beforeApproval should run");
        assert!(before.get("error").is_none(), "{before:?}");

        let pending = PluginSandbox::call_action_budgeted(
            &entry,
            "pending",
            serde_json::json!({ "threadId": "thread-1" }),
            serde_json::json!({ "workspace": dir.to_string_lossy(), "caller": "web", "now": 10_000 }),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("pending action should run");
        assert_eq!(pending["data"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(pending["data"]["items"][0]["tool"], "run_command");

        let after = PluginSandbox::call_hook_budgeted(
            &entry,
            "afterApproval",
            serde_json::json!({
                "__ada_hook_event": {
                    "tool": "run_command",
                    "approved": true,
                    "by": "user",
                    "waitedMs": 1200
                },
                "__ada_hook_ctx": hook_ctx("call-1")["__ada_hook_ctx"]
            }),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("afterApproval should run");
        assert!(after.get("error").is_none(), "{after:?}");

        let pending = PluginSandbox::call_action_budgeted(
            &entry,
            "pending",
            serde_json::json!({ "threadId": "thread-1" }),
            serde_json::json!({ "workspace": dir.to_string_lossy(), "caller": "web", "now": 20_000 }),
            "read",
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("pending action should run");
        assert!(pending["data"]["items"].as_array().unwrap().is_empty());
        assert_eq!(pending["data"]["approved"], 1);
        assert_eq!(pending["data"]["rejected"], 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_dynamic_routes_match_params_and_return_http_shapes() {
        let dir = std::env::temp_dir().join(format!("a_da_route_rt_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("routes.ts");
        std::fs::write(
            &path,
            r#"
            export default function (ada: any) {
              ada.route({
                method: "GET",
                path: "/hello/:name",
                async handler(req: any) {
                  return { hello: req.params.name, q: req.query.q, thread: req.threadId };
                },
              });
              ada.route({
                method: "POST",
                path: "/echo",
                async handler(req: any, ctx: any) {
                  return { body: req.body, kind: req.bodyKind, ws: ctx.workspace };
                },
              });
              ada.route({
                method: "GET",
                path: "/text",
                async handler() { return ada.text("plain", 201, { "x-note": "ok" }); },
              });
              ada.route({
                method: "GET",
                path: "/bin",
                async handler() {
                  return ada.response({
                    status: 200,
                    body: "aGk=",
                    bodyKind: "base64",
                    contentType: "application/octet-stream",
                  });
                },
              });
              ada.route({
                method: "GET",
                path: "/files/*rest",
                async handler(req: any) { return { rest: req.params.rest }; },
              });
            }
            "#,
        )
        .unwrap();

        let inspected = PluginSandbox::inspect_plugin_full(
            &path,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect("route plugin should inspect");
        assert_eq!(inspected.routes.len(), 5, "{:?}", inspected.routes);
        assert!(inspected
            .routes
            .iter()
            .any(|route| route.method == "GET" && route.path == "/hello/:name"));

        let request = |extra: serde_json::Value| {
            let mut base = serde_json::json!({
                "method": "GET",
                "path": "/hello/ada",
                "query": {},
                "headers": {},
                "body": null,
                "bodyKind": "empty",
                "workspace": dir.to_string_lossy(),
                "threadId": "thread-1",
                "now": 1_700_000_000_000u64,
            });
            if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    base.insert(key.clone(), value.clone());
                }
            }
            base
        };

        let hello = PluginSandbox::call_route_budgeted(
            &path,
            "GET",
            "/hello/ada",
            request(serde_json::json!({ "query": { "q": "1" } })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("route should run");
        assert_eq!(hello["ok"], true, "{hello:?}");
        let response = &hello["response"];
        assert_eq!(response["status"], 200);
        let payload: serde_json::Value =
            serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
        assert_eq!(payload["hello"], "ada");
        assert_eq!(payload["q"], "1");
        assert_eq!(payload["thread"], "thread-1");

        let echo = PluginSandbox::call_route_budgeted(
            &path,
            "POST",
            "/echo",
            request(serde_json::json!({
                "method": "POST",
                "path": "/echo",
                "body": { "a": 1 },
                "bodyKind": "json",
            })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("POST route should run");
        let payload: serde_json::Value =
            serde_json::from_str(echo["response"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(payload["body"]["a"], 1);
        assert_eq!(payload["kind"], "json");
        assert_eq!(
            payload["ws"].as_str().unwrap(),
            dir.to_string_lossy().to_string()
        );

        let text = PluginSandbox::call_route_budgeted(
            &path,
            "GET",
            "/text",
            request(serde_json::json!({ "path": "/text" })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("text helper should run");
        assert_eq!(text["response"]["status"], 201);
        assert_eq!(text["response"]["body"], "plain");
        assert_eq!(
            text["response"]["headers"]["x-note"],
            "ok",
            "{text:?}"
        );

        let bin = PluginSandbox::call_route_budgeted(
            &path,
            "GET",
            "/bin",
            request(serde_json::json!({ "path": "/bin" })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("binary helper should run");
        assert_eq!(bin["response"]["bodyKind"], "base64");
        assert_eq!(bin["response"]["body"], "aGk=");

        let wild = PluginSandbox::call_route_budgeted(
            &path,
            "GET",
            "/files/a/b.txt",
            request(serde_json::json!({ "path": "/files/a/b.txt" })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("wildcard route should run");
        let payload: serde_json::Value =
            serde_json::from_str(wild["response"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(payload["rest"], "a/b.txt");

        let head = PluginSandbox::call_route_budgeted(
            &path,
            "HEAD",
            "/hello/ada",
            request(serde_json::json!({ "method": "HEAD", "path": "/hello/ada" })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("HEAD should fall back to GET");
        assert_eq!(head["response"]["status"], 200);

        let missing = PluginSandbox::call_route_budgeted(
            &path,
            "GET",
            "/nope",
            request(serde_json::json!({ "path": "/nope" })),
            &dir,
            5,
            ada_ts_engine::Needs::everything(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("missing route should return a structured miss");
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["kind"], "not_found");

        let bad = dir.join("duplicate.ts");
        std::fs::write(
            &bad,
            r#"
            export default function (ada: any) {
              ada.route({ method: "GET", path: "/x", handler: () => 1 });
              ada.route({ method: "GET", path: "/x", handler: () => 2 });
            }
            "#,
        )
        .unwrap();
        let error = PluginSandbox::inspect_plugin_full(
            &bad,
            &dir,
            ada_ts_engine::Needs::everything(),
        )
        .await
        .expect_err("duplicate route should fail inspection");
        assert!(error.to_string().contains("重复的路由"), "{error}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
