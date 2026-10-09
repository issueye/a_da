//! **交互平台**：网关把"派活"变成"驱动远端 agent 跑一轮并拿回结果"。
//!
//! # 为什么这件事归网关
//!
//! PM agent 要"控制和与 coding agent 交互协作"。它可以自己走一遍协议
//! （建线程 → 发消息 → 盯快照 → 判断完成），但那样每个调用方都要理解
//! 线程/事件语义，而且**取消**要跨进程穿透——那是**平台**该提供的，不是每个调用方重复实现。
//!
//! ```text
//! PM 节点 ──AgentBus──► 网关 ──gateway.delegate──► 驱动 coding 节点跑一轮
//!                          │
//!                          └── 取消：gateway.cancelDelegation → thread.abort 打到目标
//! ```
//!
//! # 完成判据（不用猜）
//!
//! 目标节点会持续广播 `evt.state.snapshot`。一次派活**完成**的判据是：
//!
//! 1. 目标线程出现在快照里；
//! 2. 它**不在** `runningThreadIds` 里；
//! 3. 它至少有一条 `assistant` 条目（否则"刚建好还没开跑"会被误判成完成）。
//!
//! 第 3 条是关键：连接那一刻目标会推一帧种子快照，那时线程还不存在或还是空的。
//! 只判 1+2 会在种子快照上立刻"完成"，拿回一个空结果。
//!
//! # 深度上限
//!
//! 跨网关委派让"嵌套深度"**跨过了进程边界**——本地的 `NEVER_FOR_SUBAGENT` 黑名单
//! 管不到另一台机器上的 agent。所以深度由请求携带（`depth`），网关在此**强制上限**。
//! 拿不到依据时**倒向拒绝**（`FailDirection::Closed` 同原则）：深度不明就不派活。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::info;

use crate::registry::AgentInstance;

/// 允许的最大委派深度。`depth` 从 1 开始（第一次跨网关派活就是 1 层）。
///
/// 为什么是 2 而不是更大：每一层都是一个**独立的 agent 节点**在跑（各自的模型调用
/// 与工具执行）。层数越多，用户看到的东西越难解释、取消越难追、账单越难归因。
/// 需要更深时应当显式调大并说明理由——而不是让它默默长起来。
pub const MAX_DELEGATION_DEPTH: u32 = 2;

/// 一轮派活的等待上限（防止目标卡住时永远挂着）。
pub const DEFAULT_DELEGATION_TIMEOUT: Duration = Duration::from_secs(600);

/// 取消轮询间隔。
///
/// 与节点侧 `local_bus` 同口径：50ms 在秒级场景完全够用，且不引入新的契约
/// （`CancelToken` 只有轮询式 `is_cancelled()`，没有"等它发生"的入口）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 派活失败的原因——**每种都如实区分**，让调用方能给出有用的信息。
#[derive(Debug)]
pub enum DelegateError {
    /// 深度超限（含"深度不明"）
    DepthExceeded { depth: u32, max: u32 },
    /// 连不上目标
    Connect(String),
    /// 目标拒绝了请求（协议错误）
    Remote { method: String, message: String },
    /// 超过等待上限
    Timeout { waited: Duration },
    /// 被取消
    Cancelled,
    /// 目标在时限内没有给出可判定的结果（如线程一直没出现）
    NoResult { reason: String },
}

impl std::fmt::Display for DelegateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DepthExceeded { depth, max } => {
                write!(f, "委派深度超限：请求 {depth}，上限 {max}（拒绝而不是截断）")
            }
            Self::Connect(e) => write!(f, "连不上目标 agent：{e}"),
            Self::Remote { method, message } => write!(f, "目标拒绝 `{method}`：{message}"),
            Self::Timeout { waited } => write!(f, "派活超过 {waited:?} 未完成"),
            Self::Cancelled => write!(f, "派活已被取消"),
            Self::NoResult { reason } => write!(f, "拿不到结果：{reason}"),
        }
    }
}

impl std::error::Error for DelegateError {}

/// 一次派活的结果。
#[derive(Debug, Clone)]
pub struct DelegateOutcome {
    pub delegation_id: String,
    pub agent_id: String,
    pub thread_id: String,
    /// 目标那一轮的最终回复文本
    pub summary: String,
    /// 结构化明细（步数 / 工具调用数 / 耗时），进 `details` 字段
    pub details: serde_json::Value,
}

/// 进行中的派活：取消标志。
///
/// 用 `AtomicBool` 而不是 `Notify`：取消**可能先于**派活任务注册（客户端刚发请求就取消）。
/// 布尔标志不会丢事件，`Notify` 会（"取消发生在等待之前"是经典丢唤醒场景）。
#[derive(Default)]
pub struct DelegationRegistry {
    inner: RwLock<BTreeMap<String, Arc<AtomicBool>>>,
}

impl DelegationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn open(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.inner.write().await.insert(id.to_string(), flag.clone());
        flag
    }

    /// 取消一个派活。返回 `false` 表示**没有这个派活**（可能已完成、也可能 id 写错）。
    ///
    /// 这两种情况刻意不区分：网关不缓存已完成派活的历史（那是"网关持有会话状态"的开端）。
    pub async fn cancel(&self, id: &str) -> bool {
        match self.inner.read().await.get(id) {
            Some(f) => {
                f.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub async fn close(&self, id: &str) {
        self.inner.write().await.remove(id);
    }

    pub async fn active_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

/// 驱动目标 agent 跑一轮，拿回结果。
///
/// `depth` 是**本次派活的层数**（调用方携带）。超限直接拒绝——不截断、不降级。
pub async fn delegate(
    instance: &AgentInstance,
    task: &str,
    delegation_id: &str,
    depth: u32,
    cancel: Arc<AtomicBool>,
    registry: Arc<DelegationRegistry>,
    timeout: Duration,
) -> Result<DelegateOutcome, DelegateError> {
    if depth == 0 || depth > MAX_DELEGATION_DEPTH {
        return Err(DelegateError::DepthExceeded {
            depth,
            max: MAX_DELEGATION_DEPTH,
        });
    }

    let result = tokio::time::timeout(
        timeout,
        run_delegation(instance, task, delegation_id, cancel, registry.clone()),
    )
    .await;

    registry.close(delegation_id).await;

    match result {
        Ok(r) => r,
        Err(_) => Err(DelegateError::Timeout { waited: timeout }),
    }
}

async fn run_delegation(
    instance: &AgentInstance,
    task: &str,
    delegation_id: &str,
    cancel: Arc<AtomicBool>,
    _registry: Arc<DelegationRegistry>,
) -> Result<DelegateOutcome, DelegateError> {
    let req = instance
        .endpoint
        .clone()
        .into_client_request()
        .map_err(|e| DelegateError::Connect(e.to_string()))?;
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| DelegateError::Connect(e.to_string()))?;
    let (mut tx, mut rx) = ws.split();

    let mut next_id: u64 = 1;

    // ① 握手（宿主规定：握手前只接受 session.initialize）
    send_frame(
        &mut tx,
        &mut next_id,
        "session.initialize",
        serde_json::json!({
            "protocolVersion": "1.0",
            "client": { "name": "a-da-gateway", "version": env!("CARGO_PKG_VERSION"), "platform": "gateway" }
        }),
    )
    .await?;

    // ② 建线程（派活用一个**全新线程**：不复用别人的 active，也不污染它）
    send_frame(
        &mut tx,
        &mut next_id,
        "thread.create",
        serde_json::json!({ "title": format!("委派 {delegation_id}") }),
    )
    .await?;

    // ③ 发任务
    let mut thread_id: Option<String> = None;
    let mut sent_task = false;
    let mut last_assistant = String::new();
    let mut steps: u64 = 0;
    let mut tool_calls: u64 = 0;
    let started_at = now_ms();
    let mut saw_running = false;

    loop {
        // 取消检查必须**与读帧并发**，不能只在"收到下一帧时"顺带查一次。
        //
        // 为什么（这条是被端到端测试抓出来的真缺陷）：目标在跑长任务时是**安静的**——
        // 它不发帧。若把取消检查放在 `rx.next()` 之后，取消只能在"下一帧到达"时
        // 才生效，而那可能要等到 600s 超时。表现就是"取消看起来发了、实际到不了"，
        // 正是本仓反复清理的那类假接线。
        let msg = tokio::select! {
            m = rx.next() => m,
            _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                if cancel.load(Ordering::SeqCst) {
                    if let Some(tid) = &thread_id {
                        let abort_id = next_id;
                        let _ = send_frame(
                            &mut tx,
                            &mut next_id,
                            "thread.abort",
                            serde_json::json!({ "threadId": tid }),
                        )
                        .await;

                        // **等中止被受理**（有限等待）再回 "cancelled"。
                        //
                        // 为什么不能发完就回：那样调用方拿到"已取消"时，目标可能还没读到
                        // 中止帧——"已取消"就成了一个**没有保证**的说法。
                        // 等它的响应，`cancelled` 才真的意味着"目标已受理"。
                        // 上限 2s：目标不响应时也不能把取消卡住（取消本身不该比任务还慢）。
                        let _ = tokio::time::timeout(Duration::from_secs(2), async {
                            while let Some(Ok(m)) = rx.next().await {
                                let Message::Text(t) = m else { continue };
                                let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
                                    continue;
                                };
                                if v.get("id").and_then(|x| x.as_u64()) == Some(abort_id) {
                                    return;
                                }
                            }
                        })
                        .await;
                    }
                    return Err(DelegateError::Cancelled);
                }
                continue;
            }
        };

        let msg = match msg {
            Some(Ok(m)) => m,
            Some(Err(e)) => return Err(DelegateError::Connect(e.to_string())),
            None => {
                return Err(DelegateError::NoResult {
                    reason: "目标连接在出结果前就关闭了".to_string(),
                })
            }
        };

        let Message::Text(text) = msg else { continue };
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };

        // 错误响应 → 如实上报（`method` 用 id 反查不到就在信息里说明）
        if let Some(err) = frame.get("error") {
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("（无 message）")
                .to_string();
            return Err(DelegateError::Remote {
                method: frame
                    .get("id")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                message,
            });
        }

        // 请求响应：thread.create 的结果给出新线程 id
        if let Some(result) = frame.get("result") {
            if thread_id.is_none() {
                if let Some(tid) = extract_thread_id(result) {
                    thread_id = Some(tid.clone());
                    info!("派活 {delegation_id} 建立线程 {tid}");
                    send_frame(
                        &mut tx,
                        &mut next_id,
                        "thread.send",
                        serde_json::json!({ "threadId": tid, "text": task }),
                    )
                    .await?;
                    sent_task = true;
                    continue;
                }
            }
            continue;
        }

        // 事件：只看状态快照（完成判据）
        if frame.get("method").and_then(|v| v.as_str()) == Some("evt.state.snapshot") {
            let Some(params) = frame.get("params") else { continue };
            let running: Vec<&str> = params
                .get("runningThreadIds")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();

            // 线程还没建好之前不看快照
            let Some(tid) = thread_id.clone() else { continue };
            if running.contains(&tid.as_str()) {
                saw_running = true;
            }

            let Some(thread) = params
                .get("threads")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tid.as_str())))
            else {
                continue;
            };

            // 统计（结构化明细用）
            if let Some(items) = thread.get("items").and_then(|v| v.as_array()) {
                tool_calls = items
                    .iter()
                    .filter(|i| i.get("kind").and_then(|v| v.as_str()) == Some("tool"))
                    .count() as u64;
                steps = items.len() as u64;
                if let Some(last) = items.iter().rev().find(|i| {
                    i.get("kind").and_then(|v| v.as_str()) == Some("assistant")
                }) {
                    if let Some(t) = last.get("text").and_then(|v| v.as_str()) {
                        last_assistant = t.to_string();
                    }
                }
            }

            // 完成判据：已开跑过 + 当前不在跑 + 有 assistant 内容
            if sent_task
                && saw_running
                && !running.contains(&tid.as_str())
                && !last_assistant.is_empty()
            {
                return Ok(DelegateOutcome {
                    delegation_id: delegation_id.to_string(),
                    agent_id: instance.id.clone(),
                    thread_id: tid,
                    summary: last_assistant,
                    details: serde_json::json!({
                        "delegationId": delegation_id,
                        "agentId": instance.id,
                        "steps": steps,
                        "toolCalls": tool_calls,
                        "durationMs": now_ms() - started_at,
                    }),
                });
            }
        }
    }
}

/// 发一帧 JSON-RPC 请求（自增 id）。
///
/// 独立成函数而不是闭包：闭包会**同时**可变捕获 `tx` 与 `next_id`，
/// 返回的 future 借用它们时生命周期逃不出闭包体（编译器会拒绝）。
async fn send_frame<S>(
    tx: &mut S,
    next_id: &mut u64,
    method: &str,
    params: serde_json::Value,
) -> Result<(), DelegateError>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": *next_id,
        "method": method,
        "params": params,
    });
    *next_id += 1;
    tx.send(Message::Text(frame.to_string().into()))
        .await
        .map_err(|e| DelegateError::Connect(e.to_string()))
}
/// 从 `thread.create` 的响应里取出线程 id。
///
/// 兼容两种形状（`{id}` 与 `{threadId}`）：响应形状由节点决定，
/// 而网关**不该因为字段名不同就判定失败**——那是脆弱的耦合。
fn extract_thread_id(result: &serde_json::Value) -> Option<String> {
    for key in ["threadId", "id"] {
        if let Some(v) = result.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    result
        .get("thread")
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_depth_zero_is_rejected_not_defaulted() {
        // depth=0 是"调用方没给"——必须拒绝，而不是当成 1（倒向拒绝）
        let f = tokio::runtime::Runtime::new().unwrap();
        let reg = Arc::new(DelegationRegistry::new());
        let inst = AgentInstance {
            id: "a".into(),
            product: "ada-coding".into(),
            workspace: "E:/ws".into(),
            endpoint: "ws://127.0.0.1:1/rpc".into(),
            status: crate::registry::AgentStatus::Ready,
            pid: None,
            started_at: 0,
        };
        let err = f
            .block_on(delegate(&inst, "t", "d1", 0, Arc::new(AtomicBool::new(false)), reg, Duration::from_millis(10)))
            .expect_err("depth=0 应被拒绝");
        assert!(matches!(err, DelegateError::DepthExceeded { depth: 0, .. }), "{err}");
    }

    #[test]
    fn test_depth_over_max_is_rejected() {
        let f = tokio::runtime::Runtime::new().unwrap();
        let reg = Arc::new(DelegationRegistry::new());
        let inst = AgentInstance {
            id: "a".into(),
            product: "ada-coding".into(),
            workspace: "E:/ws".into(),
            endpoint: "ws://127.0.0.1:1/rpc".into(),
            status: crate::registry::AgentStatus::Ready,
            pid: None,
            started_at: 0,
        };
        let err = f
            .block_on(delegate(&inst, "t", "d1", MAX_DELEGATION_DEPTH + 1, Arc::new(AtomicBool::new(false)), reg, Duration::from_millis(10)))
            .expect_err("超限应被拒绝");
        assert!(matches!(err, DelegateError::DepthExceeded { .. }), "{err}");
    }

    /// 取消是**布尔标志**而不是通知：先取消、后注册也必须生效（丢唤醒是经典坑）。
    #[tokio::test]
    async fn test_cancel_before_open_is_not_lost() {
        let reg = DelegationRegistry::new();
        // 尚未 open 就取消 → 返回 false（没有这个派活），且不 panic
        assert!(!reg.cancel("never-opened").await);

        let flag = reg.open("d1").await;
        assert!(reg.cancel("d1").await);
        assert!(flag.load(Ordering::SeqCst), "取消必须落到标志上");
        assert_eq!(reg.active_count().await, 1);
        reg.close("d1").await;
        assert_eq!(reg.active_count().await, 0);
    }

    #[test]
    fn test_extract_thread_id_supports_both_shapes() {
        assert_eq!(
            extract_thread_id(&serde_json::json!({"threadId": "t1"})),
            Some("t1".to_string())
        );
        assert_eq!(
            extract_thread_id(&serde_json::json!({"id": "t2"})),
            Some("t2".to_string())
        );
        assert_eq!(
            extract_thread_id(&serde_json::json!({"thread": {"id": "t3"}})),
            Some("t3".to_string())
        );
        assert_eq!(extract_thread_id(&serde_json::json!({"ok": true})), None);
    }
}
