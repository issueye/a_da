//! **进程内 Web UI 后端**：把 `agent-web` 的四个原语接到网关自己的调用面。
//!
//! 独立运行时 `agent-web` 走 HTTP/WS 回连网关；挂载进网关后没有这个回环：
//!
//! - 一元查询/命令 → [`crate::supervisor::ensure_agent`] + [`crate::http_api::call_agent`]；
//! - 网关管理面（`fs.*`）→ 直接调 [`Gateway`] 的结果函数；
//! - 事件订阅 → 直连该 agent 的本地端点，并持有 [`ConnectionGuard`]。
//!
//! 🔴 **事件订阅必须持有连接守卫**：回收器只认 `AgentRegistry::connections`，
//! 而这条连接不经过 `relay`。没有守卫时，浏览器标签页开着 15 分钟后 agent 会被
//! 判为空闲回收，表现就是"回答突然不动了"。

use std::path::PathBuf;
use std::sync::Arc;

use agent_web::agent::AgentClient;
use agent_web::backend::WebBackend;
use agent_web::guard::{CredentialVerifier, SharedAccess};
use agent_web::handlers::AppState as WebState;
use agent_web::plugins::PluginRegistry;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::link::AgentEndpoint;
use crate::relay::Gateway;
use crate::supervisor::{ensure_agent, SpawnSpec};

/// 把网关的凭据面（`--token` 或一次性配对码）借给浏览器登录。
///
/// 校验通过后 guard 只发一个随机 session id 的 HttpOnly cookie；
/// 网关 token 本身不会进入浏览器。
pub struct GatewayLogin {
    auth: Arc<crate::auth::AuthConfig>,
}

impl GatewayLogin {
    pub fn new(auth: Arc<crate::auth::AuthConfig>) -> Self {
        Self { auth }
    }
}

impl CredentialVerifier for GatewayLogin {
    fn verify(&self, credential: &str) -> Result<(), String> {
        match self.auth.authorize(Some(credential)) {
            Ok(crate::auth::AuthOutcome::Scoped(_)) => Ok(()),
            _ => self
                .auth
                .redeem_pairing_code(credential)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }
}

/// 组合辅助：给入口防护挂上网关登录。
pub fn with_gateway_login(
    access: agent_web::guard::Access,
    auth: Arc<crate::auth::AuthConfig>,
) -> agent_web::guard::Access {
    access.with_verifier(Arc::new(GatewayLogin::new(auth)))
}

/// 网关进程内的 [`WebBackend`] 实现。
pub struct GatewayBackend {
    gateway: Arc<Gateway>,
    workspace: String,
    product: String,
}

impl GatewayBackend {
    pub fn new(gateway: Arc<Gateway>, workspace: impl Into<String>, product: impl Into<String>) -> Self {
        Self {
            gateway,
            workspace: workspace.into(),
            product: product.into(),
        }
    }

    async fn instance(&self) -> Result<crate::registry::AgentInstance> {
        let spec = SpawnSpec::new(self.product.clone(), &self.workspace);
        ensure_agent(
            &spec,
            &self.gateway.registry,
            &self.gateway.spawn_lock,
            self.gateway.pid,
        )
        .await
        .map_err(|error| anyhow!("拉起 agent 失败：{error:?}"))
    }

    async fn call_rpc(&self, method: &str, params: Value) -> Result<Value> {
        let instance = self.instance().await?;
        crate::http_api::call_agent(&instance, method, params)
            .await
            .map_err(|error| anyhow!("agent 调用失败：{error}"))
    }
}

#[async_trait]
impl WebBackend for GatewayBackend {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let query_value = |name: &str| {
            query
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };

        match path {
            "/api/snapshot" => self.call_rpc("session.snapshot", json!({})).await,
            "/api/config" => self.call_rpc("config.get", json!({})).await,
            "/api/providers" => self.call_rpc("provider.list", json!({})).await,
            "/api/plugins" => self.call_rpc("plugin.list", json!({})).await,
            "/api/fs/roots" => Ok(self.gateway.fs_roots_result()),
            "/api/fs/list" => {
                let mut params = json!({});
                if let Some(path) = query_value("path") {
                    params["path"] = json!(path);
                }
                self.gateway
                    .fs_list_directory_result(&params)
                    .map_err(|error| anyhow!("列目录失败：{error}"))
            }
            other => {
                // `/api/threads/{id}/items`
                let Some(thread_id) = other
                    .strip_prefix("/api/threads/")
                    .and_then(|rest| rest.strip_suffix("/items"))
                else {
                    bail!("未映射的 GET {other}");
                };
                let mut params = json!({ "threadId": thread_id });
                if let Some(from) = query_value("from").and_then(|v| v.parse::<u64>().ok()) {
                    params["from"] = json!(from);
                }
                if let Some(count) = query_value("count").and_then(|v| v.parse::<u64>().ok()) {
                    params["count"] = json!(count);
                }
                self.call_rpc("thread.items.page", params).await
            }
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        match path {
            "/api/threads" => self.call_rpc("thread.create", body).await,
            "/api/fs/mkdir" => self
                .gateway
                .fs_make_directory_result(&body)
                .map_err(|error| anyhow!("新建目录失败：{error}")),
            other => bail!("未映射的 POST {other}"),
        }
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        self.call_rpc(method, params).await
    }

    async fn subscribe(&self, tx: mpsc::Sender<Value>) -> Result<()> {
        let instance = self.instance().await?;
        // 守卫先于连接建立：从这一刻起回收器认为"有客户端在看这个 agent"。
        let _conn = self
            .gateway
            .registry
            .open_connection(instance.id.clone());
        let endpoint = AgentEndpoint::parse(&instance.endpoint)
            .map_err(|error| anyhow!("解析 agent 端点失败：{error}"))?;
        let ws = endpoint.connect().await?;
        let (mut sink, mut rx) = ws.split();

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session.initialize",
            "params": {
                "protocolVersion": "1.0",
                "client": { "name": "a-da-web-ui", "capabilities": ["state.patch"] }
            }
        });
        let text = serde_json::to_string(&init)?;
        sink.send(Message::Text(text.into()))
            .await
            .map_err(|error| anyhow!("发送 initialize 失败：{error}"))?;

        while let Some(next) = rx.next().await {
            let message = next.map_err(|error| anyhow!("读取 agent 事件失败：{error}"))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(text.as_str())?;
            let Some(method) = value.get("method").and_then(Value::as_str) else {
                continue;
            };
            if method != "evt.state.snapshot" && method != "evt.state.patch" {
                continue;
            }
            // 每帧都刷新活跃时间；连接计数已经保证不会被回收，这里是双保险。
            self.gateway.registry.touch(&instance.id);
            let frame = json!({
                "method": method,
                "params": value.get("params").cloned().unwrap_or(Value::Null),
            });
            if tx.send(frame).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    fn base_url(&self) -> &str {
        "gateway://in-process"
    }
}

/// 构造挂载给网关的 [`WebState`]（后端 + 前端插件注册表 + 入口防护）。
pub fn build_web_state(
    gateway: Arc<Gateway>,
    workspace: String,
    product: String,
    plugins_dir: PathBuf,
    access: SharedAccess,
) -> WebState {
    let backend = Arc::new(GatewayBackend::new(
        gateway,
        workspace.clone(),
        product,
    ));
    let agent = Arc::new(AgentClient::with_backend(backend, workspace));
    let frontend = Arc::new(PluginRegistry::new(plugins_dir));
    WebState {
        agent,
        frontend,
        access,
    }
}
