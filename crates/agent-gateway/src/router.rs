//! 网关 HTTP 路由的**唯一组装点**。
//!
//! 生产（`main.rs`）与集成测试都调这里，所以“测试里跑的路由”和“线上跑的路由”
//! 是同一份。早先测试自己复制过一段分流逻辑，结果只有测试覆盖到的那条路是对的——
//! 这种坑不该有第二次。
//!
//! 路由只有三条：
//!
//! | 路径 | 去向 |
//! |---|---|
//! | `/rpc` | [`crate::relay::ws_handler`]：鉴权后升级为 WebSocket |
//! | `/api/*` | [`crate::http_api`]：unary 命令/查询的 HTTP 面 |
//! | 其他 | SSR 子 service（[`router_with_web`]）/ 404（[`router`]） |
//!
//! 三条路在**同一个端口**上，所以浏览器里“页面”与“数据通道”天然同源。

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use crate::http_api;
use crate::relay::{ws_handler, AppState};

/// 没有 SSR 子 service 时的扑空响应。
///
/// 生产不会走这里（`main.rs` 总是挂 SSR）；留着是给集成测试与将来的“纯 API”
/// 形态一个确定答案：**404，而不是一页说明或一个静态产物**。
async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "404 Not Found")
}

/// 组装**纯 API** 的 HTTP service（无页面）。
///
/// 三条路由：
/// - `/rpc`：原有 JSON-RPC over WebSocket（事件与交互不变）；
/// - `/api/*`：HTTP API（unary 命令/查询；事件仍走 `/rpc` 的推送）；
/// - 其他：404。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rpc", get(ws_handler))
        .nest("/api", http_api::router(state.clone()))
        .fallback(not_found)
        .with_state(state)
}

/// 组装网关 + SSR 网页界面。
///
/// SSR 路由作为一个完整的子 service 挂在 fallback 上：`/rpc` 与 `/api/*` 先匹配，
/// 其余请求（页面、表单动作、SSE、`/static/*`、插件路由）交给 `agent-web`。
/// 它自己的 guard/CSP 只包住这一个子 service，不会污染 API 面。
pub fn router_with_web(state: AppState, web: Router) -> Router {
    Router::new()
        .route("/rpc", get(ws_handler))
        .nest("/api", http_api::router(state.clone()))
        .fallback_service(web)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::auth::AuthConfig;
    use crate::registry::{AgentInstance, AgentRegistry, AgentStatus};
    use crate::relay::Gateway;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio_tungstenite::tungstenite::Message;

    fn state_with_auth(token: &str) -> AppState {
        let gateway = Gateway::new("ada-coding", "E:/test")
            .with_auth(AuthConfig::from_args(&[token.to_string()], &[]).unwrap());
        AppState {
            gateway: Arc::new(gateway),
            default_workspace: "E:/test".to_string(),
        }
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("读取响应体");
        serde_json::from_slice(&bytes).expect("响应必须是 JSON")
    }

    #[tokio::test]
    async fn test_api_gateway_info_is_public_and_routes_are_nested() {
        let app = router(state_with_auth("secret"));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/gateway/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value = body_json(resp).await;
        assert_eq!(value["protocolVersion"], "1.0");
        assert_eq!(value["authRequired"], true);
    }

    #[tokio::test]
    async fn test_api_requires_bearer_token() {
        let app = router(state_with_auth("secret"));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/gateway/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/gateway/status")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value = body_json(resp).await;
        assert_eq!(value["product"], "ada-coding");
    }

    #[tokio::test]
    async fn test_scoped_token_can_read_gateway_status_but_not_other_workspace() {
        let gateway = Gateway::new("ada-coding", "E:/test")
            .with_auth(AuthConfig::from_args(&["t=E:/other".to_string()], &[]).unwrap());
        let app = router(AppState {
            gateway: Arc::new(gateway),
            default_workspace: "E:/test".to_string(),
        });

        // 全局管理面不受工作区作用域限制
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/gateway/status")
                    .header("authorization", "Bearer t")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 但 agent 面仍必须落在 token 作用域内（这里会在拉起 agent 前拒绝）
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .header("authorization", "Bearer t")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_api_cors_preflight_does_not_require_token() {
        let app = router(state_with_auth("secret"));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/config")
                    .header("origin", "http://localhost:5173")
                    .header("access-control-request-method", "PUT")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://localhost:5173")
        );
    }

    /// 起一个假 agent：每条连接先应答 initialize，再应答一个方法。
    async fn spawn_fake_agent(connections: usize) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            for _ in 0..connections {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let (mut tx, mut rx) = ws.split();

                    let init = rx.next().await.unwrap().unwrap();
                    let Message::Text(init_text) = init else {
                        return;
                    };
                    let init_val: Value = serde_json::from_str(init_text.as_str()).unwrap();
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
                        return;
                    };
                    let call_val: Value = serde_json::from_str(call_text.as_str()).unwrap();
                    let call_id = call_val["id"].clone();
                    let method = call_val["method"].as_str().unwrap_or("").to_string();
                    let result = if method == agent_proto::SESSION_SNAPSHOT {
                        json!({ "seq": 42, "threads": [] })
                    } else {
                        json!({ "method": method, "params": call_val["params"].clone() })
                    };
                    tx.send(Message::Text(
                        json!({ "jsonrpc": "2.0", "id": call_id, "result": result })
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn test_api_forwards_get_write_and_serves_snapshot_etag() {
        let port = spawn_fake_agent(4).await;
        let gateway = Gateway::new("ada-coding", "E:/test")
            .with_auth(AuthConfig::from_args(&["secret".to_string()], &[]).unwrap());
        let agent_id =
            AgentRegistry::id_for_workspace("ada-coding", std::path::Path::new("E:/test"));
        gateway.registry.register(AgentInstance {
            id: agent_id,
            product: "ada-coding".to_string(),
            workspace: "E:/test".to_string(),
            endpoint: format!("ws://127.0.0.1:{port}/rpc?token=t"),
            status: AgentStatus::Ready,
            pid: None,
            started_at: 0,
            products: vec!["ada-coding".to_string()],
            last_active_ms: 0,
        });
        let state = AppState {
            gateway: Arc::new(gateway),
            default_workspace: "E:/test".to_string(),
        };
        let app = router(state);

        // GET -> config.get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value = body_json(resp).await;
        assert_eq!(value["method"], agent_proto::CONFIG_GET);

        // PUT -> config.setEffort，body 直接是 params
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config/effort")
                    .header("authorization", "Bearer secret")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"effort":"medium"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value = body_json(resp).await;
        assert_eq!(value["method"], agent_proto::CONFIG_SET_EFFORT);
        assert_eq!(value["params"]["effort"], "medium");

        // GET snapshot 带 ETag；同 ETag 再请求得到 304
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("etag").and_then(|v| v.to_str().ok()),
            Some("\"snap-42\"")
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot")
                    .header("authorization", "Bearer secret")
                    .header("if-none-match", "\"snap-42\"")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn test_api_unknown_route_is_json_404() {
        let app = router(state_with_auth("secret"));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/nope")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let value = body_json(resp).await;
        assert!(value["error"]["message"].as_str().unwrap().contains("未知"));
    }
}
