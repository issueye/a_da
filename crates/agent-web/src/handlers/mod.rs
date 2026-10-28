//! HTTP 处理器：按页面家族拆成子模块，这里只放共享类型与工具。
//!
//! - [`chat`]：对话页 / live 轮询 / 会话动作（发送、中止、重试、排队、审批）
//! - [`dashboard`]：概览
//! - [`providers`]：供应商与模型管理（含编辑/新增弹窗）
//! - [`extensions`]：插件页与插件动作
//!
//! 新增页面请按家族放进对应子模块，不要再往本文件里堆。

use askama::Template;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::AgentClient;
use crate::context::{AppMeta, RenderContext};
use crate::guard::SharedAccess;
use crate::plugins::{PluginRegistry, Slot};
use crate::templates::*;
use crate::ui::{self, ConfirmDialog};

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<AgentClient>,
    pub frontend: Arc<PluginRegistry>,
    pub access: SharedAccess,
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn upstream(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>ssr-h 错误</title>\
             <body style=\"font:14px/1.6 system-ui;padding:2rem;max-width:720px;margin:auto\">\
             <h1 style=\"font-size:18px\">请求失败</h1><p style=\"color:#b91c1c\">{}</p>\
             <p><a href=\"/\">返回概览</a></p></body>",
            escape_html(&self.message)
        );
        (self.status, Html(body)).into_response()
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render<T: Template>(template: &T) -> Result<Html<String>, AppError> {
    template.render().map(Html).map_err(|error| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("模板渲染失败: {error}"),
        )
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    notice: Option<String>,
    error: Option<String>,
    count: Option<usize>,
    failed: Option<usize>,
    thread: Option<String>,
    /// 供应商页当前选中项（Master-Detail 的 Detail）。
    provider: Option<String>,
    /// 供应商详情区的页签（`connection` / `models`，缺省为 `connection`）。
    tab: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateThreadForm {
    title: String,
    /// 目标工作区；缺省时由侧栏/后端决定（通常是当前激活工作区）。
    workspace: Option<String>,
    /// 会话模式（code / plan / create / pm）；缺省 code。开始后不再可切（与 原桌面端 一致）。
    mode: Option<String>,
}


fn plugin_redirect_target(referer: Option<&str>, location: &str) -> Option<String> {
    let referer = referer?.parse::<axum::http::Uri>().ok()?;
    let path = referer.path();
    if !path.starts_with("/ext/")
        || path.starts_with("//")
        || path.contains('\\')
        || path.split('/').any(|part| part == "..")
    {
        return None;
    }
    if location != "/providers" && !location.starts_with("/providers?") {
        return None;
    }
    match location.split_once('?') {
        Some((_, query)) if !query.is_empty() => Some(format!("{path}?{query}")),
        _ => Some(path.to_string()),
    }
}

/// 供应商动作原本回跳 `/providers`；从插件页提交时按 Referer 改回插件页。
pub async fn rewrite_plugin_redirect(request: Request, next: Next) -> Response {
    let referer = request
        .headers()
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut response = next.run(request).await;
    let Some(location) = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
    else {
        return response;
    };
    if let Some(target) = plugin_redirect_target(referer.as_deref(), &location) {
        if let Ok(value) = HeaderValue::from_str(&target) {
            response.headers_mut().insert(header::LOCATION, value);
        }
    }
    response
}

/// notice key → 中文文案，**单一真源**。
///
/// 🔴 新增「动作成功回跳」必须在这里登记，否则横幅会把英文 key 原样显示给用户；
/// `tests::notice_literals_are_translated` 扫描 `src/` 里的 `notice=<key>` 字面量守着这张表。
/// 已知盲区：以参数形式传给 `provider_back` / `provider_back_dialog`（providers.rs）的
/// key 不在扫描范围内，改那两个函数的调用点时人工对照本表。
const NOTICE_TEXTS: &[(&str, &str)] = &[
    ("frontend_reloaded", "前端插件已重新扫描"),
    ("agent_reloaded", "已请求 agent 重载插件；新增工具将在下一条消息生效"),
    ("backend_action_ok", "后端动作已执行"),
    ("task_killed", "后台任务已终止"),
    ("created", "会话已创建"),
    ("sent", "消息已发送"),
    ("aborted", "已请求中止当前轮次"),
    ("retried", "已请求重试上一轮"),
    ("model_selected", "已切换模型"),
    ("effort_selected", "已更新推理等级"),
    ("queue_promoted", "已把排队消息提升到队首"),
    ("queue_removed", "已删除排队消息"),
    ("queue_cleared", "已清空当前会话的排队消息"),
    ("workspace_selected", "已切换工作区"),
    ("workspace_created", "已在工作区创建会话"),
    ("workspace_mkdir", "已新建目录"),
    ("plugin_enabled", "插件已启用并挂载到界面"),
    ("plugin_disabled", "插件已停用，界面已回到默认实现"),
    ("provider_created", "已新建供应商，填好端点与密钥后保存"),
    ("provider_saved", "供应商连接参数已保存"),
    ("provider_saved_activated", "已保存并设为当前供应商"),
    ("provider_activated", "已切换当前供应商"),
    ("provider_deleted", "供应商已删除"),
    ("model_added", "模型已加入该供应商"),
    ("model_saved", "模型参数已保存"),
    ("model_deleted", "模型已从该供应商移除"),
    ("thread_renamed", "已重命名会话"),
    ("thread_deleted", "已删除会话"),
    ("thread_mode_set", "已切换会话模式"),
    ("thread_workspace_set", "已切换会话工作区"),
];

fn notice_text(notice: &str) -> Option<String> {
    NOTICE_TEXTS
        .iter()
        .find(|(key, _)| *key == notice)
        .map(|(_, text)| (*text).to_string())
}

/* ── 局部提交（`data-partial`）────────────────────────────────────────────
 *
 * composer 周边的轻动作（切模型 / 推理等级 / 审批档位 / 批准拒绝 / 排队增删）
 * 默认是整页 POST → 303 回跳：页面重建会打断阅读位置、展开态与草稿。
 * 前端给这类表单加 `data-partial`，fetch 带 `Accept: application/json` 提交；
 * 服务端命中这个头时返回 JSON 回执（不 303），由客户端就地反馈 + 拉一帧 live。
 *
 * 🔴 无 JS / 旧页面照旧走 303：这里只是**多一个分流**，不是替换原有跳转路径。
 * 文案与跳转式 flash 共用 `NOTICE_TEXTS`，避免两套口径。
 */
fn wants_partial(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("application/json"))
        .unwrap_or(false)
}

fn partial_ok(notice_key: &str) -> Response {
    let message = notice_text(notice_key).unwrap_or_else(|| notice_key.to_string());
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "notice": message })),
    )
        .into_response()
}

fn partial_err(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({ "ok": false, "error": message.into() })),
    )
        .into_response()
}

fn flash_from_query(query: &PageQuery) -> Option<Flash> {
    if let Some(error) = &query.error {
        return Some(Flash {
            kind: "error".to_string(),
            message: error.clone(),
        });
    }
    let notice = query.notice.as_deref()?;
    let message = if notice == "frontend_reloaded" {
        // 唯一需要 query 参数（count/failed）的分支；表里的同名条目只作守门对账用。
        let failed = query.failed.unwrap_or(0);
        if failed > 0 {
            format!(
                "前端插件已重新扫描：{} 个（{} 个加载失败，见下方错误）",
                query.count.unwrap_or(0),
                failed
            )
        } else {
            format!("前端插件已重新扫描：当前 {} 个", query.count.unwrap_or(0))
        }
    } else {
        notice_text(notice).unwrap_or_else(|| {
            tracing::warn!("未登记中文文案的 notice key：{notice}（横幅会直接显示 key）");
            notice.to_string()
        })
    };
    Some(Flash {
        kind: "success".to_string(),
        message,
    })
}

/// 二级页的离线壳基座：保留侧栏但把连接状态点画成"未连接"。
///
/// 与聊天页的 offline_base 同口径；页面内容换成 [`OfflinePage`]，不渲染假统计。
async fn offline_page_base(
    state: &AppState,
    title: &str,
    active: &str,
    flash: Option<Flash>,
) -> PageBase {
    let mut base = base(state, title, active, flash).await;
    base.sidebar = ui::sidebar::Sidebar::from_base(&base)
        .connected(Some(false))
        .render();
    base
}

async fn base(state: &AppState, title: &str, active: &str, flash: Option<Flash>) -> PageBase {
    base_with_snapshot(state, title, active, flash, None, None).await
}

/// 页面外壳 + 快照。
///
/// 有快照的页面（聊天页）把 `data.tokens` / `data.todo` 一起给插槽，插件才能显示
/// 实时用量与任务计划；没有快照的页面（供应商/扩展）维持原样，插件读到 `null`。
///
/// `thread_id` 是"这些插槽属于哪条会话"：聊天页传正在渲染的那条（深链接时即使
/// `thread.focus` 没生效，卡片也不会拿错会话的计划）；其它页面传 `None`。
async fn base_with_snapshot(
    state: &AppState,
    title: &str,
    active: &str,
    flash: Option<Flash>,
    snapshot: Option<&Value>,
    thread_id: Option<&str>,
) -> PageBase {
    let mut context = match snapshot {
        Some(snapshot) => render_context_with_tokens(state, active, snapshot, thread_id),
        None => render_context(state, active),
    };
    // 进程管理是**插件**（`process-manager`，声明 `processes.read`）：
    // 只有真有启用插件要这份数据时才去问网关，停用插件后不再白跑。
    if state.frontend.has_capability("processes.read") {
        let processes = load_processes(state).await;
        if let Some(data) = context.data.as_object_mut() {
            data.insert("processes".to_string(), processes);
        }
    }
    attach_backend_actions(state, &mut context).await;
    attach_tasks(state, &mut context, None).await;
    let slots = slot_content(&state.frontend, context).await;
    // 侧栏"新会话"要落在当前激活工作区。快照里的 `workspace.project` 是 agent 的真实
    // 当前项目（与 原桌面端 的 `activeWorkspace` 同源）；没有快照时退回启动参数。
    let active_workspace = snapshot
        .and_then(|snapshot| snapshot.get("workspace"))
        .and_then(|workspace| workspace.get("project"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.agent.workspace.clone());
    let mut base = PageBase {
        title: title.to_string(),
        active: active.to_string(),
        active_workspace_name: workspace_name(&active_workspace),
        active_workspace,
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        plugin_nav: state.frontend.nav_links(),
        plugin_styles: state.frontend.styles(),
        plugin_scripts: state.frontend.scripts(),
        flash,
        slots,
        icons: ui::sprite(),
        sidebar: String::new(),
        // 每个页面外壳都带一份确认弹窗（组件库渲染），二级页不再退回原生 confirm。
        confirm_dialog: ConfirmDialog::markup(),
    };
    // 侧栏由组件库渲染（模板只摆位置）。二级页没有会话树，先给一份不含列表的；
    // 聊天页拿到工作区树后再覆盖一次。
    base.sidebar = ui::sidebar::Sidebar::from_base(&base).render();
    base
}

/// 插件片段/页面的只读上下文：不把 `AppState` 整个暴露给模板。
fn render_context(state: &AppState, active: &str) -> RenderContext {
    RenderContext::new(AppMeta {
        workspace: state.agent.workspace.clone(),
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .with_data(serde_json::json!({
        "plugin_count": state.frontend.count(),
    }))
    .with_route_path(active)
}

/// 带上下文用量的渲染上下文。
///
/// 只有拿得到 `snapshot` 的页面（插件页、概览卡片）才有完整的 `data`；
/// 拿不到时插件读到 `tokens = null` / `thread = null`，页面照常渲染。
///
/// `thread_id` 是"这个插槽属于哪条会话"：聊天页传当前会话，二级页传 `None`
/// （沿用快照的 `activeThreadId`）。不传会出现"界面在看 A，天气插件按快照的
/// activeThreadId（可能是 B）算用量"——实测显示成 0%。
///
/// 数据结构见 [docs/PLUGIN-DATA.md](../../docs/PLUGIN-DATA.md)。
fn render_context_with_tokens(
    state: &AppState,
    active: &str,
    snapshot: &Value,
    thread_id: Option<&str>,
) -> RenderContext {
    let mut data = plugin_data(snapshot, thread_id, state.frontend.count());
    // `runs`/`processes` 同一口径：只有**已启用**的插件声明了能力才算这份投影
    // （停用即消失，不白遍历一遍会话 item）。
    if state.frontend.has_capability("runs.read") {
        if let Some(object) = data.as_object_mut() {
            object.insert("runs".to_string(), runs_view(thread_view(snapshot, thread_id)));
        }
    }
    render_context(state, active).with_data(data)
}

/// 插件可见的数据投影（唯一出口）。
///
/// 只给**事实**：数量、状态、来自模型自己报回的 usage；不给 apiKey、不给原始
/// `providers[]`（里面带密钥）。新增字段要同步 `docs/PLUGIN-DATA.md`。
fn plugin_data(snapshot: &Value, thread_id: Option<&str>, plugin_count: usize) -> Value {
    let thread = thread_view(snapshot, thread_id);
    serde_json::json!({
        "plugin_count": plugin_count,
        // 渲染时刻（epoch ms）：插件做"几秒前"之类的相对时间时用它，别信浏览器时钟。
        "render": { "at": now_ms() },
        "tokens": token_usage_view(snapshot, thread_id),
        "thread": thread_summary(thread, snapshot),
        "model": model_view(snapshot),
        "config": config_view(snapshot),
        "workspace": workspace_view(snapshot, thread),
        "counts": counts_view(snapshot, thread),
        // 任务规划（`todo` 工具）的当前计划：卡片由 `task-plan` 插件渲染，
        // 计划状态的推演仍留在核心（set/update/get 的语义只有一处）。
        "todo": todo_view(snapshot, thread_id),
    })
}

/// 给统一插件的前端插槽预取只读后端动作。
///
/// 只跑 `frontend.actions.read=true` 的绑定；每个动作独立超时，失败写进
/// `props.data.backend.<plugin>.<binding>.error`，**不让整页 500**。
/// 写入动作永远不预取——它们只能由浏览器显式 POST 触发。
async fn attach_backend_actions(state: &AppState, context: &mut RenderContext) {
    let bindings = state.frontend.backend_reads();
    if bindings.is_empty() {
        return;
    }
    if !context.data.is_object() {
        context.data = serde_json::json!({});
    }
    // 后端动作可能需要会话 id（task-plan 的计划按 threadId 存）；没有会话就传空对象。
    let thread_id = context
        .data
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut backend = serde_json::Map::new();
    for binding in bindings.into_iter().take(16) {
        let input = match &thread_id {
            Some(thread_id) => serde_json::json!({ "threadId": thread_id }),
            None => serde_json::json!({}),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(6),
            state.agent.invoke_plugin_action(
                &binding.backend_plugin_id,
                &binding.action,
                input,
                "read",
            ),
        )
        .await;
        let value = match result {
            Ok(Ok(value)) => serde_json::json!({
                "ok": true,
                "data": value.get("data").cloned().unwrap_or(value),
                "error": Value::Null,
                "fetchedAt": now_ms(),
            }),
            Ok(Err(error)) => serde_json::json!({
                "ok": false,
                "data": Value::Null,
                "error": public_reason(&error),
                "fetchedAt": now_ms(),
            }),
            Err(_) => serde_json::json!({
                "ok": false,
                "data": Value::Null,
                "error": "后端动作预取超时",
                "fetchedAt": now_ms(),
            }),
        };
        let entry = backend
            .entry(binding.plugin_id.clone())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(object) = entry.as_object_mut() {
            object.insert(binding.name.clone(), value);
        }
    }
    if let Some(data) = context.data.as_object_mut() {
        data.insert("backend".to_string(), Value::Object(backend));
    }
}

/// 给声明 `tasks.read` 的插件抓取后台任务。
///
/// 任务表在 agent 进程的 `command` 工具包里（`BackgroundTasks`）；这里只做投影，
/// 不缓存第二份状态。拿不到时给空列表 + 错误文案，插件自己显示“读取失败”。
///
/// `preset`：调用方**刚刚**为同一条会话取到的任务表（SSE 事件循环为算指纹每拍取一次）。
/// 给了就不再问一遍网关——否则同一帧会打两次 `task.list`。
async fn attach_tasks(state: &AppState, context: &mut RenderContext, preset: Option<&Value>) {
    if !state.frontend.has_capability("tasks.read") {
        return;
    }
    if !context.data.is_object() {
        context.data = serde_json::json!({});
    }
    if let Some(value) = preset {
        if let Some(data) = context.data.as_object_mut() {
            data.insert("tasks".to_string(), value.clone());
        }
        return;
    }
    // 任务管理是**当前会话**的视图：没有会话（插件页/仪表盘）就只给空列表，
    // 不能把整个工作区乃至整个 agent 进程的后台任务摊到别的页面上。
    let thread_id = context
        .data
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let value = match thread_id.as_deref() {
        Some(thread_id) => {
            match tokio::time::timeout(
                Duration::from_secs(3),
                state.agent.tasks(Some(thread_id)),
            )
            .await
            {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => serde_json::json!({
                    "tasks": [],
                    "count": 0,
                    "error": public_reason(&error),
                }),
                Err(_) => serde_json::json!({
                    "tasks": [],
                    "count": 0,
                    "error": "后台任务读取超时",
                }),
            }
        }
        None => serde_json::json!({ "tasks": [], "count": 0 }),
    };
    if let Some(data) = context.data.as_object_mut() {
        data.insert("tasks".to_string(), value);
    }
}

/// 当前会话最近执行过的**前台命令**（`run_command`），给声明 `runs.read` 的插件。
///
/// # 为什么从会话快照取，而不是新增一份 RPC
///
/// `run_command` 是**阻塞调用**：它结束后不留任何活体句柄（不在 `BackgroundTasks`
/// 里，也没有可终止的进程），唯一权威记录就是它在对话流里留下的那条 tool item——
/// 也就是用户看到的"执行命令 + 执行输出 + 输入参数"卡片本身。
///
/// 所以这里**只做投影**（命令 / cwd / 退出码 / 耗时 / 输出预览全部原样取自 item），
/// 不新开 RPC、不缓存第二份状态。代价是历史被压缩（`compact` 掉的老 item 不再出现），
/// 而这恰好就是"最近命令"的语义。
///
/// 没跑过命令 / 当前会话不在快照里 → 空列表（这是"还没有"，不是"读取失败"）。
fn runs_view(thread: Option<&Value>) -> Value {
    /// 最多回看多少条：面板是"最近命令"，不是完整历史。
    const RUNS_LIMIT: usize = 20;
    /// 单条输出预览上限（与后台任务的 4000 同量级，这里更小：面板里一次要挤 20 条）。
    const OUTPUT_PREVIEW: usize = 1200;

    let items = thread
        .and_then(|thread| thread.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut runs: Vec<Value> = Vec::new();
    for item in items {
        if str_at(item, "kind") != "tool" || str_at(item, "name") != "run_command" {
            continue;
        }
        // 参数既可能是对象，也可能是模型原始吐出的 JSON 字符串（`rawArgs`）。
        // 口径与工具行一致（对话流里的"输入参数"就是这么解析的）。
        let args = match item
            .get("args")
            .filter(|value| value.is_object())
            .or_else(|| item.get("rawArgs"))
            .cloned()
            .unwrap_or(Value::Null)
        {
            Value::String(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)),
            other => other,
        };
        let command = str_at(&args, "command").trim().to_string();
        let cwd = str_at(&args, "cwd").trim().to_string();
        let status = str_at(item, "status").to_string();
        let output = item.get("output").and_then(Value::as_str).unwrap_or("");
        // 退出码只认结构化字段（与工具行尾标同一套口径），不解析输出措辞。
        let exit_code = item
            .get("details")
            .and_then(|details| {
                details
                    .get("exitCode")
                    .or_else(|| details.get("exit_code"))
            })
            .and_then(Value::as_i64);
        runs.push(serde_json::json!({
            "id": str_at(item, "id"),
            "callId": str_at(item, "callId"),
            "command": command,
            "cwd": cwd,
            "status": status,
            "running": matches!(status.as_str(), "running" | "queued" | "awaiting"),
            "exitCode": exit_code,
            "durationMs": item.get("durationMs").and_then(Value::as_u64),
            "startedAt": item.get("startedAt").and_then(Value::as_i64),
            "at": item.get("at").and_then(Value::as_i64),
            "outputLines": output.lines().count(),
            "outputPreview": truncate(output, OUTPUT_PREVIEW),
        }));
    }
    // 面板自上而下"新 → 旧"：会话 item 是正序的，这里翻一次；超出上限的老记录直接丢。
    runs.reverse();
    let total = runs.len();
    runs.truncate(RUNS_LIMIT);
    serde_json::json!({ "runs": runs, "count": runs.len(), "total": total })
}

/// 给 `processes.read` 插件的进程投影。
///
/// 只给网关自己产生的管理事实（pid / 状态 / 工作区 / 已剥 token 的端点）；
/// 拿不到时返回 `null` 字段 + 错误文案，插件据此显示"读取失败"而不是空白面板。
async fn load_processes(state: &AppState) -> Value {
    let (status, agents) = tokio::join!(
        state.agent.gateway_status(),
        state.agent.gateway_agents()
    );
    match (status, agents) {
        (Ok(status), Ok(agents)) => serde_json::json!({
            "status": status,
            "agents": agents.get("agents").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
            "count": agents.get("count").and_then(Value::as_u64).unwrap_or(0),
            "error": Value::Null,
        }),
        (status, agents) => {
            let error = status
                .err()
                .or_else(|| agents.err())
                .map(|error| error.to_string())
                .unwrap_or_default();
            serde_json::json!({
                "status": null,
                "agents": [],
                "count": 0,
                "error": error,
            })
        }
    }
}


fn now_ms() -> u64 {
    crate::ui::now_ms()
}

fn percent_of(part: u64, whole: u64) -> Option<u64> {
    (whole > 0).then(|| {
        let scaled = part.saturating_mul(100).saturating_add(whole / 2);
        (scaled / whole).min(100)
    })
}

/// 当前会话的**上下文用量**（给插件用）。
///
/// 口径与引擎的上下文治理一致：最近一次模型调用如实报回的 `usage.promptTokens`
/// 除以 `config.contextWindow`——是"这一轮实际塞进窗口的输入规模"，不是累计消费，
/// 也不是按字符估算。窗口未配置（0）时 `percent` 为 `null`，由插件自己决定怎么显示。
fn token_usage_view(snapshot: &Value, thread_id: Option<&str>) -> Value {
    let thread = thread_view(snapshot, thread_id);
    let latest = thread.and_then(last_usage);
    let usage = latest
        .map(|(usage, _)| usage)
        .cloned()
        .unwrap_or(Value::Null);
    let duration = latest.map(|(_, duration)| duration).unwrap_or(0);
    let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let used = number("promptTokens");
    let completion = number("completionTokens");
    let thinking = number("thinkingTokens");
    let total = {
        let raw = number("totalTokens");
        if raw > 0 {
            raw
        } else {
            used.saturating_add(completion)
        }
    };
    // 缓存命中率：优先最近一次调用；模型不是每次都报 `cachedTokens`，缺失时回落到
    // 最近一次**上报过缓存**的调用，并用 `cacheStale` 如实标记（别把旧值当成本轮）。
    let cache_usage = latest
        .map(|(usage, _)| usage)
        .filter(|usage| usage.get("cachedTokens").is_some())
        .or_else(|| thread.and_then(last_usage_with_cache));
    let cache_stale = match (latest.map(|(usage, _)| usage), cache_usage) {
        (Some(latest_usage), Some(cache_usage)) => !std::ptr::eq(latest_usage, cache_usage),
        _ => false,
    };
    let cached = cache_usage
        .and_then(|usage| usage.get("cachedTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_rate = cache_usage.and_then(|usage| {
        let cached = usage.get("cachedTokens").and_then(Value::as_u64)?;
        let prompt = usage
            .get("promptTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        (prompt > 0).then(|| {
            // 保留一位小数：缓存命中常在 99.x%，四舍五入成 100% 会让人以为"从不 miss"。
            ((cached as f64) * 1000.0 / (prompt as f64)).round() / 10.0
        })
    });
    let cache_percent = cache_rate.map(|rate| rate.round() as u64);
    let window = snapshot
        .get("config")
        .and_then(|config| config.get("contextWindow"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    tracing::debug!(
        thread = thread.map(|thread| str_at(thread, "id")).unwrap_or(""),
        found = thread.is_some(),
        items = thread
            .and_then(|thread| thread.get("items"))
            .and_then(|items| items.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        used,
        window,
        "token usage view"
    );
    let percent = percent_of(used, window);
    // 会话显示名与侧栏同口径：原始标题是"新会话"时，用第一条用户消息兜底。
    // 否则插件浮窗会显示"新会话"，而界面标题早就变成任务名了。
    let thread_title = thread.map(thread_title).unwrap_or_default();
    serde_json::json!({
        "available": latest.is_some(),
        "used": used,
        "window": window,
        "percent": percent,
        // 缓存命中：模型报回的 cachedTokens ÷ promptTokens（没有缓存字段时为空）。
        "cached": cached,
        "cachePercent": cache_percent,
        // 一位小数的精确命中率（99.8 而不是 100）；老插件可继续用 cachePercent。
        "cacheRate": cache_rate,
        // 本轮没有上报缓存、数字来自更早一次调用时为 true（UI 应标注来源）。
        "cacheStale": cache_stale,
        "completion": completion,
        "thinking": thinking,
        "total": total,
        // 速度：completionTokens ÷ durationMs；太短的调用算不出有意义的速率。
        "tokensPerSec": (duration >= 200 && completion > 0)
            .then(|| (completion.saturating_mul(1000) / duration).max(1)),
        "durationMs": duration,
        "threadId": thread.map(|thread| str_at(thread, "id")).unwrap_or(""),
        "threadTitle": thread_title,
        "model": snapshot
            .get("config")
            .and_then(|config| config.get("model"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
}

/// 最近一次**上报了** `cachedTokens` 的助手回复的 `usage`。
///
/// 有些模型只在部分请求里返回缓存明细；没有它时不要显示 0%，但可以沿用
/// 上一条真实上报（UI 用 `cacheStale` 标注"不是本轮"）。
fn last_usage_with_cache(thread: &Value) -> Option<&Value> {
    array_at(thread, "items").iter().rev().find_map(|item| {
        if str_at(item, "kind") != "assistant" {
            return None;
        }
        item.get("usage")
            .filter(|usage| usage.get("cachedTokens").and_then(Value::as_u64).is_some())
    })
}

/// 会话里**最近一次**带用量的助手回复的 `usage` 与耗时。
///
/// 一轮里有多次工具调用时，最后一条助手卡的 promptTokens 就是"当前上下文大小"；
/// 流式中的助手卡还没有 usage，要跳过而不是当成 0。
fn last_usage(thread: &Value) -> Option<(&Value, u64)> {
    array_at(thread, "items").iter().rev().find_map(|item| {
        if str_at(item, "kind") != "assistant" {
            return None;
        }
        let usage = item.get("usage")?;
        let prompt = usage
            .get("promptTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if prompt == 0 {
            return None;
        }
        Some((
            usage,
            item.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
        ))
    })
}

/// 会话摘要（给插件）。
fn thread_summary(thread: Option<&Value>, snapshot: &Value) -> Value {
    let Some(thread) = thread else {
        return Value::Null;
    };
    let id = str_at(thread, "id");
    let mode = {
        let mode = str_at(thread, "mode");
        if mode.is_empty() {
            "code"
        } else {
            mode
        }
    };
    serde_json::json!({
        "id": id,
        "title": thread_title(thread),
        "workspace": str_at(thread, "workspace"),
        "mode": mode,
        "agentId": str_at(thread, "agentId"),
        "createdAt": thread.get("createdAt").and_then(Value::as_u64).unwrap_or(0),
        "running": id_list_contains(snapshot, "runningThreadIds", id),
        "waiting": id_list_contains(snapshot, "waitingThreadIds", id),
        "isSubagent": thread.get("isSubagent").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// 当前模型与所属供应商（**不含密钥**：只投影 id / 名称 / 协议）。
fn model_view(snapshot: &Value) -> Value {
    let config = snapshot.get("config");
    let model = config
        .and_then(|config| config.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let provider_id = str_at(snapshot, "activeProviderId");
    let provider = array_at(snapshot, "providers")
        .iter()
        .find(|provider| !provider_id.is_empty() && str_at(provider, "id") == provider_id);
    let display_name = provider
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|entry| str_at(entry, "id") == model)
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(model);
    serde_json::json!({
        "id": model,
        "name": display_name,
        "providerId": provider_id,
        "providerName": provider.map(|provider| str_at(provider, "name")).unwrap_or(""),
        "protocol": provider.map(|provider| str_at(provider, "protocol")).unwrap_or(""),
    })
}

/// 配置快照里与"这一轮怎么跑"有关的字段（不含供应商密钥）。
fn config_view(snapshot: &Value) -> Value {
    let config = snapshot.get("config");
    let text = |key: &str| {
        config
            .and_then(|config| config.get(key))
            .and_then(Value::as_str)
    };
    serde_json::json!({
        "effort": text("effort").unwrap_or("max"),
        "mode": text("mode").unwrap_or("code"),
        "contextWindow": config
            .and_then(|config| config.get("contextWindow"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "maxOutputTokens": config
            .and_then(|config| config.get("maxOutputTokens"))
            .cloned()
            .unwrap_or(Value::Null),
        "supportsImages": config
            .and_then(|config| config.get("supportsImages"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "retryCount": config
            .and_then(|config| config.get("retryCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// 工作区：agent 的当前项目 + 当前会话所属目录。
fn workspace_view(snapshot: &Value, thread: Option<&Value>) -> Value {
    let project = snapshot
        .get("workspace")
        .and_then(|workspace| workspace.get("project"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let current = thread
        .map(|thread| str_at(thread, "workspace"))
        .filter(|workspace| !workspace.is_empty())
        .unwrap_or(project);
    serde_json::json!({
        "project": project,
        "current": current,
        "publicWorkspace": str_at(snapshot, "publicWorkspace"),
    })
}

/// 计数：给插槽做"今天有多少活"这类概览，不必自己遍历 threads。
fn counts_view(snapshot: &Value, thread: Option<&Value>) -> Value {
    let thread_id = thread.map(|thread| str_at(thread, "id")).unwrap_or("");
    let queue = array_at(snapshot, "queue")
        .iter()
        .filter(|item| thread_id.is_empty() || str_at(item, "threadId") == thread_id)
        .count();
    serde_json::json!({
        "threads": array_at(snapshot, "threads").len(),
        "running": array_at(snapshot, "runningThreadIds").len(),
        "waiting": array_at(snapshot, "waitingThreadIds").len(),
        "queue": queue,
    })
}

fn id_list_contains(snapshot: &Value, key: &str, id: &str) -> bool {
    !id.is_empty()
        && array_at(snapshot, key)
            .iter()
            .any(|value| value.as_str() == Some(id))
}

/// 会话显示名：与侧栏同口径（"新会话"用首条用户消息兜底）。
fn thread_title(thread: &Value) -> String {
    title_from_items(str_at(thread, "title"), array_at(thread, "items"))
}

/// 标题解析的单一实现：原始标题是"新会话"/空时，用首条用户消息的短标题兜底。
///
/// 聊天页的 `parts` 快照只保留工具卡与助手用量（不含用户消息），所以投影时
/// 必须先用**完整 items** 把标题解析好写回，否则插件浮窗会在 2 秒后从
/// "提交代码"变成"新会话"。
fn title_from_items(raw: &str, items: &[Value]) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw == "新会话" {
        items
            .iter()
            .find(|item| str_at(item, "kind") == "user")
            .map(|item| short_title(str_at(item, "text"), 30))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "新会话".to_string())
    } else {
        raw.to_string()
    }
}

/// 插件视角的"当前会话"：显式 `thread_id` 优先，其次快照的激活会话，再其次第一条。
///
/// 显式 id 必须优先：界面可能正看着 A，而快照的 `activeThreadId` 还是 B
/// （聚焦还没生效 / 另开了一个 Tab），插槽不能按 B 算数据。
fn thread_view<'a>(snapshot: &'a Value, thread_id: Option<&str>) -> Option<&'a Value> {
    let threads = array_at(snapshot, "threads");
    let wanted = thread_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| str_at(snapshot, "activeThreadId").to_string());
    threads
        .iter()
        .find(|thread| !wanted.is_empty() && str_at(thread, "id") == wanted)
        .or_else(|| threads.first())
}

/// 任务规划（`todo` 工具）的当前计划（给插件用）。
///
/// 只投影**推演后**的结果：`set` 替换 / `update` 按 id 增量合并 / `get` 只读，
/// 都由 [`ui::derive_todo_plan`] 负责（唯一的语义实现）。插件不该、也不需要
/// 把一张张 todo 卡片自己重演一遍——那正是 原桌面端 曾经停在旧计划上的原因。
///
/// 没有规划过、当前会话不在快照里、或快照没带 items 时给 `null`：
/// 插件据此不渲染卡片，而不是画一张 `0/0` 的空面板。
fn todo_view(snapshot: &Value, thread_id: Option<&str>) -> Value {
    let Some(thread) = thread_view(snapshot, thread_id) else {
        return Value::Null;
    };
    let Some(plan) = ui::derive_todo_plan(array_at(thread, "items")) else {
        return Value::Null;
    };
    let steps = plan
        .steps
        .iter()
        .map(|step| {
            serde_json::json!({
                "id": step.id,
                "title": step.title,
                "status": match step.status {
                    ui::TodoStatus::Completed => "completed",
                    ui::TodoStatus::InProgress => "in_progress",
                    ui::TodoStatus::Pending => "pending",
                },
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "done": plan.completed(),
        "total": plan.steps.len(),
        "allDone": plan.all_done(),
        "active": plan.active_step().map(|step| step.title.clone()).unwrap_or_default(),
        "notes": plan.notes,
        "markdown": plan.to_markdown(),
        "steps": steps,
    })
}

/// 组装输入框（组件库 + 插件插槽）。
///
/// 工具行左右两侧与辅助区都是**插件插槽**：插件可以往里放自己的控件，
/// 插件挂了或坏了，插槽退回默认实现，输入框照常可用。
///
/// 十一个插槽一次性丢进阻塞线程池渲染（TSX 是同步 CPU 活），别在这里逐个 `await`。
async fn slot_content(registry: &Arc<PluginRegistry>, context: RenderContext) -> SlotContent {
    let outputs =
        crate::plugins::render_slots_off_thread(Arc::clone(registry), Slot::all(), context).await;
    let mut by_slot: std::collections::HashMap<&'static str, String> = Slot::all()
        .iter()
        .zip(outputs)
        .map(|(slot, output)| (slot.as_str(), output.html))
        .collect();
    let mut pick = |slot: Slot| by_slot.remove(slot.as_str()).unwrap_or_default();
    SlotContent {
        sidebar_top: pick(Slot::SidebarTop),
        sidebar_bottom: pick(Slot::SidebarBottom),
        topbar_right: pick(Slot::TopbarRight),
        chat_corner: pick(Slot::ChatCorner),
        composer_left: pick(Slot::ComposerLeft),
        composer_right: pick(Slot::ComposerRight),
        composer_footer: pick(Slot::ComposerFooter),
        composer_above: pick(Slot::ComposerAbove),
        thread_tray: pick(Slot::ThreadTray),
        transcript_after: pick(Slot::TranscriptAfter),
        extensions_list: pick(Slot::ExtensionsList),
        dashboard_widgets: pick(Slot::DashboardWidgets),
    }
}

fn redirect(path: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(
            header::LOCATION,
            HeaderValue::from_str(path).unwrap_or(HeaderValue::from_static("/")),
        )],
    )
        .into_response()
}

/// 用户可见的失败原因：去掉 URL 里的凭据，只留第一行。
///
/// 上游错误可能很长（甚至带网关地址），直接塞进页面既难读也容易泄露；
/// 完整错误进服务端日志，页面只给"发生了什么"。
fn public_reason(error: &anyhow::Error) -> String {
    let text = redact_tokens(&format!("{error:#}"));
    let first = text.lines().next().unwrap_or("未知错误").trim();
    truncate(first, 200)
}

/// 把 `token=…` 的值打码（URL query 是最常见的泄漏位置）。
fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("token=") {
        out.push_str(&rest[..position]);
        out.push_str("token=***");
        rest = &rest[position + "token=".len()..];
        let end = rest
            .find(|ch: char| ch == '&' || ch.is_whitespace() || ch == '"' || ch == '\'')
            .unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// 查询参数编码：只让 ASCII 安全字符原样通过，其余按 UTF-8 字节百分号编码。
fn encode_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 动作失败：回跳到会话页并用 flash 说明原因，**不**把整个界面换成裸错误页。
fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn array_at<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn running_ids(snapshot: &Value) -> Vec<String> {
    array_at(snapshot, "runningThreadIds")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// 工作区显示名：路径末段（`E:/code/app` → `app`）。
fn workspace_name(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn short_title(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    out
}

fn thread_row(value: &Value, running: &[String]) -> ThreadRow {
    let id = str_at(value, "id").to_string();
    let items = value.get("items").and_then(Value::as_array);
    let item_slice = items.map(Vec::as_slice).unwrap_or(&[]);
    let created_at = value.get("createdAt").and_then(Value::as_u64).unwrap_or(0);
    // 条目数与"最近活动"优先取网关给的 `itemCount` / `updatedAt`。
    //
    // 🔴 不能只看 `items.len()`：快照与增量帧为了压体积，只给"激活 / 打开的 Tab /
    // 运行中 / 子智能体"这几种会话带 items，其余会话的 items 是空数组
    // （`agent_rpc::state::snapshot` 的瘦身规则）。老实现拿 items 判空，于是侧栏把
    // 一整屏历史会话当成空会话丢掉——用户看到的就是"当前工作区只剩一条新会话"。
    // 老网关（没有这两个字段）仍回退到 items 计算，行为不变。
    let item_count = value
        .get("itemCount")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
        .unwrap_or(item_slice.len());
    let updated_at = value
        .get("updatedAt")
        .and_then(Value::as_u64)
        .filter(|at| *at > 0)
        .unwrap_or_else(|| {
            item_slice
                .iter()
                .filter_map(|item| item.get("at").and_then(Value::as_u64))
                .max()
                .unwrap_or(0)
        })
        // 空会话没有卡片：回退创建时刻，否则它会在侧栏显示成 1970 年、"9 分钟前"也排到最后。
        .max(created_at);
    let raw_title = str_at(value, "title").trim();
    let title = if raw_title.is_empty() || raw_title == "新会话" {
        item_slice
            .iter()
            .find(|item| str_at(item, "kind") == "user")
            .map(|item| short_title(str_at(item, "text"), 30))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "新会话".to_string())
    } else {
        raw_title.to_string()
    };
    ThreadRow {
        running: running.iter().any(|r| r == &id),
        id,
        title,
        workspace: str_at(value, "workspace").to_string(),
        mode: {
            let mode = str_at(value, "mode");
            if mode.is_empty() {
                "code".to_string()
            } else {
                mode.to_string()
            }
        },
        item_count,
        created_at,
        updated_at,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n…（已截断）");
    out
}

/// 当前模型：优先取运行中快照的 `config.model`，回退到 `/api/config` 的 saved。
pub async fn not_found(State(state): State<AppState>) -> Response {
    let page = ErrorPage {
        base: base(&state, "页面不存在", "", None).await,
        message: "这个地址没有对应的页面。".to_string(),
    };
    match page.render() {
        Ok(html) => (StatusCode::NOT_FOUND, Html(html)).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("渲染错误页失败: {error}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `runs` 投影只认 `run_command`：后台任务走 `tasks`，两者不能混。
    ///
    /// 顺带守两条口径：① 最新的排最前；② `rawArgs` 是 JSON 字符串时也要解析出
    /// `command`——模型偶尔把参数当字符串吐，界面上早就是这么兜的。
    #[test]
    fn runs_view_projects_run_command_items_newest_first() {
        let thread = serde_json::json!({
            "id": "t1",
            "items": [
                {"id":"i1","kind":"tool","name":"run_command","status":"done",
                 "args":{"command":"echo a"},"output":"a\n","durationMs":820,
                 "details":{"exitCode":0}},
                {"id":"i2","kind":"assistant","text":"ok"},
                {"id":"i3","kind":"tool","name":"run_background","status":"done",
                 "args":{"command":"npm run dev"}},
                {"id":"i4","kind":"tool","name":"run_command","status":"done",
                 "rawArgs":"{\"command\":\"cargo build\"}","output":"x",
                 "details":{"exit_code":101}}
            ]
        });
        let view = runs_view(Some(&thread));
        let runs = view.get("runs").and_then(Value::as_array).expect("runs 数组");
        assert_eq!(runs.len(), 2, "只投影 run_command：{view}");
        assert_eq!(runs[0]["id"], "i4", "最新的在最前：{view}");
        assert_eq!(runs[0]["command"], "cargo build", "rawArgs 要能解析：{view}");
        assert_eq!(runs[0]["exitCode"], 101, "退出码只认结构化字段：{view}");
        assert_eq!(runs[1]["id"], "i1");
        assert_eq!(runs[1]["durationMs"], 820);
        assert_eq!(runs[1]["exitCode"], 0);
        assert_eq!(view["count"], 2);
    }

    /// 超过上限只留最近 20 条，但 `total` 如实报出被截掉的规模。
    #[test]
    fn runs_view_caps_history_and_keeps_total_honest() {
        let items: Vec<Value> = (0..25)
            .map(|index| {
                serde_json::json!({
                    "id": format!("i{index}"),
                    "kind": "tool",
                    "name": "run_command",
                    "status": "done",
                    "args": { "command": format!("echo {index}") }
                })
            })
            .collect();
        let thread = serde_json::json!({ "id": "t1", "items": items });
        let view = runs_view(Some(&thread));
        let runs = view.get("runs").and_then(Value::as_array).expect("runs 数组");
        assert_eq!(runs.len(), 20, "面板最多回看 20 条：{view}");
        assert_eq!(runs[0]["command"], "echo 24", "留下的必须是最新的：{view}");
        assert_eq!(view["total"], 25, "total 要如实报规模：{view}");
    }

    /// 没有会话 / 还没跑过命令 = 空列表，不是"读取失败"（不塞 `error`）。
    #[test]
    fn runs_view_without_thread_is_empty_not_an_error() {
        let view = runs_view(None);
        assert_eq!(view["count"], 0);
        assert!(view.get("error").is_none(), "没有命令不是读取失败：{view}");
    }

    #[test]
    fn plugin_referer_rewrites_provider_redirect_back_to_plugin_page() {
        assert_eq!(
            plugin_redirect_target(
                Some("http://127.0.0.1:8788/ext/provider-admin/providers?provider=p1"),
                "/providers?provider=p1&tab=models&notice=model_added"
            ),
            Some(
                "/ext/provider-admin/providers?provider=p1&tab=models&notice=model_added"
                    .to_string()
            )
        );
        assert_eq!(
            plugin_redirect_target(Some("http://127.0.0.1:8788/"), "/providers"),
            None
        );
        assert_eq!(
            plugin_redirect_target(
                Some("http://127.0.0.1:8788/ext/evil/../admin"),
                "/providers"
            ),
            None
        );
    }

    #[test]
    fn public_reason_redacts_tokens_and_keeps_only_the_first_line() {
        let error =
            anyhow::anyhow!("连接网关失败: ws://127.0.0.1:1/rpc?workspace=w&token=SECRET\n第二行");
        let reason = public_reason(&error);
        assert!(!reason.contains("SECRET"));
        assert!(!reason.contains("第二行"));
        assert!(reason.contains("token=***"));
    }

    #[test]
    fn encode_query_percent_encodes_utf8_and_reserved_chars() {
        assert_eq!(encode_query("a b&c=中"), "a+b%26c%3D%E4%B8%AD");
    }

    /// 收集 `src/` 里所有 `notice=<key>` 字面量（含 format 字符串与测试断言里的）。
    /// 动态拼接（`notice={}`）抓不到 key、天然跳过。
    fn notice_keys_in_source() -> Vec<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys: Vec<String> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let mut rest = source.as_str();
                while let Some(pos) = rest.find("notice=") {
                    rest = &rest[pos + "notice=".len()..];
                    let key: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                        .collect();
                    if !key.is_empty() {
                        keys.push(key);
                    }
                }
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    #[test]
    fn notice_literals_are_translated() {
        let keys = notice_keys_in_source();
        assert!(
            !keys.is_empty(),
            "源码扫描应能找到 notice= 字面量，扫描器本身坏了"
        );
        for key in keys {
            let exact = NOTICE_TEXTS.iter().any(|(k, _)| *k == key);
            let dynamic_prefix = NOTICE_TEXTS.iter().any(|(k, _)| k.starts_with(key.as_str()));
            assert!(
                exact || dynamic_prefix,
                "notice key `{key}` 出现在源码里但没有登记中文文案（NOTICE_TEXTS），\
                 横幅会把它原样显示给用户"
            );
        }
    }

    #[test]
    fn wants_partial_reads_accept_json_only() {
        let mut headers = HeaderMap::new();
        assert!(!wants_partial(&headers), "没有 Accept 时按整页跳转处理");
        headers.insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        assert!(!wants_partial(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(wants_partial(&headers));
    }

    #[test]
    fn notice_text_covers_the_three_keys_added_with_this_table() {
        // 这三个 key 曾以英文裸串形式上过线（建议 04），锁一下防止回归。
        assert_eq!(notice_text("thread_renamed").as_deref(), Some("已重命名会话"));
        assert_eq!(notice_text("thread_deleted").as_deref(), Some("已删除会话"));
        assert_eq!(notice_text("no_such_notice"), None);
    }

    fn snapshot(threads: serde_json::Value, window: u64) -> Value {
        serde_json::json!({
            "activeThreadId": "t1",
            "threads": threads,
            "config": { "model": "deepseek-chat", "contextWindow": window },
        })
    }

    fn assistant(prompt: u64) -> Value {
        serde_json::json!({
            "kind": "assistant",
            "usage": { "promptTokens": prompt, "completionTokens": 10, "totalTokens": prompt + 10 }
        })
    }

    fn thread(id: &str, items: Vec<Value>) -> Value {
        serde_json::json!({ "id": id, "title": format!("会话 {id}"), "items": items })
    }

    #[test]
    fn token_usage_is_prompt_tokens_over_context_window() {
        let thread = thread("t1", vec![assistant(64_000)]);
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 128_000), None);
        assert_eq!(view["used"], 64_000);
        assert_eq!(view["window"], 128_000);
        assert_eq!(view["percent"], 50);
        assert_eq!(view["threadTitle"], "会话 t1");
        assert_eq!(view["model"], "deepseek-chat");
    }

    #[test]
    fn token_usage_uses_the_latest_assistant_usage_and_skips_streaming_cards() {
        // 最后一张助手卡还在流式（没有 usage），不能用它把用量清零。
        let thread = thread(
            "t1",
            vec![
                assistant(1_000),
                serde_json::json!({ "kind": "user", "text": "再来" }),
                serde_json::json!({ "kind": "assistant", "text": "（正在生成）" }),
            ],
        );
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 10_000), None);
        assert_eq!(view["used"], 1_000);
        assert_eq!(view["percent"], 10);
    }

    #[test]
    fn token_usage_title_falls_back_to_first_user_message() {
        // 会话头还叫"新会话"、界面标题已经由首条用户消息派生时，
        // 插件浮窗/完整页面应显示同一个名字，而不是"新会话"。
        let thread = serde_json::json!({
            "id": "t1",
            "title": "新会话",
            "items": [
                { "kind": "user", "text": "提交后端代码并验证构建" },
                assistant(1_000),
            ],
        });
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 100_000), Some("t1"));
        assert_eq!(view["threadTitle"], "提交后端代码并验证构建");
    }

    #[test]
    fn token_usage_rounds_and_clamps_to_100() {
        let round = thread("t1", vec![assistant(1)]);
        assert_eq!(
            token_usage_view(&snapshot(serde_json::json!([round]), 3), None)["percent"],
            33
        );
        let over = thread("t1", vec![assistant(999_999)]);
        assert_eq!(
            token_usage_view(&snapshot(serde_json::json!([over]), 128_000), None)["percent"],
            100
        );
    }

    #[test]
    fn token_usage_without_window_is_null_instead_of_zero_percent() {
        // 窗口未配置：`0%` 会假装"很空"，必须给 null 让插件显示"无数据"。
        let thread = thread("t1", vec![assistant(5_000)]);
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 0), None);
        assert_eq!(view["used"], 5_000);
        assert!(view["percent"].is_null(), "{view}");
    }

    #[test]
    fn token_usage_follows_the_active_thread_then_falls_back_to_the_first() {
        let threads = serde_json::json!([
            thread("t1", vec![assistant(1_000)]),
            thread("t2", vec![assistant(90_000)]),
        ]);
        let mut snap = snapshot(threads.clone(), 100_000);
        snap["activeThreadId"] = serde_json::json!("t2");
        assert_eq!(token_usage_view(&snap, None)["percent"], 90);
        assert_eq!(token_usage_view(&snap, Some("t1"))["percent"], 1);

        // 没有任何活动会话时用第一条，而不是让页面变成"无数据"。
        snap["activeThreadId"] = serde_json::json!("");
        assert_eq!(token_usage_view(&snap, None)["percent"], 1);
    }

    #[test]
    fn token_usage_of_an_empty_gateway_snapshot_is_all_zero() {
        let view = token_usage_view(&Value::Null, None);
        assert_eq!(view["used"], 0);
        assert_eq!(view["window"], 0);
        assert!(view["percent"].is_null());
        assert_eq!(view["threadId"], "");
    }

    #[test]
    fn token_usage_falls_back_to_the_last_reported_cache() {
        // 模型并非每轮都报 cachedTokens：本轮没报时沿用上一条真实上报，并标记 stale。
        let thread = thread(
            "t1",
            vec![
                serde_json::json!({
                    "kind": "assistant",
                    "usage": { "promptTokens": 10000, "completionTokens": 10, "cachedTokens": 9000 }
                }),
                serde_json::json!({ "kind": "user", "text": "再来" }),
                serde_json::json!({
                    "kind": "assistant",
                    "usage": { "promptTokens": 20000, "completionTokens": 10 }
                }),
            ],
        );
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 100_000), None);
        assert_eq!(view["used"], 20000);
        assert_eq!(view["cached"], 9000);
        assert_eq!(view["cachePercent"], 90);
        assert_eq!(view["cacheRate"], 90.0);
        assert_eq!(view["cacheStale"], true, "{view}");
    }

    #[test]
    fn token_usage_cache_is_not_stale_when_the_latest_turn_reports_it() {
        let thread = thread(
            "t1",
            vec![serde_json::json!({
                "kind": "assistant",
                "usage": { "promptTokens": 10000, "completionTokens": 10, "cachedTokens": 8500 }
            })],
        );
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 100_000), None);
        assert_eq!(view["cachePercent"], 85);
        assert_eq!(view["cacheStale"], false, "{view}");
    }

    fn todo_item(action: &str, items: Value) -> Value {
        serde_json::json!({
            "kind": "tool",
            "name": "todo",
            "rawArgs": serde_json::json!({ "action": action, "items": items }).to_string(),
        })
    }

    #[test]
    fn plugin_data_exposes_thread_model_config_counts_and_cache() {
        let thread = serde_json::json!({
            "id": "t1",
            "title": "提交代码",
            "workspace": "E:/proj",
            "mode": "code",
            "agentId": "ada-coding",
            "createdAt": 1234,
            "items": [
                {
                    "kind": "assistant",
                    "durationMs": 2000,
                    "usage": {
                        "promptTokens": 10000,
                        "completionTokens": 500,
                        "totalTokens": 10500,
                        "cachedTokens": 8500
                    }
                },
                { "kind": "tool", "status": "waiting" },
                { "kind": "tool", "status": "done" }
            ]
        });
        let snapshot = serde_json::json!({
            "activeThreadId": "t1",
            "runningThreadIds": ["t1"],
            "waitingThreadIds": [],
            "queue": [{ "threadId": "t1" }, { "threadId": "t2" }],
            "threads": [thread],
            "config": {
                "model": "m1",
                "effort": "high",
                "mode": "code",
                "contextWindow": 100000,
                "maxOutputTokens": 8000,
                "supportsImages": true,
                "retryCount": 5
            },
            "activeProviderId": "p1",
            "providers": [{
                "id": "p1",
                "name": "供应商",
                "protocol": "openai_chat",
                "apiKey": "SECRET-TOKEN",
                "models": [{ "id": "m1", "name": "DeepSeek V4.1" }]
            }],
            "workspace": { "project": "E:/agent" },
            "publicWorkspace": "E:/pub"
        });
        let data = plugin_data(&snapshot, Some("t1"), 3);
        assert_eq!(data["plugin_count"], 3);
        assert_eq!(data["tokens"]["cachePercent"], 85, "{data}");
        assert_eq!(data["tokens"]["cacheRate"], 85.0, "{data}");
        assert_eq!(data["tokens"]["cached"], 8500);
        assert_eq!(data["tokens"]["completion"], 500);
        assert_eq!(data["tokens"]["total"], 10500);
        assert_eq!(data["tokens"]["tokensPerSec"], 250);
        assert_eq!(data["tokens"]["durationMs"], 2000);
        assert_eq!(data["tokens"]["available"], true);
        assert_eq!(data["thread"]["title"], "提交代码");
        assert_eq!(data["thread"]["running"], true);
        assert_eq!(data["thread"]["waiting"], false);
        assert_eq!(data["model"]["name"], "DeepSeek V4.1");
        assert_eq!(data["model"]["providerName"], "供应商");
        assert_eq!(data["config"]["effort"], "high");
        assert_eq!(data["config"]["maxOutputTokens"], 8000);
        assert_eq!(data["counts"]["queue"], 1, "只数当前会话的排队");
        assert_eq!(data["workspace"]["current"], "E:/proj");
        assert!(data["render"]["at"].as_u64().unwrap_or(0) > 0);
        // 数据投影绝不能带供应商密钥。
        assert!(!data.to_string().contains("SECRET-TOKEN"), "{data}");
    }

    #[test]
    fn plugin_data_without_a_snapshot_is_null_instead_of_empty_strings() {
        let data = plugin_data(&Value::Null, None, 2);
        assert!(data["thread"].is_null(), "{data}");
        assert!(data["todo"].is_null(), "{data}");
        assert_eq!(data["tokens"]["available"], false);
        assert_eq!(data["counts"]["threads"], 0);
        assert_eq!(data["plugin_count"], 2);
    }

    #[test]
    fn todo_view_projects_the_derived_plan() {
        // 插件只消费推演后的结果：set 建基线、update 按 id 合并，都该体现在投影里。
        let thread = thread(
            "t1",
            vec![
                todo_item(
                    "set",
                    serde_json::json!([
                        { "id": "t1", "title": "读列表", "status": "completed" },
                        { "id": "t2", "title": "写条目", "status": "in_progress" },
                        { "id": "t3", "title": "验证", "status": "pending" }
                    ]),
                ),
                todo_item(
                    "update",
                    serde_json::json!([{ "id": "t2", "title": "写条目", "status": "completed" }]),
                ),
            ],
        );
        let view = todo_view(&snapshot(serde_json::json!([thread]), 100_000), Some("t1"));
        assert_eq!(view["done"], 2, "{view}");
        assert_eq!(view["total"], 3, "{view}");
        assert_eq!(view["allDone"], false, "{view}");
        assert_eq!(view["active"], "验证", "{view}");
        assert_eq!(view["steps"][1]["status"], "completed", "{view}");
        assert!(
            view["markdown"]
                .as_str()
                .unwrap_or("")
                .contains("- [x] 写条目"),
            "{view}"
        );
    }

    #[test]
    fn todo_view_is_null_without_a_plan() {
        let thread = thread(
            "t1",
            vec![serde_json::json!({ "kind": "assistant", "text": "hi" })],
        );
        assert!(todo_view(&snapshot(serde_json::json!([thread]), 100_000), None).is_null());
        assert!(todo_view(&Value::Null, None).is_null());
    }

    #[test]
    fn todo_view_follows_the_requested_thread() {
        let first = thread(
            "t1",
            vec![todo_item(
                "set",
                serde_json::json!([{ "id": "a", "title": "A", "done": true }]),
            )],
        );
        let second = thread(
            "t2",
            vec![todo_item(
                "set",
                serde_json::json!([{ "id": "b", "title": "B", "done": false }]),
            )],
        );
        let snap = snapshot(serde_json::json!([first, second]), 100_000);
        // 显式 thread_id 优先（界面看着 B、快照激活的却可能还是 A）。
        assert_eq!(todo_view(&snap, Some("t2"))["steps"][0]["title"], "B");
        // 不传时跟 activeThreadId（本夹具是 t1）。
        assert_eq!(todo_view(&snap, None)["steps"][0]["title"], "A");
    }
}

mod bridge;
mod chat;
mod dashboard;
mod extensions;
mod plugin_route;
mod providers;
mod tasks;
mod workspaces;

pub use bridge::*;
pub use chat::*;
pub use dashboard::*;
pub use extensions::*;
pub use plugin_route::*;
pub use providers::*;
pub use tasks::*;
pub use workspaces::*;
