use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentScope {
    Builtin,
    Global,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentMode {
    Readonly,
    Readwrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentGateConfig {
    pub criteria: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_open: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentModelOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    pub mode: SubagentMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<SubagentModelOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<SubagentGateConfig>,
    pub enabled: bool,
    pub scope: SubagentScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunResult {
    pub ok: bool,
    pub summary: String,
    pub steps_executed: usize,
    pub duration_ms: u64,
    pub tool_calls_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStepUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub step: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentToolArgs {
    pub subagent_id: String,
    pub task: String,
    #[serde(default)]
    pub additional_context: Option<String>,
    #[serde(default)]
    pub r#async: Option<bool>,
}
