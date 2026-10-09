//! AgentBus 的**网关实现**（S6）：经协议把任务派给**另一个 agent 节点**。
//!
//! # 为什么放在桥接面（gent-rpc）而不是节点里
//!
//! 它需要一个 **WebSocket 客户端**。而 erify-wiring 的 check I 规定
//! **节点不含协议管道**（不得出现 	okio_tungstenite）——那条边界的意图是
//! 「节点不拥有传输」。这里正好是对那条边界的检验：**跨进程委派的传输属于桥接面**，
//! 所以实现放在本 crate，节点只保留端口（gent_node::agent_bus）。
//!
//! 副产品：gent-node 至今**不依赖任何传输库**，可以被网关当作纯节点管理。
//!
//! # 它和 `LocalAgentBus` 的区别
//!
//! | | `LocalAgentBus` | `GatewayAgentBus` |
//! |---|---|---|
//! | 目标 | 进程内的子智能体（临时上下文） | 经网关可达的**另一个 agent 节点** |
//! | 上下文 | 一次性，跑完即散 | 目标节点上的一整个线程（**可续**） |
//! | 取消 | 转发到子智能体引擎 → 进程树 | `gateway.cancelDelegation` → 目标 `thread.abort` |
//! | 深度 | 进程内 `NEVER_FOR_SUBAGENT` 拦 | **请求携带 depth**，网关强制上限 |
//!
//! # 为什么实现放在**节点侧**而不是网关侧
//!
//! `AgentBus` 是**被节点消费**的端口（PM agent 的工具调它）。它的实现要"作为客户端
//! 去连网关"，所以属于消费者一侧。网关只提供**服务端**（`gateway.delegate`）。
//! 这也让 `verify-wiring` 的 check J 能继续成立：网关**不依赖**节点。
//!
//! # 取消怎么跨进程生效
//!
//! `dispatch` 是一个"发一帧、等一帧"的往返，但取消必须能打断它。做法：
//! - 主流程发 `gateway.delegate` 后**阻塞等响应**；
//! - 另起一个观察任务轮询 `req.cancel`，一旦置位就往**同一条连接**发
//!   `gateway.cancelDelegation`；
//! - 网关收到后置取消标志，派活任务在下一帧把 `thread.abort` 打到目标节点。
//!
//! 这条链上任何一环断了，取消都会"看起来发了、实际到不了"——所以端到端测试
//! 断言的是**目标那一轮真的停了**，而不是"我们发了取消帧"。

use std::time::Duration;

use agent_base::ports::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use agent_node::agent_bus::{AgentBus, AgentHandle, DispatchOutcome, DispatchRequest};

/// 取消轮询间隔。与 `local_bus` 同口径（50ms 在秒级场景完全够用，且不引入新契约）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 派活的等待上限（比网关侧的上限再宽一点：让**网关**先超时并给出明确原因）。
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(660);

/// 经网关委派给另一个 agent 节点。
pub struct GatewayAgentBus {
    /// 网关端点（`ws://127.0.0.1:<port>/rpc`，可带 `?token=`）
    endpoint: String,
    /// 本节点的委派深度：发起一次派活时携带的层数。
    ///
    /// 由组合根按**产品声明**给出：PM 类产品声明 `delegation = "gateway"` 时给 1；
    /// 叶子产品用 `LocalAgentBus`（不参与跨网关链），所以它们的委派不会继续加深。
    depth: u32,
    timeout: Duration,
}

impl GatewayAgentBus {
    pub fn new(endpoint: impl Into<String>, depth: u32) -> Self {
        Self {
            endpoint: endpoint.into(),
            depth,
            timeout: DISPATCH_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// 开一条到网关的连接（返回单写者通道 + 读端）。
    async fn connect(
        &self,
    ) -> Result<
        (
            mpsc::UnboundedSender<Message>,
            tokio::task::JoinHandle<()>,
            impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                + Unpin,
        ),
        String,
    > {
        let req = self
            .endpoint
            .clone()
            .into_client_request()
            .map_err(|e| format!("网关端点非法（{}）：{e}", self.endpoint))?;
        let (ws, _) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| format!("连不上网关（{}）：{e}", self.endpoint))?;

        let (mut sink, stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });
        Ok((tx, writer, stream))
    }
}

impl AgentBus for GatewayAgentBus {
    fn list_agents(&self) -> BoxFuture<'_, Vec<AgentHandle>> {
        Box::pin(async move {
            let Ok((tx, _writer, mut stream)) = self.connect().await else {
                // 发现失败 → **空列表**（不是编造的列表）。
                // 调用方据此报"找不到 agent"，而不是拿到一批不存在的名字。
                return Vec::new();
            };
            let frame = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "gateway.listAgents", "params": {}
            });
            if tx.send(Message::Text(frame.to_string().into())).is_err() {
                return Vec::new();
            }
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let Some(agents) = v.pointer("/result/agents").and_then(|a| a.as_array()) else {
                    continue;
                };
                return agents
                    .iter()
                    .filter_map(|a| {
                        Some(AgentHandle {
                            id: a.get("id")?.as_str()?.to_string(),
                            name: a
                                .get("workspace")
                                .and_then(|v| v.as_str())
                                .unwrap_or("agent")
                                .to_string(),
                            description: a
                                .get("product")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            enabled: a.get("status").and_then(|v| v.as_str()) == Some("ready"),
                        })
                    })
                    .collect();
            }
            Vec::new()
        })
    }

    fn dispatch<'a>(&'a self, req: DispatchRequest<'a>) -> BoxFuture<'a, DispatchOutcome> {
        Box::pin(async move {
            let (tx, writer, mut stream) = match self.connect().await {
                Ok(v) => v,
                Err(e) => return DispatchOutcome::rejected(e),
            };

            let delegation_id = uuid::Uuid::new_v4().simple().to_string();
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "gateway.delegate",
                "params": {
                    "agentId": req.agent_id,
                    "task": req.task,
                    // 深度：装配期给的节点深度（`self.depth`）与调用方携带的取较大者。
                    // 两者都表示"这条委派链已经走了多远"，取较大者是**保守**的一侧。
                    "depth": req.depth.max(self.depth),
                    "delegationId": delegation_id,
                }
            });
            if tx.send(Message::Text(frame.to_string().into())).is_err() {
                writer.abort();
                return DispatchOutcome::rejected("向网关发送派活请求失败（连接已断）");
            }

            // 取消观察：一旦父会话取消，就往**同一条连接**发取消帧。
            //
            // 为什么用 `select!` 而不是 `tokio::spawn`：`req.cancel` 是
            // `&'a dyn CancelToken`（**借用**，生命周期短于 `'static`），
            // move 进 spawn 的 future 编译器会拒绝（E0521）。
            // `select!` 在**同一个 future** 里并发两条分支，两个问题一起解决：
            // 读响应不被取消轮询饿死，取消也不被阻塞的读饿死。
            // ——与 W4-T3 修 `run_command` 假接线用的是同一个手法。
            let read = async {
                while let Some(Ok(msg)) = stream.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    // 只认 id=1 的那一帧（派活的响应）；取消帧的响应（id=2）忽略
                    if v.get("id").and_then(|x| x.as_u64()) != Some(1) {
                        continue;
                    }
                    if let Some(err) = v.get("error") {
                        let msg = err
                            .get("message")
                            .and_then(|x| x.as_str())
                            .unwrap_or("（无 message）")
                            .to_string();
                        // 网关侧的错误**不是"未进入执行"**——它是真去跑了才发现的问题，
                        // 所以给 `details`（否则调用方会以为根本没派出去）
                        return DispatchOutcome {
                            ok: false,
                            summary: msg.clone(),
                            error_message: Some(msg),
                            details: Some(serde_json::json!({
                                "delegationId": delegation_id,
                                "phase": "gateway",
                            })),
                        };
                    }
                    let Some(result) = v.get("result") else { continue };
                    return DispatchOutcome {
                        ok: result.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
                        summary: result
                            .get("summary")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        error_message: None,
                        details: result.get("details").cloned(),
                    };
                }
                DispatchOutcome::rejected("网关连接在给出结果前关闭了")
            };
            tokio::pin!(read);

            let mut cancel_sent = false;
            let _out = loop {
                tokio::select! {
                    r = &mut read => break r,
                    _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                        // 只发一次：重复发没有意义，而且会让网关侧日志变噪
                        if !cancel_sent
                            && req.cancel.map(|c| c.is_cancelled()).unwrap_or(false)
                        {
                            let f = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": 2,
                                "method": "gateway.cancelDelegation",
                                "params": { "delegationId": delegation_id }
                            });
                            let _ = tx.send(Message::Text(f.to_string().into()));
                            cancel_sent = true;
                        }
                    }
                }
            };

            let out = match tokio::time::timeout(self.timeout, read).await {
                Ok(v) => v,
                Err(_) => DispatchOutcome {
                    ok: false,
                    summary: format!("经网关派活超过 {:?} 未完成", self.timeout),
                    error_message: Some("timeout".to_string()),
                    details: Some(serde_json::json!({ "delegationId": delegation_id })),
                },
            };

            drop(tx);
            let _ = writer.await;
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连不上网关时必须**如实拒绝**（而不是编造一个空结果或挂死）。
    #[tokio::test]
    async fn test_dispatch_reports_unreachable_gateway() {
        // 端口 1 上不会有服务
        let bus = GatewayAgentBus::new("ws://127.0.0.1:1/rpc", 1).with_timeout(Duration::from_millis(500));
        let out = bus
            .dispatch(DispatchRequest {
                agent_id: "any",
                task: "t",
                additional_context: None,
                cancel: None,
                depth: 1,
            })
            .await;
        assert!(!out.ok);
        assert!(
            out.summary.contains("连不上网关") || out.summary.contains("connect"),
            "要如实说明连不上：{}",
            out.summary
        );
        assert_eq!(out.details, None, "没连上 → 未进入执行 → details 必须是 None");
    }

    /// 连不上网关时 `list_agents` 返回**空列表**，而不是编造的名字。
    #[tokio::test]
    async fn test_list_agents_is_empty_when_gateway_unreachable() {
        let bus = GatewayAgentBus::new("ws://127.0.0.1:1/rpc", 1);
        assert!(bus.list_agents().await.is_empty());
    }
}
