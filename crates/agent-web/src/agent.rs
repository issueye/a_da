//! 网关联调客户端：所有处理器只通过它访问 agent。
//!
//! 具体怎么到达网关由 [`WebBackend`] 决定：
//!
//! - 独立运行时用 [`HttpBackend`]（HTTP `/api/*` + 短/长 WebSocket）；
//! - 被网关挂载时用网关进程内的 `GatewayBackend`（不经过网络回环）。
//!
//! 这样界面层不复制任何 agent 状态，也不依赖桌面端内部实现。

use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::backend::{HttpBackend, WebBackend};

#[derive(Clone)]
pub struct AgentClient {
    backend: Arc<dyn WebBackend>,
    pub workspace: String,
}

impl AgentClient {
    /// 独立运行：HTTP + WebSocket 连网关。
    pub fn new(
        base: impl Into<String>,
        token: impl Into<String>,
        workspace: impl Into<String>,
    ) -> Self {
        let workspace = workspace.into();
        Self::with_backend(
            Arc::new(HttpBackend::new(base, token, workspace.clone())),
            workspace,
        )
    }

    /// 注入任意后端（网关进程内实现 / 测试桩）。
    pub fn with_backend(backend: Arc<dyn WebBackend>, workspace: impl Into<String>) -> Self {
        Self {
            backend,
            workspace: workspace.into(),
        }
    }

    pub fn base_url(&self) -> &str {
        self.backend.base_url()
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.backend.get(path, &[]).await
    }

    /// 带额外 query 的 GET（`workspace` 由后端统一附加）。
    async fn get_with(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        self.backend.get(path, params).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.backend.post(path, body).await
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        self.backend.rpc(method, params).await
    }

    /// 调用后端插件声明的界面动作（`plugin.invoke`）。
    ///
    /// `intent` 由调用方声明：只读预取/GET 用 `read`，表单/POST 用 `write`。
    /// 真正的只读/写入判定在后端插件里，前端改标也绕不过去。
    pub async fn invoke_plugin_action(
        &self,
        plugin_id: &str,
        action: &str,
        input: Value,
        intent: &str,
    ) -> Result<Value> {
        self.rpc(
            "plugin.invoke",
            json!({
                "pluginId": plugin_id,
                "action": action,
                "input": input,
                "intent": intent,
            }),
        )
        .await
    }

    /// 把一次 HTTP 请求交给插件自己声明的动态路由（`plugin.route`）。
    ///
    /// 路由表在 TS 侧；这里只负责带上请求上下文。`bodyKind` 决定插件拿到的
    /// `body` 是 JSON / 表单 / 文本还是 base64。
    #[allow(clippy::too_many_arguments)]
    pub async fn plugin_route(
        &self,
        plugin_id: &str,
        method: &str,
        path: &str,
        query: Value,
        headers: Value,
        body: Value,
        body_kind: &str,
        thread_id: Option<&str>,
    ) -> Result<Value> {
        let mut params = json!({
            "pluginId": plugin_id,
            "method": method,
            "path": path,
            "query": query,
            "headers": headers,
            "body": body,
            "bodyKind": body_kind,
            "workspace": self.workspace,
        });
        if let Some(thread_id) = thread_id.map(str::trim).filter(|value| !value.is_empty()) {
            params["threadId"] = json!(thread_id);
        }
        self.rpc("plugin.route", params).await
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

    /// 网关自身状态（`gateway.status`）。
    pub async fn gateway_status(&self) -> Result<Value> {
        self.get("/api/gateway/status").await
    }

    /// 网关管理的 agent 实例（`gateway.listAgents`，端点已剥 token）。
    pub async fn gateway_agents(&self) -> Result<Value> {
        self.get("/api/gateway/agents").await
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
    /// 与 原桌面端 的设置项同源：网关落在 `config.json` 的 `effort` 并广播快照，
    /// 不在本进程里留第二份状态。
    pub async fn set_effort(&self, effort: &str) -> Result<Value> {
        self.rpc("config.setEffort", json!({ "effort": effort }))
            .await
    }

    /// 切换审批档位（`config.setApproval`：auto / ask / readonly / full）。
    ///
    /// 后端会落盘并广播——快照的 `config.approval` 变化会让 parts 重发，
    /// 输入框的档位弹层随之更新。
    pub async fn set_approval_mode(&self, mode: &str) -> Result<Value> {
        self.rpc("config.setApproval", json!({ "mode": mode }))
            .await
    }

    /// agent 插件列表（网关 `/api/plugins` → `plugin.list`）。
    ///
    /// 注意与前端插件自省端点（`/web-ui/frontend-plugins`）不是一回事。
    pub async fn plugins(&self) -> Result<Value> {
        self.get("/api/plugins").await
    }

    pub async fn create_thread(&self, title: &str) -> Result<Value> {
        self.create_thread_in(title, &self.workspace, None).await
    }

    /// 切换会话模式（`thread.setMode`：code / plan / create / pm）。
    ///
    /// 🔴 只允许**空会话**：网关对已开始的会话会拒绝并给出原因
    /// （模式决定 system prompt 与工具集，中途换等于把一段对话劈成两个智能体）。
    pub async fn set_thread_mode(&self, thread_id: &str, mode: &str) -> Result<Value> {
        self.rpc(
            "thread.setMode",
            json!({ "threadId": thread_id, "mode": mode }),
        )
        .await
    }

    /// 把一条会话绑定到指定工作区（`thread.setWorkspace`：只改会话属性）。
    ///
    /// 与 `/workspaces/select` 的 `workspace.add` 不同：**不切换** agent 的当前项目，
    /// 也不会因为"该工作区没有会话"再多建一条。
    pub async fn set_thread_workspace(&self, thread_id: &str, workspace: &str) -> Result<Value> {
        self.rpc(
            "thread.setWorkspace",
            json!({ "threadId": thread_id, "workspace": workspace }),
        )
        .await
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
    /// 与 原桌面端 的 `setActiveProject` 同源：它也是"先记工作区、再新建会话"。
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

    /// 发送一条用户消息。`images` 是 **data URI**（`data:image/png;base64,…`）；
    /// 网关的 `thread.send` 原样接收，并交给引擎按协议编成图片内容块。
    pub async fn send_message(
        &self,
        thread_id: &str,
        text: &str,
        images: &[String],
    ) -> Result<Value> {
        let mut params = json!({ "threadId": thread_id, "text": text });
        if !images.is_empty() {
            params["images"] = json!(images);
        }
        self.rpc("thread.send", params).await
    }

    /// 聚焦会话（`thread.focus`）：设为激活会话并打开它的 Tab。
    ///
    /// 快照只给"激活 / 打开的 Tab / 运行中 / 子智能体"会话带 items，所以从侧栏
    /// 打开一条历史会话前必须先聚焦，否则正文是空的。
    pub async fn focus_thread(&self, thread_id: &str) -> Result<Value> {
        self.rpc("thread.focus", json!({ "threadId": thread_id }))
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

    /// 当前会话的后台任务（`task.list`）；`thread_id` 为空时返回工作区全部任务。
    pub async fn tasks(&self, thread_id: Option<&str>) -> Result<Value> {
        let params = match thread_id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(thread_id) => json!({ "threadId": thread_id }),
            None => json!({}),
        };
        self.rpc("task.list", params).await
    }

    /// 终止后台任务（`task.kill`）。
    pub async fn kill_task(&self, task_id: &str) -> Result<Value> {
        self.rpc("task.kill", json!({ "taskId": task_id })).await
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

    /// 订阅网关状态事件（`evt.state.snapshot` / `evt.state.patch`）。
    ///
    /// 连接断开时函数返回，调用方据此结束 SSE 流；浏览器 `EventSource` 会自动重连，
    /// 重连后重新走一遍首帧快照，界面因此能自愈。
    pub async fn pump_events(&self, tx: mpsc::Sender<Value>) -> Result<()> {
        self.backend.subscribe(tx).await
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
