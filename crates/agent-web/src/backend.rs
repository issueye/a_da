//! 到网关的传输端口。
//!
//! 界面处理器只认 [`WebBackend`] 的四个原语；两种实现：
//!
//! - [`HttpBackend`]：独立运行时的 HTTP + 短/长 WebSocket（原 `AgentClient` 逻辑）；
//! - 网关进程内的 `GatewayBackend`（在 `agent-gateway`）：一元调用直接进网关函数，
//!   事件订阅直连本工作区 agent，少一次自反代。
//!
//! 端口保持"路径/方法 + JSON"的形状，是为了让两种实现可以用同一组等价性测试
//! 对比返回值，而不是各自发明一套 DTO。

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const RPC_TIMEOUT: Duration = Duration::from_secs(60);

#[async_trait]
pub trait WebBackend: Send + Sync {
    /// 一元 GET（`path` 形如 `/api/snapshot`，`query` 是额外参数）。
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value>;

    /// 一元 POST。
    async fn post(&self, path: &str, body: Value) -> Result<Value>;

    /// 需要 WS 的命令（`thread.send` / `question.answer` / ...）。
    async fn rpc(&self, method: &str, params: Value) -> Result<Value>;

    /// 订阅 `evt.state.snapshot` / `evt.state.patch`；连接断开时返回。
    async fn subscribe(&self, tx: mpsc::Sender<Value>) -> Result<()>;

    /// 展示用地址（错误文案里不带 token）。
    fn base_url(&self) -> &str;
}

/// 独立运行时的后端：HTTP `/api/*` + 每次一条短 WS 的命令 + 一条长 WS 的事件。
pub struct HttpBackend {
    http: reqwest::Client,
    base: String,
    token: String,
    workspace: String,
}

impl HttpBackend {
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

    /// 网关 `/rpc` 的 WebSocket 地址（工作区与 token 都走 query）。
    fn rpc_url(&self) -> Result<reqwest::Url> {
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
        Ok(url)
    }
}

#[async_trait]
impl WebBackend for HttpBackend {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let mut url = self.url(path)?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        let resp = self
            .auth(self.http.get(url))
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

    /// 一次短 WS 连接：`session.initialize` → 目标方法 → 读取响应。
    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let url = self.rpc_url()?;
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

    /// 订阅网关的**状态事件**，把 `evt.state.snapshot` / `evt.state.patch` 转进通道。
    ///
    /// 与 [`WebBackend::rpc`] 的区别：这条连接是**长期持有**的（页面开着就一直连着），
    /// 而且只读通知、不注册任何界面状态。`state.patch` 能力必须在这里声明：
    /// 宿主只有收到声明才会把后续帧切成"只带变过的 item"的增量帧。
    async fn subscribe(&self, tx: mpsc::Sender<Value>) -> Result<()> {
        let url = self.rpc_url()?;
        let display = display_url(&url);
        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .with_context(|| format!("连接网关失败: {display}"))?;
        let (mut sink, mut rx) = ws.split();

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session.initialize",
            "params": {
                "protocolVersion": "1.0",
                "client": { "name": "ssr-h", "capabilities": ["state.patch"] }
            }
        });
        send(&mut sink, init).await?;

        loop {
            let next = rx.next().await.ok_or_else(|| anyhow!("网关连接已关闭"))?;
            let msg = next.map_err(|e| anyhow!("读取网关事件失败: {e}"))?;
            let Message::Text(text) = msg else {
                continue;
            };
            let value: Value =
                serde_json::from_str(text.as_str()).context("网关返回了非法 JSON")?;
            let Some(method) = value.get("method").and_then(Value::as_str) else {
                continue; // 请求/响应帧（含 initialize 的应答）不参与状态投影
            };
            if method != "evt.state.snapshot" && method != "evt.state.patch" {
                continue;
            }
            let frame = json!({
                "method": method,
                "params": value.get("params").cloned().unwrap_or(Value::Null),
            });
            if tx.send(frame).await.is_err() {
                break; // 页面已经关了：结束连接，别继续占着网关
            }
        }
        Ok(())
    }

    fn base_url(&self) -> &str {
        &self.base
    }
}

/// 可安全展示的端点：丢掉 query（里面带着网关 token）与 userinfo。
pub(crate) fn display_url(url: &reqwest::Url) -> String {
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
