//! 网关挂载 SSR 界面的端到端测试（桩 agent，不依赖外部进程）。
//!
//! 这里守四件事：
//!
//! 1. `--web-ui ssr` 时 `/` 由 SSR 接管，`/api/*` 与 `/rpc` 仍是网关自己的；
//! 2. SSr 的 guard/CSP 只作用于 SSR 路由，不会污染 `/api`；
//! 3. 前端插件自省端点搬到了 `/web-ui/frontend-plugins`，不再撞 `/api/plugins`；
//! 4. 事件订阅持有连接计数守卫——浏览器开着页面时实例不会被回收器判为空闲。

use std::sync::Arc;
use std::time::Duration;

use agent_gateway::registry::{AgentInstance, AgentStatus};
use agent_gateway::relay::AppState;
use agent_gateway::router::router_with_web;
use agent_gateway::auth::AuthConfig;
use agent_gateway::web::{GatewayBackend, GatewayLogin};
use agent_gateway::{AgentRegistry, Gateway};
use agent_web::agent::AgentClient;
use agent_web::backend::WebBackend;
use agent_web::guard::Access;
use agent_web::handlers::AppState as WebState;
use agent_web::plugins::PluginRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:8788";

/// 桩 agent 宿主：连上先推种子快照，然后对每个请求回一帧带方法名的响应。
async fn stub_agent_host() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("桩宿主绑定");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (mut tx, mut rx) = ws.split();
                let seed = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "evt.state.snapshot",
                    "params": { "seq": 0, "topic": "evt.state.snapshot", "payload": { "from": "stub" } }
                });
                let _ = tx.send(Message::Text(seed.to_string().into())).await;

                while let Some(Ok(msg)) = rx.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(req) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "answeredBy": "stub-agent", "method": method }
                    });
                    if tx.send(Message::Text(resp.to_string().into())).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    port
}

fn new_gateway(workspace: &str) -> Arc<Gateway> {
    Arc::new(
        Gateway::new("ada-coding", workspace).with_workspaces(Arc::new(
            agent_gateway::WorkspacesStore::new(Some(
                std::env::temp_dir()
                    .join(format!("a_da_web_ws_{}.json", uuid::Uuid::new_v4().simple())),
            )),
        )),
    )
}

fn register_stub(gateway: &Arc<Gateway>, workspace: &str, stub_port: u16) {
    let ws_norm = agent_gateway::normalize_workspace(workspace);
    let id = AgentRegistry::id_for_workspace("ada-coding", &ws_norm);
    gateway.registry.register(AgentInstance {
        id,
        product: "ada-coding".to_string(),
        workspace: ws_norm.to_string_lossy().to_string(),
        endpoint: format!("ws://127.0.0.1:{stub_port}/rpc?token=stub-token"),
        status: AgentStatus::Ready,
        pid: None,
        started_at: 1,
        products: vec!["ada-coding".to_string()],
        last_active_ms: 1,
    });
}

async fn gateway_with_stub_agent() -> (Arc<Gateway>, String) {
    let workspace = "E:/a_da_web_e2e".to_string();
    let stub_port = stub_agent_host().await;
    let gateway = new_gateway(&workspace);
    register_stub(&gateway, &workspace, stub_port);
    (gateway, workspace)
}

fn web_state_with_access(
    gateway: Arc<Gateway>,
    workspace: &str,
    access: Access,
) -> WebState {
    let backend = Arc::new(GatewayBackend::new(
        gateway,
        workspace.to_string(),
        "ada-coding",
    ));
    let agent = Arc::new(AgentClient::with_backend(backend, workspace.to_string()));
    let plugins = std::env::temp_dir().join(format!(
        "a_da_web_plugins_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&plugins).unwrap();
    let state_file = plugins.join(".state.json");
    let frontend = Arc::new(PluginRegistry::with_state_path(plugins, state_file));
    WebState {
        agent,
        frontend,
        access: Arc::new(access),
    }
}

fn web_state(gateway: Arc<Gateway>, workspace: &str) -> WebState {
    web_state_with_access(
        gateway,
        workspace,
        Access::for_host("127.0.0.1".parse().unwrap(), 8788, None),
    )
}

fn app_with_web(gateway: Arc<Gateway>, workspace: &str) -> Router {
    let state = AppState {
        gateway: gateway.clone(),
        default_workspace: workspace.to_string(),
        site: None,
    };
    let web = agent_web::router(web_state(gateway, workspace));
    router_with_web(state, web)
}

async fn login(app: &Router, credential: &str) -> axum::response::Response {
    let body = format!("token={credential}");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/__auth")
                .header("host", HOST)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("host", HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn ssr_owns_root_while_api_and_plugin_introspection_stay_separate() {
    let (gateway, workspace) = gateway_with_stub_agent().await;
    let app = app_with_web(gateway, &workspace);

    let (status, headers, body) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/html"),
        "根路径应由 SSR 返回 HTML"
    );
    assert!(body.to_ascii_lowercase().contains("<html"), "首页不是 HTML：{body}");

    // 网关自己的 API 不受影响；agent 插件列表由桩 agent 回答。
    let (status, _, body) = get(&app, "/api/plugins").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("\"answeredBy\":\"stub-agent\""),
        "`/api/plugins` 应仍走网关的 plugin.list：{body}"
    );
    assert!(!body.contains("\"slots\""), "`/api/plugins` 不应命中前端插件自省");

    // 前端插件自省搬到了 `/web-ui/frontend-plugins`。
    let (status, _, body) = get(&app, "/web-ui/frontend-plugins").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"plugins\""), "自省端点应返回插件快照：{body}");
}

#[tokio::test]
async fn ssr_guard_does_not_leak_csp_into_api_responses() {
    let (gateway, workspace) = gateway_with_stub_agent().await;
    let app = app_with_web(gateway, &workspace);

    let (status, headers, _) = get(&app, "/api/gateway/info").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.get("content-security-policy").is_none(),
        "SSR 的 CSP 只应包住 SSR 路由"
    );

    // 跨站写请求被 SSR guard 拦下（不是被 API 的 Origin 规则拦）。
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/threads")
                .header("host", HOST)
                .header("origin", "http://evil.example")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("title=x&workspace=E:/a_da_web_e2e"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn browser_login_with_gateway_token_or_pairing_code_gets_a_session_cookie() {
    let workspace = "E:/a_da_web_e2e_login".to_string();
    let stub_port = stub_agent_host().await;
    let auth = Arc::new(
        AuthConfig::from_args(&["gateway-secret-token".to_string()], &[]).unwrap(),
    );
    let gateway = Arc::new(
        Gateway::new("ada-coding", &workspace)
            .with_auth_arc(auth.clone())
            .with_workspaces(Arc::new(agent_gateway::WorkspacesStore::new(Some(
                std::env::temp_dir()
                    .join(format!("a_da_web_ws_{}.json", uuid::Uuid::new_v4().simple())),
            )))),
    );
    register_stub(&gateway, &workspace, stub_port);

    let access = Access::for_host("127.0.0.1".parse().unwrap(), 8788, None)
        .with_verifier(Arc::new(GatewayLogin::new(auth.clone())));
    let web = web_state_with_access(gateway.clone(), &workspace, access);
    let state = AppState {
        gateway,
        default_workspace: workspace.clone(),
        site: None,
    };
    let app = router_with_web(state, agent_web::router(web));

    // 未登录：页面 401，而不是把内容直接给出去。
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 网关 token 登录：换发随机 session id，cookie 里不能出现网关 token 原文。
    let response = login(&app, "gateway-secret-token").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("应设置 cookie")
        .to_string();
    assert!(cookie.contains("ssr_h_session="), "{cookie}");
    assert!(!cookie.contains("gateway-secret-token"), "cookie 不应含网关 token");
    let session = cookie.split(';').next().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", HOST)
                .header("cookie", &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 一次性配对码也能登录（同一个凭据面，不需要再记一个 --web-token）。
    let code = auth.issue_pairing_code(Vec::new());
    let response = login(&app, &code).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("ssr_h_session=")
    );
}

#[tokio::test]
async fn event_subscription_holds_a_connection_guard() {
    let (gateway, workspace) = gateway_with_stub_agent().await;
    let ws_norm = agent_gateway::normalize_workspace(&workspace);
    let id = AgentRegistry::id_for_workspace("ada-coding", &ws_norm);

    let backend = Arc::new(GatewayBackend::new(
        gateway.clone(),
        workspace.clone(),
        "ada-coding",
    ));
    let (tx, mut rx) = mpsc::channel(8);
    let task = tokio::spawn(async move { backend.subscribe(tx).await });

    for _ in 0..200 {
        if gateway.registry.active_connections(&id) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(gateway.registry.active_connections(&id), 1);
    assert!(
        gateway
            .registry
            .idle_since(i64::MAX / 2, 0)
            .is_empty(),
        "有订阅连接时不能被判定为空闲"
    );

    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("应收到种子快照")
        .expect("事件通道不应关闭");
    assert_eq!(frame["method"], "evt.state.snapshot");

    task.abort();
    let _ = task.await;
    for _ in 0..200 {
        if gateway.registry.active_connections(&id) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(gateway.registry.active_connections(&id), 0);
    assert_eq!(
        gateway.registry.idle_since(i64::MAX / 2, 0),
        vec![id],
        "订阅结束后应重新可被回收"
    );
}
