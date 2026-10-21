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

    /// 整条保存一个供应商（`provider.save`）。
    ///
    /// 必须发**整条**实体：后端是"存在就替换"的语义，少带的字段会被当成空值落盘
    /// （模型列表就丢了）。所以调用方一律先读当前条目、改字段、再整体写回。
    pub async fn save_provider(&self, provider: &Value) -> Result<Value> {
        self.rpc("provider.save", json!({ "provider": provider }))
            .await
    }

    pub async fn delete_provider(&self, id: &str) -> Result<Value> {
        self.rpc("provider.delete", json!({ "id": id })).await
    }

    /// 设为激活供应商，可选指定模型。
    pub async fn set_active_provider(&self, id: &str, model: Option<&str>) -> Result<Value> {
        let mut params = json!({ "id": id });
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            params["model"] = json!(model.trim());
        }
        self.rpc("provider.setActive", params).await
    }

    /// 拉取供应商的远程模型列表（会真的发 `/models` 请求）。
    pub async fn fetch_provider_models(&self, params: Value) -> Result<Value> {
        self.rpc("provider.fetchModels", params).await
    }

    /// 配置校验（`config.checkProvider`）。注意后端当前只做参数校验、**不发起真实请求**。
    pub async fn check_provider(&self, params: Value) -> Result<Value> {
        self.rpc("config.checkProvider", params).await
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

        // 🔴 错误文案里只能是 scheme://host:port/path：URL 的 query 带着网关 token，
        // 它会一路被 AppError 回显到浏览器页面（2026-10-21 实测过的泄漏路径）。
        let display = display_url(&url);
        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .with_context(|| format!("连接网关失败: {display}"))?;
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

/// 可安全展示的端点：丢掉 query（里面带着网关 token）与 userinfo。
fn display_url(url: &reqwest::Url) -> String {
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or("?"));
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(url.path());
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_url_drops_query_so_the_gateway_token_never_reaches_a_page() {
        let url =
            reqwest::Url::parse("ws://127.0.0.1:52353/rpc?workspace=E%3A%2Fw&token=SECRET-TOKEN")
                .unwrap();
        let shown = display_url(&url);
        assert_eq!(shown, "ws://127.0.0.1:52353/rpc");
        assert!(!shown.contains("SECRET"));
        assert!(!shown.contains('?'));
    }
}
