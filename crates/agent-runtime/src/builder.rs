//! 组合根：唯一允许读取声明、装配端口与构建 AgentRuntime 的地方（INV-8）。

use std::sync::Arc;
use std::time::Duration;

use agent_base::engine::{AgentRuntime, RunPolicy};
use agent_base::ports::{
    ApprovalGate, Clock, Consumer, ContractViolation, ModelClient, PromptSource,
    Scope, SessionStore, Tool, ToolCatalog,
};
use thiserror::Error;

use crate::catalog::CompositeToolCatalog;
use crate::spec::AgentSpec;

#[derive(Debug, Error)]
pub enum SpecError {
    #[error("规格配置违约：{0}")]
    Violation(String),
    #[error("缺少必要端口装配：{0}")]
    MissingPort(String),
}

/// 产品装配器 / 组合根（ProductBuilder）。
pub struct ProductBuilder {
    pub spec: AgentSpec,
    tools: Vec<Arc<dyn Tool>>,
    model: Option<Arc<dyn ModelClient>>,
    approval: Option<Arc<dyn ApprovalGate>>,
    store: Option<Arc<dyn SessionStore>>,
    prompt: Option<Arc<dyn PromptSource>>,
    scope: Option<Arc<dyn Scope>>,
    clock: Option<Arc<dyn Clock>>,
}

impl ProductBuilder {
    pub fn new(spec: AgentSpec) -> Self {
        Self {
            spec,
            tools: Vec::new(),
            model: None,
            approval: None,
            store: None,
            prompt: None,
            scope: None,
            clock: None,
        }
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    pub fn with_model(mut self, model: Arc<dyn ModelClient>) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_approval(mut self, approval: Arc<dyn ApprovalGate>) -> Self {
        self.approval = Some(approval);
        self
    }

    pub fn with_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn with_prompt(mut self, prompt: Arc<dyn PromptSource>) -> Self {
        self.prompt = Some(prompt);
        self
    }

    pub fn with_scope(mut self, scope: Arc<dyn Scope>) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// 校验当前装配是否与产品声明契约双向一致。
    pub fn validate(&self) -> Vec<ContractViolation> {
        let catalog = CompositeToolCatalog::new(self.tools.clone(), None);
        catalog.validate(&Consumer::ALL)
    }

    /// 装配所有端口并产出单例 `AgentRuntime`。
    pub fn build(self) -> Result<AgentRuntime, SpecError> {
        let violations = self.validate();
        if !violations.is_empty() {
            return Err(SpecError::Violation(format!("{:?}", violations)));
        }

        let model = self.model.ok_or_else(|| SpecError::MissingPort("ModelClient".into()))?;
        let approval = self.approval.ok_or_else(|| SpecError::MissingPort("ApprovalGate".into()))?;
        let store = self.store.ok_or_else(|| SpecError::MissingPort("SessionStore".into()))?;
        let prompt = self.prompt.ok_or_else(|| SpecError::MissingPort("PromptSource".into()))?;
        let scope = self.scope.ok_or_else(|| SpecError::MissingPort("Scope".into()))?;
        let clock = self.clock.ok_or_else(|| SpecError::MissingPort("Clock".into()))?;

        // 装配一次 ToolCatalog，注入组合根
        let catalog = Arc::new(CompositeToolCatalog::new(self.tools, None));

        let policy = RunPolicy {
            max_steps: self.spec.policies.max_steps,
            max_parallel_tools: self.spec.policies.parallel_tools.unwrap_or(1),
            tool_timeout: self.spec.policies.tool_timeout_sec.map(Duration::from_secs),
        };

        Ok(AgentRuntime::new(
            model,
            catalog,
            approval,
            store,
            prompt,
            scope,
            clock,
            policy,
        ))
    }
}
