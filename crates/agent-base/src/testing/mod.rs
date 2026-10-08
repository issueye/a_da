//! 测试替身：让"引擎测试"不再需要真 WS、真磁盘、真窗口（计划 §9 的样板目标）。
//!
//! 这些替身放在基座里（而不是各 crate 的 `#[cfg(test)]`），因为适配器与产品的测试都要用同一套，
//! 且它们**必须**与端口实现同一份契约——合规套件拿它们当参照实现。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use crate::domain::AgentEvent;
use crate::ports::{AppHome, CancelToken, Clock, EventSink};

/// 确定性时钟：时间只在测试要求时前进。
pub struct FixedClock {
    now_ms: AtomicI64,
}

impl FixedClock {
    pub fn new(start_ms: i64) -> Self {
        Self { now_ms: AtomicI64::new(start_ms) }
    }

    pub fn advance(&self, delta_ms: i64) -> i64 {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst) + delta_ms
    }

    pub fn set(&self, ms: i64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new(1_700_000_000_000)
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// 录制型事件出口：把事件按顺序留下来供断言（替代"靠日志文本判断"）。
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("事件锁中毒").clone()
    }

    /// 取走已录制的事件并清空（便于分段断言）。
    pub fn take(&self) -> Vec<AgentEvent> {
        std::mem::take(&mut *self.events.lock().expect("事件锁中毒"))
    }

    /// 事件类型序列（断言顺序用）。
    pub fn kinds(&self) -> Vec<&'static str> {
        self.snapshot().into_iter().map(|e| e.body.kind()).collect()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().expect("事件锁中毒").push(event);
    }
}

/// 永不被取消的令牌。
pub struct NeverCancel;

impl CancelToken for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 由测试指定的应用目录（**调用方给路径**：基座不读环境变量，也不建目录）。
pub struct TempAppHome {
    root: PathBuf,
    config_file: Option<PathBuf>,
}

impl TempAppHome {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), config_file: None }
    }

    pub fn with_config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_file = Some(path.into());
        self
    }
}

impl AppHome for TempAppHome {
    fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn config_file(&self) -> PathBuf {
        self.config_file.clone().unwrap_or_else(|| self.root.join("config.json"))
    }
}

/// 可动态触发取消的令牌。
pub struct ManualCancel {
    cancelled: std::sync::atomic::AtomicBool,
}

impl ManualCancel {
    pub fn new() -> Self {
        Self { cancelled: std::sync::atomic::AtomicBool::new(false) }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Default for ManualCancel {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken for ManualCancel {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

use std::collections::HashMap;
use std::sync::Arc;
use crate::domain::{AgentError, AgentMessage, DenialKind, FailDirection, ToolCall, ToolDescriptor, ToolReceipt};
use crate::model::StreamDelta;
use crate::ports::{
    AnsweredBy, ApprovalGate, ApprovalOutcome, ApprovalRequest, BoxFuture,
    CompletionRequest, Consumer, ContractViolation, DeltaStream, ModelCapabilities,
    ModelClient, ModelError, PromptSource, Scope, SessionStore, Tool,
    ToolCatalog, ToolContext, ToolError,
};

/// 内存会话存储替身：线程安全、零 IO。
#[derive(Default)]
pub struct InMemorySessionStore {
    messages: Mutex<HashMap<String, Vec<AgentMessage>>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, thread_id: &str) -> Vec<AgentMessage> {
        self.messages.lock().expect("store lock").get(thread_id).cloned().unwrap_or_default()
    }
}

impl SessionStore for InMemorySessionStore {
    fn load_messages<'a>(
        &'a self,
        thread_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AgentMessage>, AgentError>> {
        let msgs = self.get(thread_id);
        Box::pin(async move { Ok(msgs) })
    }

    fn append_message<'a>(
        &'a self,
        thread_id: &'a str,
        message: &'a AgentMessage,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        let msg = message.clone();
        let mut lock = self.messages.lock().expect("store lock");
        lock.entry(thread_id.to_string()).or_default().push(msg);
        Box::pin(async move { Ok(()) })
    }
}

/// 固定提示词替身。
pub struct FixedPrompt {
    prompt: String,
}

impl FixedPrompt {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self { prompt: prompt.into() }
    }
}

impl Default for FixedPrompt {
    fn default() -> Self {
        Self::new("test system prompt")
    }
}

impl PromptSource for FixedPrompt {
    fn system_prompt(&self) -> String {
        self.prompt.clone()
    }
}

/// 模拟作用域替身。
pub struct MockScope {
    id: String,
}

impl MockScope {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl Default for MockScope {
    fn default() -> Self {
        Self::new("mock_scope")
    }
}

impl Scope for MockScope {
    fn id(&self) -> &str {
        &self.id
    }

    fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
        if raw.contains("..") || raw.starts_with('/') || raw.starts_with('\\') {
            return Err(DenialKind::Sandbox {
                path: raw.to_string(),
            });
        }
        Ok(PathBuf::from(&self.id).join(raw))
    }
}

/// 记录型审批闸门替身。
pub struct RecordingApprovalGate {
    outcomes: Mutex<Vec<ApprovalOutcome>>,
    calls: Mutex<Vec<ApprovalRequest>>,
    direction: FailDirection,
}

impl RecordingApprovalGate {
    pub fn new(default_approved: bool) -> Self {
        let outcome = if default_approved {
            ApprovalOutcome::allowed(AnsweredBy::Policy)
        } else {
            ApprovalOutcome::denied(AnsweredBy::Policy, "测试策略拒绝")
        };
        Self {
            outcomes: Mutex::new(vec![outcome]),
            calls: Mutex::new(Vec::new()),
            direction: FailDirection::Closed,
        }
    }

    pub fn with_outcomes(outcomes: Vec<ApprovalOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes),
            calls: Mutex::new(Vec::new()),
            direction: FailDirection::Closed,
        }
    }

    pub fn recorded_calls(&self) -> Vec<ApprovalRequest> {
        self.calls.lock().expect("lock").clone()
    }
}

impl ApprovalGate for RecordingApprovalGate {
    fn direction(&self) -> FailDirection {
        self.direction
    }

    fn decide<'a>(
        &'a self,
        req: ApprovalRequest,
        _cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, ApprovalOutcome> {
        self.calls.lock().expect("lock").push(req);
        let mut outcomes = self.outcomes.lock().expect("lock");
        let outcome = if outcomes.len() > 1 {
            outcomes.remove(0)
        } else {
            outcomes.first().cloned().unwrap_or_else(|| ApprovalOutcome::allowed(AnsweredBy::Policy))
        };
        Box::pin(async move { outcome })
    }
}

/// 模拟工具替身。
pub struct MockTool {
    descriptor: ToolDescriptor,
    receipt: Mutex<ToolReceipt>,
}

impl MockTool {
    pub fn new(descriptor: ToolDescriptor, receipt: ToolReceipt) -> Self {
        Self {
            descriptor,
            receipt: Mutex::new(receipt),
        }
    }

    pub fn set_receipt(&self, receipt: ToolReceipt) {
        *self.receipt.lock().expect("lock") = receipt;
    }
}

impl Tool for MockTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        _call: &'a ToolCall,
        _ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        let r = self.receipt.lock().expect("lock").clone();
        Box::pin(async move { r })
    }
}

/// 内存工具目录替身。
#[derive(Default)]
pub struct InMemoryToolCatalog {
    tools: Mutex<HashMap<String, Arc<dyn Tool>>>,
}

impl InMemoryToolCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.descriptor().name.clone();
        self.tools.lock().expect("lock").insert(name, tool);
    }
}

impl ToolCatalog for InMemoryToolCatalog {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .lock()
            .expect("lock")
            .values()
            .map(|t| t.descriptor().clone())
            .collect()
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Tool>, ToolError> {
        self.tools
            .lock()
            .expect("lock")
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::Unknown(name.to_string()))
    }

    fn validate(&self, _consumers: &[Consumer]) -> Vec<ContractViolation> {
        Vec::new()
    }
}

/// 脚本化大模型客户端替身（按轮次依次吐出预设的 StreamDelta 序列）。
pub struct ScriptedModelClient {
    capabilities: ModelCapabilities,
    rounds: Mutex<Vec<Vec<StreamDelta>>>,
}

impl ScriptedModelClient {
    pub fn new(rounds: Vec<Vec<StreamDelta>>) -> Self {
        Self {
            capabilities: ModelCapabilities {
                streaming: true,
                tools: true,
                images: false,
                thinking: true,
            },
            rounds: Mutex::new(rounds),
        }
    }
}

impl ModelClient for ScriptedModelClient {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities
    }

    fn stream<'a>(
        &'a self,
        _req: CompletionRequest,
        cancel: Option<&'a dyn CancelToken>,
    ) -> BoxFuture<'a, Result<DeltaStream, ModelError>> {
        if cancel.map_or(false, |c| c.is_cancelled()) {
            return Box::pin(async move { Err(ModelError::Cancelled) });
        }

        let mut rounds = self.rounds.lock().expect("lock");
        let deltas = if !rounds.is_empty() {
            rounds.remove(0)
        } else {
            Vec::new()
        };

        let (tx, rx) = tokio::sync::mpsc::channel(deltas.len().max(1));
        tokio::spawn(async move {
            for d in deltas {
                if tx.send(d).await.is_err() {
                    break;
                }
            }
        });

        Box::pin(async move { Ok(rx) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AgentEvent, AgentEventBody};

    #[test]
    fn fixed_clock_only_moves_when_told() {
        let c = FixedClock::new(1000);
        assert_eq!(c.now_ms(), 1000);
        c.advance(250);
        assert_eq!(c.now_ms(), 1250);
        c.set(42);
        assert_eq!(c.now_ms(), 42);
    }

    #[test]
    fn recording_sink_preserves_order_and_clears_on_take() {
        let sink = RecordingSink::new();
        sink.emit(AgentEvent::new(1, 0, "t1", AgentEventBody::TurnStarted));
        sink.emit(AgentEvent::new(2, 1, "t1", AgentEventBody::TextDelta { text: "hi".into() }));
        assert_eq!(sink.kinds(), vec!["turn.started", "text.delta"]);
        assert_eq!(sink.take().len(), 2);
        assert!(sink.snapshot().is_empty());
    }

    #[test]
    fn temp_app_home_maps_kinds_without_touching_disk() {
        let home = TempAppHome::at("/tmp/x");
        assert_eq!(home.root(), Path::new("/tmp/x"));
        assert_eq!(home.dir("sessions"), PathBuf::from("/tmp/x/sessions"));
        assert_eq!(home.config_file(), PathBuf::from("/tmp/x/config.json"));
    }
}
