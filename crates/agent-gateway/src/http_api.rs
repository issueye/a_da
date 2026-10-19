//! 网关的 **HTTP API**（Phase 0/1/2）。
//!
//! 现有线协议是 JSON-RPC over WebSocket；本模块把其中 **unary（一问一答）**
//! 的方法暴露成 HTTP，事件流仍然走原来的 WS。这样浏览器/外部脚本可以用普通
//! HTTP 客户端读配置、查状态、改配置，而不必先实现一套 WS 帧处理。
//!
//! # 为什么走内部 WS
//!
//! 网关不含引擎（INV-1），也不缓存会话状态（INV-8）。HTTP handler 只能通过
//! 已有的节点 RPC 面把请求转给 agent。每次请求开一条短连接（带 `?api=1`）：
//! agent 侧看到 `api=1` 时**不注册进事件广播**、不推种子快照、成功请求后也不
//! 自动广播——读接口不会把 UI 客户端叫醒；写接口的状态更新仍由 dispatch 臂
//! 里的 `broadcast_immediate()` 发出，事件通道没有被拆成两套。
//!
//! # 路径与参数
//!
//! 资源 id 走路径段；工作区与文件路径走 query（Windows 路径里有 `\`）。
//! 写接口的 body 基本就是该 JSON-RPC 方法的 `params`（个别接口兼容
//! `{config:...}` / `{provider:...}` 包装）。
//!
//! # 鉴权
//!
//! `Authorization: Bearer <token>`（兼容旧的 `?token=`），Origin 走
//! [`crate::auth::AuthConfig::check_origin`]，工作区走 token 作用域。
//! 未认证连接只允许 `GET /api/gateway/info`。

use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::warn;

use agent_proto::*;

use crate::auth::{query_param, AuthError, AuthOutcome, TokenScope};
use crate::registry::AgentInstance;
use crate::relay::{AppState, Gateway};
use crate::supervisor::{ensure_agent, SpawnSpec};

const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .fallback(api_dispatch)
        .layer(middleware::from_fn_with_state(state, api_cors))
}

// ── 路由表 ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum QKind {
    Str,
    Bool,
    U64,
}

#[derive(Clone, Copy)]
struct QSpec {
    name: &'static str,
    key: &'static str,
    kind: QKind,
    required: bool,
}

#[derive(Clone, Copy)]
enum BodyMode {
    None,
    /// body 可选；给了就必须是对象（`thread.create` 不带标题、provider 激活不指定模型）
    MergeOptional,
    /// body 必须是对象
    Merge,
    WrapOrMerge(&'static str),
}

#[derive(Clone, Copy)]
enum Target {
    Agent(&'static str),
    Gateway(GatewayOp),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GatewayOp {
    Info,
    Status,
    ListAgents,
    Attach,
    Detach,
    FsRoots,
    FsList,
    FsMkdir,
    WorkspacesList,
    WorkspacesRemove,
    CancelDelegation,
}

struct RouteSpec {
    http: &'static str,
    path: &'static str,
    target: Target,
    path_map: &'static [(&'static str, &'static str)],
    query: &'static [QSpec],
    body: BodyMode,
    with_workspace: bool,
    etag: bool,
    public: bool,
}

const fn q(name: &'static str, key: &'static str, kind: QKind, required: bool) -> QSpec {
    QSpec {
        name,
        key,
        kind,
        required,
    }
}

macro_rules! route {
    ($http:expr, $path:expr, $target:expr, [$($pm:expr),*], [$($qs:expr),*], $body:expr, $with_ws:expr, $etag:expr, $public:expr) => {
        RouteSpec {
            http: $http,
            path: $path,
            target: $target,
            path_map: &[$($pm),*],
            query: &[$($qs),*],
            body: $body,
            with_workspace: $with_ws,
            etag: $etag,
            public: $public,
        }
    };
}

static ROUTES: &[RouteSpec] = &[
    // ── Phase 1：纯查询 GET ──────────────────────────────────────────────
    route!(
        "GET",
        "/snapshot",
        Target::Agent(SESSION_SNAPSHOT),
        [],
        [],
        BodyMode::None,
        false,
        true,
        false
    ),
    route!(
        "GET",
        "/threads/:thread_id/items",
        Target::Agent(THREAD_ITEMS_PAGE),
        [("thread_id", "threadId")],
        [
            q("from", "from", QKind::U64, false),
            q("count", "count", QKind::U64, false)
        ],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/workspaces/sessions",
        Target::Agent(WORKSPACE_ENTRIES),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "GET",
        "/config",
        Target::Agent(CONFIG_GET),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/config/presets",
        Target::Agent(CONFIG_PRESETS),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/providers",
        Target::Agent(PROVIDER_LIST),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/fs/roots",
        Target::Agent(FS_ROOTS),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/fs/list",
        Target::Agent(FS_LIST),
        [],
        [
            q("path", "path", QKind::Str, true),
            q("showHidden", "showHidden", QKind::Bool, false),
            q("limit", "limit", QKind::U64, false)
        ],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/fs/raw",
        Target::Agent(FS_READ_BASE64),
        [],
        [q("path", "path", QKind::Str, true)],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/threads/:thread_id/changes/count",
        Target::Agent(CHANGE_COUNT),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/threads/:thread_id/changes",
        Target::Agent(CHANGE_LIST),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/plugins",
        Target::Agent(PLUGIN_LIST),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "GET",
        "/plugins/builtin",
        Target::Agent(PLUGIN_BUILTIN_CATALOG),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/prompts",
        Target::Agent(PROMPT_LIST),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "GET",
        "/skills",
        Target::Agent(SKILL_LIST),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "GET",
        "/subagent-profiles",
        Target::Agent(SUBAGENT_PROFILE_LIST),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "GET",
        "/debug/host",
        Target::Agent(DEBUG_HOST_INFO),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/stats/prompt-chars",
        Target::Agent(STATS_PROMPT_CHARS),
        [],
        [],
        BodyMode::MergeOptional,
        false,
        false,
        false
    ),
    // ── Phase 2：unary 写 ────────────────────────────────────────────────
    route!(
        "PUT",
        "/config/provider",
        Target::Agent(CONFIG_SET_PROVIDER),
        [],
        [],
        BodyMode::WrapOrMerge("config"),
        false,
        false,
        false
    ),
    route!(
        "PUT",
        "/config/approval",
        Target::Agent(CONFIG_SET_APPROVAL),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PUT",
        "/config/effort",
        Target::Agent(CONFIG_SET_EFFORT),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PUT",
        "/config/retry-count",
        Target::Agent(CONFIG_SET_RETRY_COUNT),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PUT",
        "/providers/:provider_id",
        Target::Agent(PROVIDER_SAVE),
        [("provider_id", "provider.id")],
        [],
        BodyMode::WrapOrMerge("provider"),
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/providers/:provider_id",
        Target::Agent(PROVIDER_DELETE),
        [("provider_id", "id")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/providers/:provider_id/activate",
        Target::Agent(PROVIDER_SET_ACTIVE),
        [("provider_id", "id")],
        [],
        BodyMode::MergeOptional,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/workspaces",
        Target::Agent(WORKSPACE_ADD),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/workspaces",
        Target::Agent(WORKSPACE_REMOVE),
        [],
        [q("workspace", "workspace", QKind::Str, true)],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/workspaces/public/open",
        Target::Agent(WORKSPACE_OPEN_PUBLIC),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/fs/mkdir",
        Target::Agent(FS_MKDIR),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads",
        Target::Agent(THREAD_CREATE),
        [],
        [],
        BodyMode::MergeOptional,
        true,
        false,
        false
    ),
    route!(
        "DELETE",
        "/threads/:thread_id",
        Target::Agent(THREAD_DELETE),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/threads/:thread_id",
        Target::Agent(THREAD_UPDATE),
        [("thread_id", "threadId")],
        [],
        BodyMode::MergeOptional,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/focus",
        Target::Agent(THREAD_FOCUS),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/threads/:thread_id/mode",
        Target::Agent(THREAD_SET_MODE),
        [("thread_id", "threadId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/threads/:thread_id/workspace",
        Target::Agent(THREAD_SET_WORKSPACE),
        [("thread_id", "threadId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/threads/:thread_id/queue",
        Target::Agent(QUEUE_CLEAR),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/queue/:index/promote",
        Target::Agent(QUEUE_PROMOTE),
        [("thread_id", "threadId"), ("index", "index")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/threads/:thread_id/queue/:index",
        Target::Agent(QUEUE_REMOVE),
        [("thread_id", "threadId"), ("index", "index")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/approvals/:call_id/decision",
        Target::Agent(APPROVAL_DECIDE),
        [("call_id", "toolItemId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/questions/:call_id/answer",
        Target::Agent(QUESTION_ANSWER),
        [("call_id", "callId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/changes/revert-card",
        Target::Agent(CHANGE_REVERT_CARD),
        [("thread_id", "threadId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/changes/revert-checkpoint",
        Target::Agent(CHANGE_REVERT_CHECKPOINT),
        [("thread_id", "threadId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/changes/revert-file",
        Target::Agent(CHANGE_REVERT_FILE),
        [("thread_id", "threadId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/threads/:thread_id/changes/revert-all",
        Target::Agent(CHANGE_REVERT_ALL),
        [("thread_id", "threadId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/plugins/:plugin_id/capabilities",
        Target::Agent(PLUGIN_CAPABILITIES_SET),
        [("plugin_id", "pluginId")],
        [],
        BodyMode::WrapOrMerge("patch"),
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/plugins/:plugin_id/config",
        Target::Agent(PLUGIN_CONFIG_SET),
        [("plugin_id", "pluginId")],
        [],
        BodyMode::WrapOrMerge("values"),
        false,
        false,
        false
    ),
    route!(
        "PUT",
        "/plugins/:plugin_id/secret",
        Target::Agent(PLUGIN_SECRET_SET),
        [("plugin_id", "pluginId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/plugins/:plugin_id/enabled",
        Target::Agent(PLUGIN_SET_ENABLED),
        [("plugin_id", "pluginId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/plugins",
        Target::Agent(PLUGIN_DELETE),
        [],
        [
            q("filePath", "filePath", QKind::Str, true),
            q("workspace", "workspace", QKind::Str, false)
        ],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "POST",
        "/plugins/template",
        Target::Agent(PLUGIN_CREATE_TEMPLATE),
        [],
        [],
        BodyMode::Merge,
        true,
        false,
        false
    ),
    route!(
        "POST",
        "/prompts",
        Target::Agent(PROMPT_CREATE),
        [],
        [],
        BodyMode::WrapOrMerge("options"),
        true,
        false,
        false
    ),
    route!(
        "PATCH",
        "/prompts",
        Target::Agent(PROMPT_UPDATE),
        [],
        [],
        BodyMode::WrapOrMerge("item"),
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/prompts",
        Target::Agent(PROMPT_DELETE),
        [],
        [q("filePath", "filePath", QKind::Str, true)],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/prompts/enabled",
        Target::Agent(PROMPT_SET_ENABLED),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "PATCH",
        "/skills/:skill_id/enabled",
        Target::Agent(SKILL_SET_ENABLED),
        [("skill_id", "id")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/skills",
        Target::Agent(SKILL_CREATE),
        [],
        [],
        BodyMode::Merge,
        true,
        false,
        false
    ),
    route!(
        "DELETE",
        "/skills/:skill_id",
        Target::Agent(SKILL_DELETE),
        [("skill_id", "id")],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "PATCH",
        "/subagent-profiles/:profile_id/enabled",
        Target::Agent(SUBAGENT_PROFILE_SET_ENABLED),
        [("profile_id", "profileId")],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/subagent-profiles/:profile_id",
        Target::Agent(SUBAGENT_PROFILE_DELETE),
        [("profile_id", "profileId")],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::None,
        true,
        false,
        false
    ),
    route!(
        "DELETE",
        "/debug/log",
        Target::Agent(DEBUG_LOG_CLEAR),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    // ── 网关管理面 ──────────────────────────────────────────────────────
    route!(
        "GET",
        "/gateway/info",
        Target::Gateway(GatewayOp::Info),
        [],
        [],
        BodyMode::None,
        false,
        false,
        true
    ),
    route!(
        "GET",
        "/gateway/status",
        Target::Gateway(GatewayOp::Status),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/gateway/agents",
        Target::Gateway(GatewayOp::ListAgents),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/gateway/attach",
        Target::Gateway(GatewayOp::Attach),
        [],
        [q("workspace", "workspace", QKind::Str, false)],
        BodyMode::MergeOptional,
        true,
        false,
        false
    ),
    route!(
        "DELETE",
        "/gateway/agents/:agent_id",
        Target::Gateway(GatewayOp::Detach),
        [("agent_id", "agentId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/gateway/fs/roots",
        Target::Gateway(GatewayOp::FsRoots),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/gateway/fs/list",
        Target::Gateway(GatewayOp::FsList),
        [],
        [
            q("path", "path", QKind::Str, false),
            q("directoriesOnly", "directoriesOnly", QKind::Bool, false),
            q("showHidden", "showHidden", QKind::Bool, false),
            q("limit", "limit", QKind::U64, false)
        ],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/gateway/fs/mkdir",
        Target::Gateway(GatewayOp::FsMkdir),
        [],
        [],
        BodyMode::Merge,
        false,
        false,
        false
    ),
    route!(
        "GET",
        "/gateway/workspaces",
        Target::Gateway(GatewayOp::WorkspacesList),
        [],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "DELETE",
        "/gateway/workspaces",
        Target::Gateway(GatewayOp::WorkspacesRemove),
        [],
        [q("workspace", "workspace", QKind::Str, true)],
        BodyMode::None,
        false,
        false,
        false
    ),
    route!(
        "POST",
        "/gateway/delegations/:delegation_id/cancel",
        Target::Gateway(GatewayOp::CancelDelegation),
        [("delegation_id", "delegationId")],
        [],
        BodyMode::None,
        false,
        false,
        false
    ),
];

// ── 请求分发 ──────────────────────────────────────────────────────────────

async fn api_dispatch(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();

    let Some(spec) = ROUTES
        .iter()
        .find(|r| r.http == method && match_path(r.path, &path).is_some())
    else {
        return ApiError::not_found(format!("未知的 /api 路径：{method} {path}")).into_response();
    };
    let captures = match_path(spec.path, &path).unwrap_or_default();

    // 先鉴权再读 body：未认证请求不该有机会让我们读 4 MiB 再拒绝。
    let auth = if spec.public {
        None
    } else {
        let need_agent = matches!(spec.target, Target::Agent(_));
        // `workspace.entries` / `workspace.remove` 的 query.workspace 是**操作对象**，
        // 不是“把网关绑到哪个 agent”。绑定目标仍优先用默认工作区，避免为了列一下
        // 目录/删一个工作区就额外拉起那个工作区的 agent。
        let bind_workspace = match spec.target {
            Target::Agent(method) if method == WORKSPACE_ENTRIES || method == WORKSPACE_REMOVE => {
                false
            }
            _ => true,
        };
        // 网关全局管理面（status / listAgents / fs.*）不该被“某个工作区作用域”
        // 卡住；只有 attach / workspaces.remove 需要检查目标工作区。
        let scope_required = match spec.target {
            Target::Agent(_) => true,
            Target::Gateway(op) => {
                matches!(op, GatewayOp::Attach | GatewayOp::WorkspacesRemove)
            }
        };
        match authenticate(&state, &parts, need_agent, bind_workspace, scope_required).await {
            Ok(a) => Some(a),
            Err(e) => return e.into_response(),
        }
    };
    let workspace = auth
        .as_ref()
        .map(|a| a.workspace.clone())
        .unwrap_or_default();

    let body_bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => return ApiError::bad_request(format!("读取请求体失败：{e}")).into_response(),
    };
    let body_json = if body_bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice::<Value>(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                return ApiError::bad_request(format!("请求体不是合法 JSON：{e}")).into_response()
            }
        }
    };

    let params = match build_params(spec, &captures, &parts.uri, body_json, &workspace) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    if let Some(auth) = &auth {
        if let Some(ws) = params.get("workspace").and_then(|v| v.as_str()) {
            if !auth.scope.allows(ws) {
                return ApiError::forbidden(format!("token 不允许访问工作区：{ws}"))
                    .into_response();
            }
        }
    }

    let result = match spec.target {
        Target::Agent(rpc) => {
            let Some(instance) = auth.as_ref().and_then(|a| a.instance.as_ref()) else {
                return ApiError::internal("内部错误：agent 实例未初始化").into_response();
            };
            call_agent(instance, rpc, params).await
        }
        Target::Gateway(op) => call_gateway(&state.gateway, op, &params).await,
    };

    match result {
        Ok(value) => success_response(spec, value, &parts.headers, &workspace),
        Err(e) => e.into_response(),
    }
}

fn success_response(
    spec: &RouteSpec,
    value: Value,
    req_headers: &HeaderMap,
    workspace: &str,
) -> Response {
    let mut resp = if value.is_null() {
        Json(json!({ "ok": true })).into_response()
    } else {
        Json(value.clone()).into_response()
    };

    if spec.etag {
        if let Some(seq) = value.get("seq").and_then(|v| v.as_u64()) {
            let etag = format!("\"snap-{seq}\"");
            let matched = req_headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .map(|raw| raw.split(',').any(|part| part.trim() == etag))
                .unwrap_or(false);
            if matched {
                let mut not_modified = StatusCode::NOT_MODIFIED.into_response();
                if let Ok(hv) = HeaderValue::from_str(&etag) {
                    not_modified.headers_mut().insert(header::ETAG, hv);
                }
                return not_modified;
            }
            if let Ok(hv) = HeaderValue::from_str(&etag) {
                resp.headers_mut().insert(header::ETAG, hv);
            }
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
    }

    if !workspace.is_empty() {
        if let Ok(hv) = HeaderValue::from_str(workspace) {
            resp.headers_mut().insert("x-a-da-workspace", hv);
        }
    }
    resp
}

// ── 鉴权 ──────────────────────────────────────────────────────────────────

struct AuthCtx {
    scope: TokenScope,
    workspace: String,
    instance: Option<AgentInstance>,
}

async fn authenticate(
    state: &AppState,
    parts: &axum::http::request::Parts,
    need_agent: bool,
    bind_workspace: bool,
    scope_required: bool,
) -> Result<AuthCtx, ApiError> {
    let gateway = &state.gateway;
    let origin = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    gateway
        .auth
        .check_origin(origin)
        .map_err(ApiError::from_auth)?;

    let token =
        bearer_token(&parts.headers).or_else(|| query_param(&parts.uri.to_string(), "token"));
    let scope = match gateway
        .auth
        .authorize(token.as_deref())
        .map_err(ApiError::from_auth)?
    {
        AuthOutcome::Anonymous => {
            return Err(ApiError::unauthorized(
                "需要 token：Authorization: Bearer <token>（或 ?token=）",
            ));
        }
        AuthOutcome::Scoped(scope) => scope,
    };

    let requested =
        workspace_from_request(parts).unwrap_or_else(|| state.default_workspace.clone());
    if scope_required && !scope.allows(&requested) {
        return Err(ApiError::forbidden(format!(
            "token 不允许访问工作区：{requested}"
        )));
    }

    // 默认优先默认工作区；token 作用域不允许默认值时，退回请求里的工作区。
    let workspace = if bind_workspace {
        requested.clone()
    } else if scope.allows(&state.default_workspace) {
        state.default_workspace.clone()
    } else {
        requested.clone()
    };

    let instance = if need_agent {
        gateway
            .workspaces
            .record(&workspace, Some(&gateway.default_product));
        let spec = SpawnSpec::new(gateway.default_product.clone(), &workspace);
        Some(
            ensure_agent(&spec, &gateway.registry, &gateway.spawn_lock, gateway.pid)
                .await
                .map_err(|e| ApiError::service_unavailable(format!("无法拉起 agent：{e}")))?,
        )
    } else {
        None
    };

    Ok(AuthCtx {
        scope,
        workspace,
        instance,
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?
        .trim();
    let mut parts = raw.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

fn workspace_from_request(parts: &axum::http::request::Parts) -> Option<String> {
    query_param(&parts.uri.to_string(), "workspace")
        .or_else(|| {
            parts
                .headers
                .get("x-a-da-workspace")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .filter(|w| !w.trim().is_empty())
}

// ── 参数构造 ──────────────────────────────────────────────────────────────

fn build_params(
    spec: &RouteSpec,
    captures: &[(String, String)],
    uri: &axum::http::Uri,
    body: Value,
    auth_workspace: &str,
) -> Result<Value, ApiError> {
    let mut params = Map::new();

    match spec.body {
        BodyMode::None => {}
        BodyMode::MergeOptional => {
            if !body.is_null() {
                merge_object(&mut params, &body)?;
            }
        }
        BodyMode::Merge => {
            merge_object(&mut params, &body)?;
        }
        BodyMode::WrapOrMerge(key) => {
            if body.is_null() {
                // 允许空 body
            } else if let Some(obj) = body.as_object() {
                if obj.contains_key(key) {
                    merge_object(&mut params, &body)?;
                } else {
                    params.insert(key.to_string(), body.clone());
                }
            } else {
                return Err(ApiError::bad_request(
                    "请求体必须是 JSON 对象（或省略）".to_string(),
                ));
            }
        }
    }

    for (capture, param_key) in spec.path_map {
        if let Some((_, value)) = captures.iter().find(|(name, _)| name == capture) {
            // `:index` 在协议里是 u64，不能按字符串塞（dispatch 侧读的是 as_u64）
            let value = if *param_key == "index" {
                match value.parse::<u64>() {
                    Ok(n) => Value::Number(n.into()),
                    Err(_) => {
                        return Err(ApiError::bad_request(format!(
                            "路径参数 `{capture}` 必须是整数：{value}"
                        )))
                    }
                }
            } else {
                Value::String(value.clone())
            };
            set_param(&mut params, param_key, value);
        }
    }

    let uri_text = uri.to_string();
    for qs in spec.query {
        match query_param(&uri_text, qs.name) {
            Some(raw) => {
                let value = match qs.kind {
                    QKind::Str => Value::String(raw),
                    QKind::Bool => Value::Bool(matches!(raw.as_str(), "1" | "true" | "yes")),
                    QKind::U64 => match raw.parse::<u64>() {
                        Ok(n) => Value::Number(n.into()),
                        Err(_) => {
                            return Err(ApiError::bad_request(format!(
                                "查询参数 `{}` 必须是整数：{raw}",
                                qs.name
                            )))
                        }
                    },
                };
                params.insert(qs.key.to_string(), value);
            }
            None if qs.required => {
                return Err(ApiError::bad_request(format!("缺少查询参数 `{}`", qs.name)))
            }
            None => {}
        }
    }

    if spec.with_workspace && !params.contains_key("workspace") && !auth_workspace.is_empty() {
        params.insert(
            "workspace".to_string(),
            Value::String(auth_workspace.to_string()),
        );
    }

    Ok(Value::Object(params))
}

fn merge_object(dst: &mut Map<String, Value>, body: &Value) -> Result<(), ApiError> {
    let Some(obj) = body.as_object() else {
        return Err(ApiError::bad_request("请求体必须是 JSON 对象".to_string()));
    };
    for (k, v) in obj {
        dst.insert(k.clone(), v.clone());
    }
    Ok(())
}

fn set_param(params: &mut Map<String, Value>, key: &str, value: Value) {
    if let Some((outer, inner)) = key.split_once('.') {
        let entry = params
            .entry(outer.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(inner.to_string(), value);
        }
    } else {
        params.insert(key.to_string(), value);
    }
}

// ── 网关 / agent 调用 ─────────────────────────────────────────────────────

async fn call_gateway(gateway: &Gateway, op: GatewayOp, params: &Value) -> Result<Value, ApiError> {
    match op {
        GatewayOp::Info => Ok(gateway.info_result()),
        GatewayOp::Status => Ok(gateway.status_result()),
        GatewayOp::ListAgents => Ok(gateway.list_agents_result()),
        GatewayOp::FsRoots => Ok(gateway.fs_roots_result()),
        GatewayOp::WorkspacesList => Ok(gateway.workspaces_list_result()),
        GatewayOp::FsList => gateway
            .fs_list_directory_result(params)
            .map_err(ApiError::bad_request),
        GatewayOp::FsMkdir => gateway
            .fs_make_directory_result(params)
            .map_err(ApiError::bad_request),
        GatewayOp::WorkspacesRemove => gateway
            .workspaces_remove_result(params)
            .map_err(ApiError::bad_request),
        GatewayOp::Attach => {
            let ws = params.get("workspace").and_then(|v| v.as_str());
            gateway
                .attach_result(ws)
                .await
                .map_err(ApiError::service_unavailable)
        }
        GatewayOp::Detach => {
            let id = params.get("agentId").and_then(|v| v.as_str()).unwrap_or("");
            if id.is_empty() {
                return Err(ApiError::bad_request("缺少 agentId".to_string()));
            }
            match gateway.registry.remove(id) {
                Some(gone) => Ok(json!({
                    "detached": true,
                    "agentId": gone.id,
                    "note": "仅从注册表移除；进程由它的父进程看门狗回收",
                })),
                None => Err(ApiError::not_found(format!("没有这个 agent 实例：{id}"))),
            }
        }
        GatewayOp::CancelDelegation => {
            let id = params
                .get("delegationId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if id.is_empty() {
                return Err(ApiError::bad_request("缺少 delegationId".to_string()));
            }
            let cancelled = gateway.delegations.cancel(id).await;
            Ok(json!({ "cancelled": cancelled, "delegationId": id }))
        }
    }
}

async fn call_agent(
    instance: &AgentInstance,
    method: &str,
    params: Value,
) -> Result<Value, ApiError> {
    let endpoint = with_api_flag(&instance.endpoint);
    let request = endpoint
        .into_client_request()
        .map_err(|e| ApiError::bad_gateway(format!("构造 agent 请求失败：{e}")))?;

    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| ApiError::gateway_timeout("连接 agent 超时"))?
        .map_err(|e| ApiError::bad_gateway(format!("连接 agent 失败：{e}")))?;
    let (mut tx, mut rx) = ws.split();

    let init_id = 1u64;
    let init = json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "method": SESSION_INITIALIZE,
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "client": { "name": "a-da-http-api", "capabilities": [] }
        }
    });
    send_frame(&mut tx, init).await?;
    let _ = await_response(&mut rx, init_id).await?;

    let call_id = 2u64;
    let call = json!({
        "jsonrpc": "2.0",
        "id": call_id,
        "method": method,
        "params": params,
    });
    send_frame(&mut tx, call).await?;
    await_response(&mut rx, call_id).await
}

fn with_api_flag(endpoint: &str) -> String {
    if endpoint.contains("api=1") {
        return endpoint.to_string();
    }
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{sep}api=1")
}

async fn send_frame<S>(tx: &mut S, frame: Value) -> Result<(), ApiError>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let text = serde_json::to_string(&frame)
        .map_err(|e| ApiError::internal(format!("序列化请求失败：{e}")))?;
    tx.send(Message::Text(text.into()))
        .await
        .map_err(|e| ApiError::bad_gateway(format!("发送 agent 请求失败：{e}")))
}

async fn await_response<S>(rx: &mut S, expected_id: u64) -> Result<Value, ApiError>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let next = tokio::time::timeout(RPC_TIMEOUT, rx.next())
            .await
            .map_err(|_| ApiError::gateway_timeout("等待 agent 响应超时"))?;
        let Some(msg) = next else {
            return Err(ApiError::bad_gateway("agent 连接提前关闭"));
        };
        let msg = msg.map_err(|e| ApiError::bad_gateway(format!("读取 agent 响应失败：{e}")))?;
        let Message::Text(text) = msg else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str())
            .map_err(|e| ApiError::bad_gateway(format!("agent 返回了非法 JSON：{e}")))?;
        if !id_matches(value.get("id"), expected_id) {
            continue;
        }
        if let Some(err) = value.get("error") {
            return Err(ApiError::from_rpc_error(err));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn id_matches(id: Option<&Value>, expected: u64) -> bool {
    match id {
        Some(Value::Number(n)) => n.as_u64() == Some(expected),
        Some(Value::String(s)) => s.parse::<u64>().ok() == Some(expected),
        _ => false,
    }
}

// ── 路径 / 错误 / CORS ────────────────────────────────────────────────────

fn match_path(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
    let pattern = pattern.trim_matches('/');
    let path = path.trim_matches('/');
    let p_parts: Vec<&str> = if pattern.is_empty() {
        Vec::new()
    } else {
        pattern.split('/').collect()
    };
    let p_path: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    if p_parts.len() != p_path.len() {
        return None;
    }
    let mut captures = Vec::new();
    for (want, got) in p_parts.iter().zip(p_path.iter()) {
        if let Some(name) = want.strip_prefix(':') {
            captures.push((name.to_string(), percent_decode(got)));
        } else if want.starts_with('{') && want.ends_with('}') {
            captures.push((want[1..want.len() - 1].to_string(), percent_decode(got)));
        } else if want != got {
            return None;
        }
    }
    Some(captures)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: i64,
    message: String,
    data: Option<Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: i64, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            data: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, -32602, message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, -32001, message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, -32004, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, -32002, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, -32603, message)
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, -32003, message)
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, -32003, message)
    }

    fn gateway_timeout(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, -32005, message)
    }

    fn from_auth(error: AuthError) -> Self {
        match error {
            AuthError::MissingToken | AuthError::BadToken => Self::unauthorized(error.to_string()),
            AuthError::OriginNotAllowed(_) | AuthError::WorkspaceNotInScope { .. } => {
                Self::forbidden(error.to_string())
            }
            AuthError::BadPairingCode | AuthError::PairingNotAvailable => {
                Self::bad_request(error.to_string())
            }
        }
    }

    fn from_rpc_error(error: &Value) -> Self {
        let code = error.get("code").and_then(|v| v.as_i64()).unwrap_or(-32603);
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("agent 返回错误")
            .to_string();
        let status = match code {
            -32600 | -32602 => StatusCode::BAD_REQUEST,
            -32001 => StatusCode::UNAUTHORIZED,
            -32002 => StatusCode::NOT_FOUND,
            -32003 => StatusCode::SERVICE_UNAVAILABLE,
            -32004 | -32007 => StatusCode::FORBIDDEN,
            -32005 => StatusCode::GATEWAY_TIMEOUT,
            -32006 | -32009 | -32010 | -32011 => StatusCode::CONFLICT,
            -32008 => StatusCode::PAYLOAD_TOO_LARGE,
            -32012 => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let mut err = Self::new(status, code, message);
        err.data = error.get("data").cloned();
        err
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "data": self.data,
            }
        });
        (self.status, Json(body)).into_response()
    }
}

async fn api_cors(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    if let Some(o) = &origin {
        if let Err(e) = state.gateway.auth.check_origin(Some(o)) {
            warn!("HTTP API 拒绝来源：{e}");
            return (StatusCode::FORBIDDEN, e.to_string()).into_response();
        }
    }

    let preflight = req.method() == Method::OPTIONS;
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };

    if let Some(o) = origin {
        let headers = response.headers_mut();
        if let Ok(hv) = HeaderValue::from_str(&o) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, hv);
        }
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,POST,PUT,PATCH,DELETE,OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(
                "Authorization,Content-Type,X-A-Da-Workspace,If-None-Match,If-Match",
            ),
        );
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("ETag,X-A-Da-Workspace"),
        );
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

// ── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_QUERY: &[QSpec] = &[q("limit", "limit", QKind::U64, false)];

    #[test]
    fn test_route_table_has_no_duplicates_and_expected_size() {
        // 71 = Phase 1 的 18 条 + Phase 2 的 42 条 + 网关管理面 11 条。
        // 加/删端点时同步改这里，避免“文档写了、路由没挂”。
        assert_eq!(ROUTES.len(), 71);
        let mut seen = std::collections::HashSet::new();
        for route in ROUTES {
            assert!(
                seen.insert((route.http, route.path)),
                "重复路由：{} {}",
                route.http,
                route.path
            );
        }
    }

    #[test]
    fn test_build_params_converts_queue_index_to_u64() {
        let spec = RouteSpec {
            http: "POST",
            path: "/threads/:thread_id/queue/:index/promote",
            target: Target::Agent(QUEUE_PROMOTE),
            path_map: &[("thread_id", "threadId"), ("index", "index")],
            query: &[],
            body: BodyMode::None,
            with_workspace: false,
            etag: false,
            public: false,
        };
        let captures = vec![
            ("thread_id".to_string(), "t1".to_string()),
            ("index".to_string(), "3".to_string()),
        ];
        let uri: axum::http::Uri = "/threads/t1/queue/3/promote".parse().unwrap();
        let params = build_params(&spec, &captures, &uri, Value::Null, "").unwrap();
        assert_eq!(params["threadId"], "t1");
        assert!(
            params["index"].is_u64(),
            "queue 的 index 必须是数字，dispatch 读的是 as_u64"
        );
        assert_eq!(params["index"], 3);
    }

    #[test]
    fn test_merge_optional_allows_empty_body() {
        let spec = RouteSpec {
            http: "POST",
            path: "/threads",
            target: Target::Agent(THREAD_CREATE),
            path_map: &[],
            query: &[],
            body: BodyMode::MergeOptional,
            with_workspace: true,
            etag: false,
            public: false,
        };
        let uri: axum::http::Uri = "/threads".parse().unwrap();
        let params = build_params(&spec, &[], &uri, Value::Null, "E:/w").unwrap();
        assert_eq!(params["workspace"], "E:/w");
    }

    #[test]
    fn test_match_path_captures_and_rejects_mismatch() {
        let captures = match_path("/threads/:thread_id/items", "/threads/t1/items").unwrap();
        assert_eq!(captures, vec![("thread_id".to_string(), "t1".to_string())]);

        assert!(match_path("/threads/:thread_id/items", "/threads/t1").is_none());
        assert!(match_path("/config", "/config/").is_some());
        assert!(match_path("/config", "/config/presets").is_none());
    }

    #[test]
    fn test_build_params_merges_body_path_query_and_workspace() {
        let spec = RouteSpec {
            http: "POST",
            path: "/threads/:thread_id/changes/revert-file",
            target: Target::Agent(CHANGE_REVERT_FILE),
            path_map: &[("thread_id", "threadId")],
            query: TEST_QUERY,
            body: BodyMode::Merge,
            with_workspace: true,
            etag: false,
            public: false,
        };
        let captures = vec![("thread_id".to_string(), "t1".to_string())];
        let uri: axum::http::Uri = "/threads/t1/changes/revert-file?limit=10".parse().unwrap();
        let params = build_params(
            &spec,
            &captures,
            &uri,
            json!({ "path": "a.txt", "workspace": "E:/w" }),
            "E:/default",
        )
        .unwrap();
        assert_eq!(params["threadId"], "t1");
        assert_eq!(params["path"], "a.txt");
        assert_eq!(params["workspace"], "E:/w");
        assert_eq!(params["limit"], 10);
    }

    #[test]
    fn test_build_params_wraps_provider_and_sets_nested_id() {
        let spec = RouteSpec {
            http: "PUT",
            path: "/providers/:provider_id",
            target: Target::Agent(PROVIDER_SAVE),
            path_map: &[("provider_id", "provider.id")],
            query: &[],
            body: BodyMode::WrapOrMerge("provider"),
            with_workspace: false,
            etag: false,
            public: false,
        };
        let captures = vec![("provider_id".to_string(), "p1".to_string())];
        let uri: axum::http::Uri = "/providers/p1".parse().unwrap();
        let params = build_params(
            &spec,
            &captures,
            &uri,
            json!({ "name": "P", "baseUrl": "https://x", "apiKey": "k", "models": [] }),
            "",
        )
        .unwrap();
        assert_eq!(params["provider"]["id"], "p1");
        assert_eq!(params["provider"]["name"], "P");
    }

    #[test]
    fn test_api_error_maps_protocol_codes_to_http_status() {
        let err = ApiError::from_rpc_error(&json!({ "code": -32002, "message": "missing" }));
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        let err = ApiError::from_rpc_error(&json!({ "code": -32005, "message": "slow" }));
        assert_eq!(err.status, StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn test_with_api_flag_is_idempotent() {
        assert_eq!(
            with_api_flag("ws://127.0.0.1:1/rpc?token=t"),
            "ws://127.0.0.1:1/rpc?token=t&api=1"
        );
        assert_eq!(
            with_api_flag("ws://127.0.0.1:1/rpc?token=t&api=1"),
            "ws://127.0.0.1:1/rpc?token=t&api=1"
        );
    }

    /// 真正的代理契约：先 `session.initialize`，再发方法；事件帧必须被忽略。
    #[tokio::test]
    async fn test_call_agent_handshakes_sends_method_and_skips_events() {
        use crate::registry::AgentStatus;
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::handshake::server::{
            Request as WsRequest, Response as WsResponse,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_query = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen = seen_query.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_hdr_async(
                stream,
                move |req: &WsRequest, resp: WsResponse| {
                    *seen.lock().unwrap() = req.uri().query().unwrap_or("").to_string();
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            let (mut tx, mut rx) = ws.split();

            let init = rx.next().await.unwrap().unwrap();
            let Message::Text(init_text) = init else {
                panic!("expected text frame");
            };
            let init_val: Value = serde_json::from_str(init_text.as_str()).unwrap();
            assert_eq!(init_val["method"], SESSION_INITIALIZE);
            let init_id = init_val["id"].clone();
            tx.send(Message::Text(
                json!({ "jsonrpc": "2.0", "id": init_id, "result": { "sessionId": "s" } })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();

            let call = rx.next().await.unwrap().unwrap();
            let Message::Text(call_text) = call else {
                panic!("expected text frame");
            };
            let call_val: Value = serde_json::from_str(call_text.as_str()).unwrap();
            let call_id = call_val["id"].clone();

            // 事件帧出现在响应之前：代理必须按 id 跳过它
            tx.send(Message::Text(
                json!({ "jsonrpc": "2.0", "method": "evt.state.snapshot", "params": {} })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            tx.send(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "id": call_id,
                    "result": { "method": call_val["method"].clone(), "ok": true }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        });

        let instance = AgentInstance {
            id: "test".to_string(),
            product: "ada-coding".to_string(),
            workspace: "E:/test".to_string(),
            endpoint: format!("ws://{addr}/rpc?token=t"),
            status: AgentStatus::Ready,
            pid: None,
            started_at: 0,
            products: vec!["ada-coding".to_string()],
            last_active_ms: 0,
        };

        let result = call_agent(&instance, CONFIG_GET, json!({})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["method"], CONFIG_GET);
        server.await.unwrap();
        assert!(
            seen_query.lock().unwrap().contains("api=1"),
            "代理连接必须带 api=1，agent 侧才会关闭自动广播"
        );
    }
}
