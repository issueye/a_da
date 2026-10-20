//! 现有网关联调客户端。
//!
//! - 查询走网关 HTTP `/api/*`（Phase 1/2 已经落地）；
//! - 需要 WS 的命令（如 `thread.send` / `thread.abort` / `plugin.reload`）走
//!   `/rpc` 的 JSON-RPC，每次短连接、握手后只发一个方法。
//!
//! 这样 `ssr-h` 不复制任何 agent 状态，也不依赖桌面端内部实现。

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const RPC_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct AgentClient {
    http: reqwest::Client,
    base: String,
    token: String,
    pub workspace: String,
}

impl AgentClient {
    pub fn new(
        base: impl Into<String>,
        token: impl Into<String>,
        workspace: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("构建 HTTP client 失败"),
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
            workspace: workspace.into(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> Result<reqwest::Url> {
        let mut url = reqwest::Url::parse(&format!("{}{}", self.base, path))
            .with_context(|| format!("非法网关地址: {}", self.base))?;
        url.query_pairs_mut()
            .append_pair("workspace", &self.workspace);
        Ok(url)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.trim().is_empty() {
            req
        } else {
            req.bearer_auth(self.token.trim())
        }
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self
            .auth(self.http.get(self.url(path)?))
            .send()
            .await
            .with_context(|| format!("请求网关失败: {path}"))?;
        self.parse_response(resp).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let resp = self
            .auth(self.http.post(self.url(path)?).json(&body))
            .send()
            .await
            .with_context(|| format!("请求网关失败: {path}"))?;
        self.parse_response(resp).await
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "网关返回 HTTP {status}: {}",
                text.chars().take(400).collect::<String>()
            );
        }
        serde_json::from_str(&text).context("网关返回的不是 JSON")
    }

    pub async fn snapshot(&self) -> Result<Value> {
        self.get("/api/snapshot").await
    }

    pub async fn config(&self) -> Result<Value> {
        self.get("/api/config").await
    }

    pub async fn providers(&self) -> Result<Value> {
        self.get("/api/providers").await
    }

    pub async fn plugins(&self) -> Result<Value> {
        self.get("/api/plugins").await
    }

    pub async fn create_thread(&self, title: &str) -> Result<Value> {
        self.post(
            "/api/threads",
            json!({ "title": title, "workspace": self.workspace }),
        )
        .await
    }

    pub async fn send_message(&self, thread_id: &str, text: &str) -> Result<Value> {
        self.rpc(
            "thread.send",
            json!({ "threadId": thread_id, "text": text }),
        )
        .await
    }

    pub async fn abort_thread(&self, thread_id: &str) -> Result<Value> {
        self.rpc("thread.abort", json!({ "threadId": thread_id }))
            .await
    }

    pub async fn retry_thread(&self, thread_id: &str) -> Result<Value> {
        self.rpc("thread.retry", json!({ "threadId": thread_id }))
            .await
    }

    pub async fn decide_approval(&self, call_id: &str, approved: bool) -> Result<Value> {
        self.rpc(
            "approval.decide",
            json!({ "toolItemId": call_id, "approved": approved }),
        )
        .await
    }

    pub async fn promote_queue(&self, index: usize, thread_id: &str) -> Result<Value> {
        self.rpc(
            "queue.promote",
            json!({ "index": index, "threadId": thread_id }),
        )
        .await
    }

    pub async fn remove_queue(&self, index: usize) -> Result<Value> {
        self.rpc("queue.remove", json!({ "index": index })).await
    }

    pub async fn clear_queue(&self, thread_id: &str) -> Result<Value> {
        self.rpc("queue.clear", json!({ "threadId": thread_id }))
            .await
    }

    pub async fn reload_agent_plugins(&self) -> Result<Value> {
        self.rpc("plugin.reload", json!({ "workspace": self.workspace }))
            .await
    }

    /// 一次短 WS 连接：`session.initialize` → 目标方法 → 读取响应。
    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let mut url = reqwest::Url::parse(&self.base)
            .with_context(|| format!("非法网关地址: {}", self.base))?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| anyhow!("网关地址不支持转换为 WebSocket"))?;
        let mut path = url.path().trim_end_matches('/').to_string();
        if !path.ends_with("/rpc") {
            path.push_str("/rpc");
        }
        url.set_path(&path);
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("workspace", &self.workspace);
            if !self.token.trim().is_empty() {
                q.append_pair("token", self.token.trim());
            }
        }

        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .with_context(|| format!("连接网关失败: {url}"))?;
        let (mut tx, mut rx) = ws.split();

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session.initialize",
            "params": {
                "protocolVersion": "1.0",
                "client": { "name": "ssr-h", "capabilities": [] }
            }
        });
        send(&mut tx, init).await?;
        let _ = wait(&mut rx, 1).await?;

        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": method,
            "params": params,
        });
        send(&mut tx, call).await?;
        wait(&mut rx, 2).await
    }
}

async fn send<S>(tx: &mut S, frame: Value) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let text = serde_json::to_string(&frame)?;
    tx.send(Message::Text(text.into()))
        .await
        .map_err(|e| anyhow!("发送网关帧失败: {e}"))
}

async fn wait<S>(rx: &mut S, expected_id: u64) -> Result<Value>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let next = tokio::time::timeout(RPC_TIMEOUT, rx.next())
            .await
            .map_err(|_| anyhow!("等待网关响应超时"))?;
        let Some(msg) = next else {
            bail!("网关连接提前关闭");
        };
        let msg = msg.map_err(|e| anyhow!("读取网关响应失败: {e}"))?;
        let Message::Text(text) = msg else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str()).context("网关返回了非法 JSON")?;
        let id_ok = match value.get("id") {
            Some(Value::Number(n)) => n.as_u64() == Some(expected_id),
            Some(Value::String(s)) => s.parse::<u64>().ok() == Some(expected_id),
            _ => false,
        };
        if !id_ok {
            continue;
        }
        if let Some(err) = value.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("网关返回错误");
            bail!("网关错误 {code}: {msg}");
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}
