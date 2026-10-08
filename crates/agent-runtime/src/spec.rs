//! 产品规格定义（agent.spec.json 对应的数据结构，设计 §4）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    pub id: String,
    pub archetype: String,
    pub identity: IdentitySpec,
    #[serde(default)]
    pub toolkits: Vec<String>,
    #[serde(default)]
    pub capabilities: CapabilitySpec,
    #[serde(default)]
    pub policies: PolicySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySpec {
    pub name: String,
    pub persona: String,
    pub locale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySpec {
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub rollback: bool,
    #[serde(default)]
    pub subagents: bool,
    #[serde(default)]
    pub plugins: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicySpec {
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub parallel_tools: Option<usize>,
    #[serde(default)]
    pub tool_timeout_sec: Option<u64>,
}

impl AgentSpec {
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}
