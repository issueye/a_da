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
use tokio::sync::mpsc;
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
        self.get_with(path, &[]).await
    }

    /// 带额外 query 的 GET（`workspace` 由 [`AgentClient::url`] 统一附加）。
    async fn get_with(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut url = self.url(path)?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in params {
                query.append_pair(key, value);
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

    /// 推理等级（`config.setEffort`：`low | medium | high | max`）。
    ///
    /// 与 desktop-ui 的设置项同源：网关落在 `config.json` 的 `effort` 并广播快照，
    /// 不在本进程里留第二份状态。
    pub async fn set_effort(&self, effort: &str) -> Result<Value> {
        self.rpc("config.setEffort", json!({ "effort": effort })).await
    }

    /// 切换审批档位（`config.setApproval`：auto / ask / readonly / full）。
    ///
    /// 后端会落盘并广播——快照的 `config.approval` 变化会让 parts 重发，
    /// 输入框的档位弹层随之更新。
    pub async fn set_approval_mode(&self, mode: &str) -> Result<Value> {
        self.rpc("config.setApproval", json!({ "mode": mode })).await
    }

    pub async fn plugins(&self) -> Result<Value> {
        self.get("/api/plugins").await
    }

    pub async fn create_thread(&self, title: &str) -> Result<Value> {
        self.create_thread_in(title, &self.workspace, None).await
    }

    /// 在指定工作区里新建会话。
    ///
    /// 工作区是**会话属性**：同一个 agent 进程能看到所有工作区的会话，
    /// 所以不需要为切换工作区重连；新建时把目标路径带上即可。
    pub async fn create_thread_in(
        &self,
        title: &str,
        workspace: &str,
        mode: Option<&str>,
    ) -> Result<Value> {
        // 网关的 POST /threads 是 THREAD_CREATE 的透明代理（BodyMode::MergeOptional），
        // mode 原样透传给 thread.create —— 建会话时就把模式定下来。
        let mut body = json!({ "title": title, "workspace": workspace });
        if let Some(mode) = mode {
            body["mode"] = json!(mode);
        }
        self.post("/api/threads", body).await
    }

    /// 把某个目录设为当前工作区（`workspace.add`：设 active project + 记入会话目录索引）。
    ///
    /// 与 desktop-ui 的 `setActiveProject` 同源：它也是"先记工作区、再新建会话"。
    pub async fn add_workspace(&self, path: &str) -> Result<Value> {
        self.rpc("workspace.add", json!({ "path": path })).await
    }

    /// 系统根目录（磁盘 / 主目录 / 当前项目），供工作区选择器使用。
    pub async fn fs_roots(&self) -> Result<Vec<Value>> {
        let value = self.get("/api/fs/roots").await?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    /// 列出目录内容（不过滤：由调用方按 `isDir` 决定展示什么）。
    pub async fn fs_list(&self, path: &str) -> Result<Value> {
        self.get_with("/api/fs/list", &[("path", path.to_string())])
            .await
    }

    /// 新建目录（工作区选择器里的"新建文件夹"）。
    pub async fn fs_mkdir(&self, path: &str) -> Result<Value> {
        self.post("/api/fs/mkdir", json!({ "path": path })).await
    }

    pub async fn send_message(&self, thread_id: &str, text: &str) -> Result<Value> {
        self.rpc(
            "thread.send",
            json!({ "threadId": thread_id, "text": text }),
        )
        .await
    }

    /// 聚焦会话（`thread.focus`）：设为激活会话并打开它的 Tab。
    ///
    /// 快照只给"激活 / 打开的 Tab / 运行中 / 子智能体"会话带 items，所以从侧栏
    /// 打开一条历史会话前必须先聚焦，否则正文是空的。
    pub async fn focus_thread(&self, thread_id: &str) -> Result<Value> {        self.rpc("thread.focus", json!({ "threadId": thread_id }))
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

    /// 回答智能体的提问（`ask_user`）。
    ///
    /// `params` 由 [`crate::questions::answer_params_from_form`] 产出（choice /
    /// choices / text 或多题 answers），`callId` 从动作路由的 path 里补进来。
    pub async fn answer_question(&self, call_id: &str, mut params: Value) -> Result<Value> {
        if let Some(obj) = params.as_object_mut() {
            obj.insert("callId".to_string(), json!(call_id));
        }
        self.rpc("question.answer", params).await
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

    /// 重命名会话（`thread.update` 只改 title 字段）。
    pub async fn rename_thread(&self, thread_id: &str, title: &str) -> Result<Value> {
        self.rpc(
            "thread.update",
            json!({ "threadId": thread_id, "title": title }),
        )
        .await
    }

    /// 删除会话（后端会连带删除该会话的落盘文件，动作前必须有用户确认）。
    pub async fn delete_thread(&self, thread_id: &str) -> Result<Value> {
        self.rpc("thread.delete", json!({ "threadId": thread_id }))
            .await
    }

    pub async fn reload_agent_plugins(&self) -> Result<Value> {
        self.rpc("plugin.reload", json!({ "workspace": self.workspace }))
            .await
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
    /// 与 [`AgentClient::rpc`] 的区别：这条连接是**长期持有**的（页面开着就一直连着），
    /// 而且只读通知、不注册任何界面状态。`state.patch` 能力必须在这里声明：
    /// 宿主只有收到声明才会把后续帧切成"只带变过的 item"的增量帧。
    ///
    /// 连接断开时函数返回，调用方据此结束 SSE 流；浏览器 `EventSource` 会自动重连，
    /// 重连后重新走一遍首帧快照，界面因此能自愈。
    pub async fn pump_events(&self, tx: mpsc::Sender<Value>) -> Result<()> {
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

    /// 拉取一条会话的**完整 item 列表**（按需分页）。
    ///
    /// 为什么不用 `/api/snapshot`：快照只对"激活 / 打开的 Tab / 运行中 / 子智能体"会话
    /// 带 items。从 ssr-h 打开一个非激活会话时，快照里它是空的——首屏与事件流都需要
    /// 真实内容，这里按 `GET /api/threads/{id}/items` 翻页取全。
    ///
    /// 走 HTTP 而不是 WS 的 `thread.items.page`：每条 WS 短连接都会让 agent 先推一份
    /// 全量种子快照（长会话上 MB），只为取一页条目并不划算。
    pub async fn thread_items(&self, thread_id: &str) -> Result<Vec<Value>> {
        // 会话 id 来自 URL 路径：只允许安全字符，避免拼出一条指向别处的请求。
        if thread_id.is_empty()
            || !thread_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            bail!("非法会话 id: {thread_id}");
        }
        let path = format!("/api/threads/{thread_id}/items");
        let mut out: Vec<Value> = Vec::new();
        loop {
            let page = self
                .get_with(
                    &path,
                    &[
                        ("from", out.len().to_string()),
                        ("count", "500".to_string()),
                    ],
                )
                .await?;
            let items = page
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = page.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
            let empty = items.is_empty();
            out.extend(items);
            if out.len() >= total || out.len() >= 5000 || empty {
                break;
            }
        }
        Ok(out)
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
