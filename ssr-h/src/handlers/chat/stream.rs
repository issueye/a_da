//! 对话流的服务端推送（SSE）。
//!
//! # 为什么要 SSE，而不是继续每 2 秒拉一次
//!
//! 轮询把"模型逐字输出"打包成 2 秒一块的整段替换：用户看到的是**一大段一大段**
//! 跳到界面上，既看不到"正在写"，也会在每次替换时丢掉展开状态与阅读位置。
//! 这里改成：服务端持有一条到网关的长连接，订阅 `evt.state.patch`（宿主只推
//! **变过的 item**），把变化的 item 渲染成 HTML 后经 SSE 推给浏览器；
//! 浏览器按 `data-item-id` 原位替换、按 `data-at` 插入新卡片。
//!
//! # 三条硬约束
//!
//! 1. **不复制状态**：这里只保留"为了渲染这一段对话"所需的投影（meta + items），
//!    没有第二份会话事实；`meta` 每次以宿主推来的为准，items 只在流式期间缓冲。
//! 2. **非投影会话的 items 是空的**：快照只给激活 / 打开的 Tab / 运行中 / 子智能体会话
//!    带 items，所以**不能**拿"空 items"当"会话被清空"——否则切个 active 会话就会把
//!    界面清屏。只有会话处于投影态时，patch 的 upsert/remove 才被采信；初次内容走
//!    `thread.items.page` 单独取全。
//! 3. **整段替换只发生在结构变化时**：新增/删除卡片才重发 transcript；
//!    纯文本增长只发变化的那一条，避免每帧重渲染整段会话（长会话会很贵）。

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{array_at, item_row, live_parts, str_at, transcript_html, AppState, ItemRow};

/// 合帧窗口：模型 token 的到达频率远高于一次 Markdown 重渲染的成本。
/// 40ms（25FPS）已经比"逐字"更细，同时把长回答的重渲染次数压下来。
const RENDER_TICK: Duration = Duration::from_millis(40);

/// 托盘 / 任务规划 / 顶栏 / 辅助区的最小重渲染间隔。
///
/// 这些区域不参与"逐字"观感，但插件插槽是服务端 TSX 渲染（每次要建 Boa Context），
/// 跟着 25FPS 一起刷会白烧 CPU。
const PARTS_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// 没有事件时也要定期刷一次：审批托盘里的"已等待 Ns"要能自己走。
const PARTS_HEARTBEAT: Duration = Duration::from_secs(5);

/// 轮次收尾后补拉条目的延迟。
///
/// 宿主把 `running` 置回 false 与把本轮 `usage` 写到助手卡上几乎同时发生，
/// 但增量帧的可见指纹不含 usage；太早拉会拿到"还没有用量"的条目，
/// 之后也不会再有 patch 来纠正。等一拍再拉，用量才稳定在卡片上。
const FINISH_RESYNC_DELAY: Duration = Duration::from_millis(220);

/// SSE 保活间隔：中间有反代/浏览器空闲超时，定期发一个注释帧。
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// `GET /threads/{id}/stream`：把这条会话的增量变化推给浏览器。
pub async fn live_stream(State(state): State<AppState>, Path(thread_id): Path<String>) -> Response {
    let (tx, rx) = mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        if let Err(error) = run_stream(state, thread_id, tx).await {
            // 页面关掉是常态，别把它记成 warn；真正的连接问题由 `pump_events` 记。
            tracing::debug!("live 事件流结束: {error:#}");
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<Event, Infallible>(event), rx))
    });
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response();
    // 事件流绝不能被缓存：重放旧帧会让界面"倒带"。
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn run_stream(
    state: AppState,
    thread_id: String,
    tx: mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    // 先订阅、再取初始状态：这中间产生的 patch 会排在通道里，
    // 初始快照的 seq 之后逐帧应用，不会漏掉。
    let (frames_tx, mut frames_rx) = mpsc::channel::<Value>(256);
    let client = state.agent.clone();
    let pump = tokio::spawn(async move {
        if let Err(error) = client.pump_events(frames_tx).await {
            tracing::debug!("订阅网关事件结束: {error:#}");
        }
    });

    let mut view = StreamView::new(thread_id.clone());
    if let Ok(snapshot) = state.agent.snapshot().await {
        view.apply_snapshot(&snapshot);
    }
    match state.agent.thread_items(&thread_id).await {
        Ok(items) => {
            view.replace_items(items);
        }
        Err(error) => {
            // 会话还没落盘（刚建的空会话）或网关暂时不可用：先用快照里已有的 items。
            tracing::debug!("读取会话条目失败，退回快照: {error:#}");
        }
    }

    let outcome = stream_loop(&state, &mut view, &tx, &mut frames_rx).await;
    pump.abort();
    outcome
}

async fn stream_loop(
    state: &AppState,
    view: &mut StreamView,
    tx: &mpsc::Sender<Event>,
    frames: &mut mpsc::Receiver<Value>,
) -> anyhow::Result<()> {
    // 连上就先整段同步一次：页面渲染与 SSE 连接之间可能有几百毫秒的间隔，
    // 期间产生的变化（例如刚发出的用户消息）不能凭空消失。
    let mut structural = true;
    let mut dirty: BTreeSet<String> = BTreeSet::new();
    let mut parts = true;
    let mut resync = false;
    let mut resync_at: Option<Instant> = None;
    let mut last_parts = Instant::now() - PARTS_MIN_INTERVAL;
    let mut last_heartbeat = Instant::now();

    let mut ticker = tokio::time::interval(RENDER_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            frame = frames.recv() => {
                let Some(frame) = frame else { break };
                let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
                let params = frame.get("params").cloned().unwrap_or(Value::Null);
                match method {
                    "evt.state.snapshot" => {
                        // 全量帧（首帧种子 / 未声明增量时的广播）：以它为准重投影。
                        if view.apply_snapshot(params.get("payload").unwrap_or(&Value::Null)) {
                            structural = true;
                        }
                        parts = true;
                    }
                    "evt.state.patch" => {
                        let change = view.apply_patch(&params);
                        if change.structural {
                            structural = true;
                            parts = true;
                        }
                        if change.parts {
                            parts = true;
                        }
                        if change.resync {
                            resync = true;
                        }
                        if change.resync_after_finish {
                            resync_at.get_or_insert(Instant::now() + FINISH_RESYNC_DELAY);
                        }
                        dirty.extend(change.changed);
                    }
                    _ => {}
                }
            }
            // 浏览器关掉页面时立刻收工：否则要等下一次推送（最多 5 秒心跳）
            // 才会发现接收端已经没了，期间白白占着网关连接。
            _ = tx.closed() => break,
            _ = ticker.tick() => {}
        }

        if last_heartbeat.elapsed() >= PARTS_HEARTBEAT {
            parts = true;
            last_heartbeat = Instant::now();
        }

        let resync_due =
            resync || resync_at.map(|at| Instant::now() >= at).unwrap_or(false);
        if resync_due {
            // 进入投影态（立即）或轮次收尾（延迟一拍）：补齐完整 item 集合。
            resync = false;
            resync_at = None;
            match state.agent.thread_items(&view.thread_id).await {
                Ok(items) => {
                    view.replace_items(items);
                    structural = true;
                    parts = true;
                }
                Err(error) => tracing::debug!("重新拉取会话条目失败: {error:#}"),
            }
        }

        if structural {
            // 结构变化（新增/删除卡片）：整段重发。条目身份由 data-item-id 表达，
            // 客户端替换后展开状态仍按 data-key 恢复（components.js）。
            //
            // 还没有可信 items（初连取数失败）时**不发**空 transcript，也不清 dirty：
            // 客户端会按 `at` 把随后的 item 插进去，等拿到权威 items 再整段对齐。
            if let Some(html) = view.render_transcript() {
                send(tx, Event::default().event("transcript").data(html)).await?;
                dirty.clear();
            }
            structural = false;
            parts = true;
        } else if !dirty.is_empty() {
            let ids: Vec<String> = dirty.iter().cloned().collect();
            dirty.clear();
            for id in ids {
                match view.item(&id) {
                    Some(item) => {
                        let html = item_row(item, &view.thread_id).html;
                        send(tx, Event::default().event("item").data(html)).await?;
                    }
                    None => {
                        // 条目已经不在了（patch 只发了文本更新、却在别处被删）：
                        // 下一拍整段重发自愈，避免客户端留下幽灵卡片。
                        structural = true;
                    }
                }
            }
        }

        if parts && last_parts.elapsed() >= PARTS_MIN_INTERVAL {
            if let Some(payload) = render_parts(state, view).await {
                send(tx, Event::default().event("parts").data(payload)).await?;
            }
            parts = false;
            last_parts = Instant::now();
        }
    }

    Ok(())
}

async fn send(tx: &mpsc::Sender<Event>, event: Event) -> anyhow::Result<()> {
    tx.send(event)
        .await
        .map_err(|_| anyhow::anyhow!("页面已断开"))
}

/// 对话流之外的动态区域（与 `/threads/{id}/live` 的 OOB 片段一一对应）。
async fn render_parts(state: &AppState, view: &StreamView) -> Option<String> {
    let snapshot = view.render_parts_snapshot()?;
    let thread = array_at(&snapshot, "threads")
        .iter()
        .find(|thread| str_at(thread, "id") == view.thread_id)?
        .clone();
    let parts = live_parts(state, &snapshot, &view.thread_id, &thread).await;
    Some(
        json!({
            "trays": parts.trays,
            "corner": parts.corner,
            "topbarStatus": parts.topbar_status,
            "topbarActions": parts.topbar_actions,
            "aux": parts.aux,
            "above": parts.composer_above,
            "approval": parts.approval_chip,
            "model": parts.model_chip,
        })
        .to_string(),
    )
}

/// 一条会话的渲染投影：`meta`（快照去掉 items）+ 当前 items。
///
/// 仅用于渲染，不是第二份事实来源：`meta` 每帧以宿主推来的为准，
/// items 只在流式期间按 patch 合并。进程重启/页面刷新后一切重新建立。
pub(super) struct StreamView {
    pub(super) thread_id: String,
    /// 已经应用过的状态版本（宿主 `seq`）：旧帧直接丢弃，防止乱序回退。
    seq: u64,
    meta: Value,
    items: Vec<Value>,
    /// 是否已经拿到可信的 items 全集。没有它就不许发整段 transcript——
    /// 否则"还没连上/取数失败"会被渲染成"会话被清空"。
    ready: bool,
    /// 参与动态区域渲染的字段指纹（不含 `log` 等高频噪声）。
    parts_fp: String,
}

#[derive(Debug, Default)]
struct PatchChange {
    structural: bool,
    parts: bool,
    changed: Vec<String>,
    /// 立即补拉完整条目（刚进入投影态）
    resync: bool,
    /// 轮次刚结束：等一拍再补拉（用量/耗时这时才写进卡片）
    resync_after_finish: bool,
}

impl StreamView {
    fn new(thread_id: String) -> Self {
        Self {
            thread_id,
            seq: 0,
            meta: Value::Null,
            items: Vec::new(),
            ready: false,
            parts_fp: String::new(),
        }
    }

    /// 应用一份全量快照。返回 `true` 表示 items 被替换（需要整段重发）。
    fn apply_snapshot(&mut self, snapshot: &Value) -> bool {
        let seq = snapshot.get("seq").and_then(Value::as_u64).unwrap_or(0);
        if seq < self.seq {
            return false;
        }
        self.seq = seq;

        let thread_items = snapshot
            .get("threads")
            .and_then(Value::as_array)
            .and_then(|threads| {
                threads
                    .iter()
                    .find(|thread| str_at(thread, "id") == self.thread_id)
            })
            .and_then(|thread| thread.get("items"))
            .and_then(Value::as_array)
            .cloned();

        self.meta = without_items(snapshot);
        self.parts_fp = parts_fingerprint(&self.meta);
        if !self.projected() {
            // 非投影会话快照里的 items 是**投影裁剪**的结果，不是事实：
            // 保留已有内容，等下一份能投影的快照或 resync。
            return false;
        }
        if let Some(items) = thread_items {
            self.items = items;
            self.sort_items();
        }
        self.ready = true;
        true
    }

    /// 应用一帧增量。只有投影态会话的 item 变化会被采信。
    fn apply_patch(&mut self, params: &Value) -> PatchChange {
        let mut change = PatchChange::default();
        let seq = params.get("seq").and_then(Value::as_u64).unwrap_or(0);
        if seq != 0 {
            if seq <= self.seq {
                return change; // 重复/旧帧：增量帧可能因为合帧窗口被重发
            }
            self.seq = seq;
        }

        let was_projected = self.projected();
        let was_running = self.running();
        if let Some(meta) = params.get("meta").filter(|meta| meta.is_object()) {
            let meta = without_items(meta);
            let fp = parts_fingerprint(&meta);
            if fp != self.parts_fp {
                change.parts = true;
                self.parts_fp = fp;
            }
            self.meta = meta;
        }
        let projected = self.projected();
        let running = self.running();
        if projected && !was_projected {
            // 开始运行时，先把完整 item 集合补一次；本帧的 upsert 也会被应用，
            // 但历史卡片可能没有被重新推送（版本表在离开投影时被清过）。
            change.resync = true;
        }
        if was_running && !running {
            // 轮次收尾：`usage` 不会进 patch（`item_visible_key` 不含它），
            // 等一拍再重新拉条目，Token 天气这类插件才能拿到真实用量。
            change.resync_after_finish = true;
        }
        if !projected {
            return change;
        }

        if let Some(upserted) = params.get("upserted").and_then(Value::as_array) {
            let mut added = false;
            for entry in upserted {
                if str_at(entry, "threadId") != self.thread_id {
                    continue;
                }
                let Some(item) = entry.get("item") else {
                    continue;
                };
                let id = str_at(item, "id").to_string();
                if id.is_empty() {
                    continue;
                }
                match self.position(&id) {
                    Some(index) => self.items[index] = item.clone(),
                    None => {
                        self.items.push(item.clone());
                        added = true;
                        change.structural = true;
                    }
                }
                let kind = str_at(item, "kind");
                if kind == "assistant" {
                    // 用量（`usage.promptTokens`）只在轮次收尾时落到助手卡上；
                    // Token 天气这类插件靠它刷新，不能等到 5 秒心跳。
                    if item.get("usage").map(Value::is_object).unwrap_or(false) {
                        change.parts = true;
                    }
                } else if kind != "thinking" {
                    // 工具/提示/压缩卡承载状态与托盘：它们的变化要重渲染动态区域。
                    change.parts = true;
                }
                change.changed.push(id);
            }
            if added {
                self.sort_items();
            }
        }

        if let Some(removed) = params.get("removed").and_then(Value::as_array) {
            for entry in removed {
                if str_at(entry, "threadId") != self.thread_id {
                    continue;
                }
                let id = str_at(entry, "itemId");
                if self.remove(id) {
                    change.structural = true;
                    change.parts = true;
                }
            }
        }

        change
    }

    /// 用一份权威的 items 集合替换当前投影（初连 / resync）。
    pub(super) fn replace_items(&mut self, items: Vec<Value>) {
        self.items = items;
        self.sort_items();
        self.ready = true;
    }

    fn item(&self, id: &str) -> Option<&Value> {
        self.items.iter().find(|item| str_at(item, "id") == id)
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|item| str_at(item, "id") == id)
    }

    fn remove(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(index) => {
                self.items.remove(index);
                true
            }
            None => false,
        }
    }

    /// 按 `at` 排序：宿主保证同一会话内卡片 `at` 严格递增，
    /// 客户端插入新卡片时也用同一个键定位。
    fn sort_items(&mut self) {
        self.items.sort_by_key(item_at);
    }

    /// 当前会话是否正在执行一轮（`runningThreadIds`）。
    fn running(&self) -> bool {
        array_at(&self.meta, "runningThreadIds")
            .iter()
            .any(|id| id.as_str() == Some(self.thread_id.as_str()))
    }

    /// 当前会话是否处于"快照会带完整 items"的状态。
    fn projected(&self) -> bool {        let meta = &self.meta;
        if meta.get("activeThreadId").and_then(Value::as_str) == Some(self.thread_id.as_str()) {
            return true;
        }
        for key in ["runningThreadIds", "waitingThreadIds"] {
            if array_at(meta, key)
                .iter()
                .any(|id| id.as_str() == Some(self.thread_id.as_str()))
            {
                return true;
            }
        }
        if let Some(ui) = meta.get("ui") {
            if array_at(ui, "openTabIds")
                .iter()
                .any(|id| id.as_str() == Some(self.thread_id.as_str()))
            {
                return true;
            }
        }
        array_at(meta, "threads").iter().any(|thread| {
            str_at(thread, "id") == self.thread_id
                && thread.get("isSubagent").and_then(Value::as_bool) == Some(true)
        })
    }

    /// 给 `live_parts` 用的快照：meta 原样，目标会话只放**工具卡 + 助手用量**。
    ///
    /// 托盘只关心 `waiting_approval` 的工具、任务规划只关心 `todo` 工具；
    /// 插件（Token 天气）的 `props.data.tokens` 读助手卡的 `usage` 与 `durationMs`
    /// （缓存命中、tok/s 都从这里来）。没必要把整段对话（长会话上千条、单体几十 KB）
    /// 每 500ms 克隆一遍，但**不能只留工具卡**——那会让用量插件读到 0，界面显示"晴天 0%"。
    fn render_parts_snapshot(&self) -> Option<Value> {
        if !self.meta.is_object() {
            return None;
        }
        let mut snapshot = self.meta.clone();
        let threads = snapshot.get_mut("threads")?.as_array_mut()?;
        let thread = threads
            .iter_mut()
            .find(|thread| str_at(thread, "id") == self.thread_id)?;
        // 标题先按完整 items 解析好：投影只带工具卡 + 助手用量，之后就没法
        // 再从"首条用户消息"兜底了（否则插件浮窗会显示"新会话"）。
        let resolved_title = super::super::title_from_items(str_at(thread, "title"), &self.items);
        thread["title"] = json!(resolved_title);
        let mut items: Vec<Value> = Vec::new();
        for item in &self.items {
            match str_at(item, "kind") {
                "tool" => items.push(item.clone()),
                "assistant" => {
                    // 只投影用量与耗时：正文/思考文本不需要进入这里。
                    if let Some(usage) = item.get("usage").filter(|usage| usage.is_object()) {
                        let mut projected = json!({
                            "kind": "assistant",
                            "usage": usage,
                        });
                        if let Some(duration) = item.get("durationMs").and_then(Value::as_u64) {
                            projected["durationMs"] = json!(duration);
                        }
                        items.push(projected);
                    }
                }
                _ => {}
            }
        }
        thread["items"] = Value::Array(items);
        Some(snapshot)
    }

    fn render_transcript(&self) -> Option<String> {
        if !self.ready {
            return None;
        }
        let items: Vec<ItemRow> = self
            .items
            .iter()
            .map(|item| item_row(item, &self.thread_id))
            .collect();
        Some(transcript_html(items))
    }
}

fn item_at(item: &Value) -> u64 {
    item.get("at").and_then(Value::as_u64).unwrap_or(0)
}

/// 快照去掉各会话的 items（`meta` 的形状与快照同形，只是不背历史内容）。
fn without_items(snapshot: &Value) -> Value {
    let mut meta = snapshot.clone();
    if let Some(threads) = meta.get_mut("threads").and_then(Value::as_array_mut) {
        for thread in threads.iter_mut() {
            if let Some(object) = thread.as_object_mut() {
                object.insert("items".to_string(), Value::Array(Vec::new()));
            }
        }
    }
    meta
}

/// 动态区域只关心这些字段：`log` / `ui.pendingDraft` 之类每帧都在变，
/// 拿整份 meta 比对会让托盘、任务规划、插件插槽跟着 25FPS 重渲染。
fn parts_fingerprint(meta: &Value) -> String {
    let threads: Vec<Value> = array_at(meta, "threads")
        .iter()
        .map(|thread| {
            json!({
                "id": thread.get("id"),
                "title": thread.get("title"),
                "mode": thread.get("mode"),
                "workspace": thread.get("workspace"),
                "isSubagent": thread.get("isSubagent"),
                "parentId": thread.get("parentId"),
            })
        })
        .collect();
    json!({
        "active": meta.get("activeThreadId"),
        "running": meta.get("runningThreadIds"),
        "waiting": meta.get("waitingThreadIds"),
        "queue": meta.get("queue"),
        "config": meta.get("config"),
        "pending": meta.get("pendingQuestions"),
        "openTabs": meta.get("ui").and_then(|ui| ui.get("openTabIds")),
        "threads": threads,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(active: &str, thread_id: &str, items: Value) -> Value {
        json!({
            "seq": 1,
            "activeThreadId": active,
            "runningThreadIds": [],
            "waitingThreadIds": [],
            "queue": [],
            "config": {"model": "m", "approval": "auto"},
            "ui": {"openTabIds": []},
            "threads": [
                {"id": "t1", "title": "会话一", "workspace": "E:/w", "items": if thread_id == "t1" { items.clone() } else { json!([]) }},
                {"id": "t2", "title": "会话二", "workspace": "E:/w", "items": if thread_id == "t2" { items.clone() } else { json!([]) }},
            ],
        })
    }

    fn assistant(id: &str, at: u64, text: &str) -> Value {
        json!({"id": id, "kind": "assistant", "at": at, "text": text})
    }

    fn tool(id: &str, at: u64, status: &str) -> Value {
        json!({"id": id, "kind": "tool", "at": at, "name": "read_file", "status": status, "callId": id})
    }

    fn patch(seq: u64, meta: Value, upserted: Value, removed: Value) -> Value {
        json!({"seq": seq, "baseSeq": seq - 1, "meta": meta, "upserted": upserted, "removed": removed})
    }

    fn meta(active: &str) -> Value {
        let mut snap = snapshot(active, "", json!([]));
        snap.as_object_mut().unwrap().remove("seq");
        snap
    }

    #[test]
    fn snapshot_seeds_items_and_strips_them_from_meta() {
        let mut view = StreamView::new("t1".to_string());
        let snap = snapshot("t1", "t1", json!([assistant("a1", 10, "你好")]));
        assert!(view.apply_snapshot(&snap));
        assert_eq!(view.items.len(), 1);
        assert!(view.render_transcript().unwrap().contains("你好"));
        // meta 里不再背 items（否则每帧重渲染托盘会顺带序列化整段历史）
        assert_eq!(view.meta["threads"][0]["items"], json!([]));
    }

    #[test]
    fn text_delta_is_an_item_update_not_a_full_transcript_rewrite() {
        let mut view = StreamView::new("t1".to_string());
        view.replace_items(vec![assistant("a1", 10, "你")]);
        let change = view.apply_patch(&patch(
            2,
            meta("t1"),
            json!([{"threadId": "t1", "item": assistant("a1", 10, "你好")}]),
            json!([]),
        ));
        assert!(!change.structural, "纯文本增长不应整段重发");
        assert_eq!(change.changed, vec!["a1".to_string()]);
    }

    #[test]
    fn new_card_is_structural_and_ordered_by_at() {
        let mut view = StreamView::new("t1".to_string());
        view.replace_items(vec![assistant("a1", 10, "一")]);
        let change = view.apply_patch(&patch(
            2,
            meta("t1"),
            json!([
                {"threadId": "t1", "item": assistant("a3", 30, "三")},
                {"threadId": "t1", "item": tool("a2", 20, "running")},
            ]),
            json!([]),
        ));
        assert!(change.structural);
        assert!(change.parts);
        let ids: Vec<&str> = view.items.iter().map(|i| str_at(i, "id")).collect();
        assert_eq!(ids, vec!["a1", "a2", "a3"]);
    }

    #[test]
    fn removing_a_card_is_structural() {
        let mut view = StreamView::new("t1".to_string());
        view.replace_items(vec![assistant("a1", 10, "一")]);
        let change = view.apply_patch(&patch(
            2,
            meta("t1"),
            json!([]),
            json!([{"threadId": "t1", "itemId": "a1"}]),
        ));
        assert!(change.structural);
        assert!(view.items.is_empty());
    }

    /// 切换 active 会话时，非投影会话的 items 不会出现在 `current` 里，
    /// 宿主会把它算成 removed——这不是"卡片被删"，不能拿它清屏。
    #[test]
    fn removals_are_ignored_while_the_thread_is_not_projected() {
        let mut view = StreamView::new("t1".to_string());
        view.replace_items(vec![assistant("a1", 10, "一")]);
        let change = view.apply_patch(&patch(
            2,
            meta("t2"),
            json!([]),
            json!([{"threadId": "t1", "itemId": "a1"}]),
        ));
        assert!(!change.structural);
        assert_eq!(view.items.len(), 1, "非投影会话的历史必须原样保留");
    }

    #[test]
    fn reentering_projection_asks_for_a_resync() {
        let mut view = StreamView::new("t1".to_string());
        view.replace_items(vec![assistant("a1", 10, "一")]);
        // 先被切走
        let _ = view.apply_patch(&patch(2, meta("t2"), json!([]), json!([])));
        // 再切回来（同时开始运行）
        let mut back = meta("t1");
        back["runningThreadIds"] = json!(["t1"]);
        let change = view.apply_patch(&patch(3, back, json!([]), json!([])));
        assert!(change.resync, "重新进入投影态必须先拉一次完整条目");
    }

    #[test]
    fn stale_frames_are_dropped() {
        let mut view = StreamView::new("t1".to_string());
        let _ = view.apply_patch(&patch(5, meta("t1"), json!([]), json!([])));
        let stale = view.apply_patch(&patch(4, meta("t1"), json!([]), json!([])));
        assert!(!stale.structural && stale.changed.is_empty());
    }

    #[test]
    fn debug_log_churn_does_not_rerender_parts_but_queue_does() {
        let mut view = StreamView::new("t1".to_string());
        let base = meta("t1");
        let _ = view.apply_patch(&patch(2, base.clone(), json!([]), json!([])));

        let mut noisy = base.clone();
        noisy["log"] = json!([{"id": 1, "text": "每帧都在变"}]);
        let change = view.apply_patch(&patch(3, noisy, json!([]), json!([])));
        assert!(!change.parts, "调试日志不该触发托盘/插件区重渲染");

        let mut queued = base.clone();
        queued["queue"] = json!([{"id": "q1", "threadId": "t1", "text": "排队"}]);
        let change = view.apply_patch(&patch(4, queued, json!([]), json!([])));
        assert!(change.parts, "队列变化必须刷新排队托盘");
    }

    #[test]
    fn transcript_is_not_sent_before_any_authoritative_items() {
        let mut view = StreamView::new("t1".to_string());
        // 非投影会话的快照没有 items：这不是"空会话"，别发一段空 transcript 清屏。
        let snap = snapshot("t2", "", json!([]));
        assert!(!view.apply_snapshot(&snap));
        assert!(view.render_transcript().is_none());
    }

    #[test]
    fn parts_snapshot_keeps_assistant_usage_but_not_its_text() {
        let mut view = StreamView::new("t1".to_string());
        // 先用一份快照建立 meta（没有 meta 就没有可投影的会话）。
        view.apply_snapshot(&snapshot("t1", "t1", json!([])));
        view.replace_items(vec![
            json!({
                "id": "a1", "kind": "assistant", "at": 10,
                "text": "很长的一段正文，不该被 500ms 一次的托盘渲染反复克隆",
                "usage": {"promptTokens": 43_614, "completionTokens": 20, "totalTokens": 43_634}
            }),
            tool("c1", 20, "done"),
        ]);
        let snapshot = view.render_parts_snapshot().unwrap();
        let items = snapshot["threads"][0]["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|item| item["kind"] == "assistant"
                    && item["usage"]["promptTokens"] == 43_614),
            "用量插件依赖助手卡的 usage.promptTokens：{items:?}"
        );
        assert!(
            items
                .iter()
                .all(|item| item["kind"] != "assistant" || item.get("text").is_none()),
            "助手正文不应被投影进 parts：{items:?}"
        );
    }

    // ── 端到端：假网关 → SSE → 浏览器读到的事件 ─────────────────────────
    //
    // 这一层守的是"接线"而不是"视图逻辑"：AgentClient 真的连 WS、真的收
    // `evt.state.patch`、真的把变化的条目编成 `item` 事件推出去。

    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::io::AsyncWriteExt as _;
    use tokio_tungstenite::tungstenite::Message as TMessage;

    fn sample_snapshot() -> Value {
        json!({
            "seq": 1,
            "activeThreadId": "t1",
            "runningThreadIds": [],
            "waitingThreadIds": [],
            "queue": [],
            "log": [],
            "config": {"model": "fake-model", "approval": "auto", "contextWindow": 1000},
            "ui": {"openTabIds": ["t1"]},
            "threads": [{
                "id": "t1",
                "title": "流式会话",
                "workspace": "E:/fake_ws",
                "items": [{"id": "a1", "kind": "assistant", "at": 10, "text": "第一段"}],
            }],
        })
    }

    fn sample_patch() -> Value {
        let mut meta = sample_snapshot();
        for thread in meta["threads"].as_array_mut().unwrap() {
            thread["items"] = json!([]);
        }
        json!({
            "seq": 2,
            "baseSeq": 1,
            "meta": meta,
            "upserted": [{
                "threadId": "t1",
                "item": {"id": "a1", "kind": "assistant", "at": 10, "text": "第一段，然后继续输出。"},
            }],
            "removed": [],
        })
    }

    async fn fake_socket(socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
        let (mut tx, mut rx) = socket.split();
        let seed = json!({
            "jsonrpc": "2.0",
            "method": "evt.state.snapshot",
            "params": {"seq": 1, "topic": "evt.state.snapshot", "payload": sample_snapshot()},
        });
        let _ = tx.send(TMessage::Text(seed.to_string().into())).await;
        while let Some(Ok(message)) = rx.next().await {
            let TMessage::Text(text) = message else {
                continue;
            };
            let Ok(request) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let reply = match request.get("method").and_then(Value::as_str) {
                Some("session.initialize") => {
                    // 真宿主在握手后才会切增量帧；这里给一小段延迟，模拟"流式正在跑"。
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let patch = json!({
                        "jsonrpc": "2.0",
                        "method": "evt.state.patch",
                        "params": sample_patch(),
                    });
                    let _ = tx.send(TMessage::Text(patch.to_string().into())).await;
                    json!({"protocolVersion": "1.0"})
                }
                _ => Value::Null,
            };
            let frame = json!({"jsonrpc": "2.0", "id": id, "result": reply});
            let _ = tx.send(TMessage::Text(frame.to_string().into())).await;
        }
    }

    /// 一个最小的"假网关"：同一个端口上同时答 `/api/snapshot`（HTTP）
    /// 与 `/rpc`（WebSocket 升级）。
    ///
    /// 刻意不用 axum 的 `ws` feature：那会给生产依赖多拉一套 tungstenite，
    /// 只为了一个测试并不划算。这里用 `peek` 只看不消费，再决定交给谁。
    async fn fake_gateway() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buffer = [0u8; 4096];
                    for _ in 0..20 {
                        match stream.peek(&mut buffer).await {
                            Ok(0) => return,
                            Ok(read) => {
                                head = buffer[..read].to_vec();
                                if head.windows(4).any(|window| window == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => return,
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
                    if text.contains("upgrade: websocket") {
                        if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                            fake_socket(ws).await;
                        }
                    } else {
                        let request_line = text.lines().next().unwrap_or("");
                        let path = request_line.split_whitespace().nth(1).unwrap_or("");
                        let body = if path.contains("/items") {
                            json!({
                                "threadId": "t1",
                                "items": [{"id": "a1", "kind": "assistant", "at": 10, "text": "第一段"}],
                                "from": 0,
                                "to": 1,
                                "total": 1,
                            })
                            .to_string()
                        } else {
                            sample_snapshot().to_string()
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn sse_stream_pushes_item_updates_from_gateway_patches() {
        let gateway = fake_gateway().await;
        let state = AppState {
            agent: Arc::new(crate::agent::AgentClient::new(gateway, "", "E:/fake_ws")),
            frontend: Arc::new(crate::plugins::PluginRegistry::new(
                std::env::temp_dir().join("ssr_h_stream_test_plugins"),
            )),
            access: Arc::new(crate::guard::Access::for_host(
                "127.0.0.1".parse().unwrap(),
                8788,
                None,
            )),
        };

        let response = live_stream(
            axum::extract::State(state),
            axum::extract::Path("t1".to_string()),
        )
        .await;
        assert_eq!(response.status(), 200);

        let mut body = response.into_body().into_data_stream();
        let mut seen = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout(Duration::from_millis(800), body.next()).await;
            let Ok(Some(Ok(chunk))) = next else { break };
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains("然后继续输出") {
                break;
            }
        }

        assert!(seen.contains("event: transcript"), "首帧应整段同步：{seen}");
        assert!(
            seen.contains("data-item-id=\"a1\""),
            "条目要带稳定身份，客户端才能原位替换：{seen}"
        );
        assert!(
            seen.contains("然后继续输出"),
            "网关的增量帧必须变成 SSE 的 item 更新：{seen}"
        );
    }
}
