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
    /// 经网关委派的目标（S6）。只有 `capabilities.delegation == "gateway"` 时才需要。
    ///
    /// 为什么放在顶层而不是塞进 capabilities：它是**端点**（部署事实），
    /// 而 capabilities 是**能力位**（这个产品能做什么）。混在一起会让
    /// "能力位"变成"能力位 + 一个字符串"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewaySpec>,
}

/// 网关端点声明。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpec {
    /// 网关的 WS 端点（如 `ws://127.0.0.1:52353/rpc`）
    pub endpoint: String,
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
    /// 钩子点位机制是否开启（P3a · SP-56）。
    ///
    /// 为什么要有这个位而不是恒为 true：钩子让每次工具调用多一次**跨沙箱往返**
    /// （默认预算 500ms），且它把"拦下"这个权力交给了外部代码。产品应当能如实声明
    /// "我不要这套机制"——那种产品里插件声明了钩子**也不会被执行**，
    /// 装配期会产一条 `hooks_disabled_by_product` 诊断说清原因。
    #[serde(default)]
    pub hooks: bool,
    /// 委派模式（S6）：`local`（进程内临时子智能体，默认）或 `gateway`（经网关到另一个节点）。
    ///
    /// **为什么由声明决定而不是运行时探测**：它决定"这个实例在委派链上的位置"，
    /// 是**装配事实**。运行时探测会让"深度有界"变成运气。
    #[serde(default)]
    pub delegation: DelegationMode,
}

/// 委派模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DelegationMode {
    /// 进程内委派（`LocalAgentBus`）：一次性、临时上下文。
    #[default]
    Local,
    /// 经网关委派（`GatewayAgentBus`）：可达另一个 agent 节点，可取消、可归因。
    Gateway,
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
