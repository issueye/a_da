//! 决策与门禁数据类型定义（AGENTS.md §9, 遵守"绝不捏造确定性"原则）

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// 问题类型：choice（多选一）、noul（是否概率）、score（档位评分）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    Choice,
    Noul,
    Score,
}

/// 决策问题定义
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DecisionQuestion {
    /// 从若干选项中选一个。criteria 为「选项 key → 选项说明」
    Choice {
        instructions: String,
        criteria: HashMap<String, Option<String>>,
    },
    /// 是/否概率问题。语义是「这件事成立的概率」
    Noul {
        instructions: String,
    },
    /// 按评分档位打分。criteria 为档位描述，最高档在前
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

impl DecisionQuestion {
    pub fn question_type(&self) -> QuestionType {
        match self {
            Self::Choice { .. } => QuestionType::Choice,
            Self::Noul { .. } => QuestionType::Noul,
            Self::Score { .. } => QuestionType::Score,
        }
    }

    pub fn instructions(&self) -> &str {
        match self {
            Self::Choice { instructions, .. } => instructions,
            Self::Noul { instructions } => instructions,
            Self::Score { instructions, .. } => instructions,
        }
    }
}

/// 决策输入请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// 被判断的材料（文本或结构化数据）
    pub state: serde_json::Value,
    /// 问题标识 -> 问题定义
    pub questions: HashMap<String, DecisionQuestion>,
    /// 单题覆盖阈值（默认 0.65）
    #[serde(default)]
    pub threshold: Option<f64>,
}

/// 引擎标识：远端 Jev / 本地模型自评 / 确定性启发式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineId {
    Jev,
    Local,
    Heuristic,
}

impl EngineId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Local => "local",
            Self::Heuristic => "heuristic",
        }
    }
}

/// 单题答案
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionAnswer {
    #[serde(rename = "type")]
    pub question_type: QuestionType,
    /// choice 为选项 key；noul 为概率 0.0~1.0；score 为档位名
    pub value: serde_json::Value,
    /// 置信度（0.0~1.0）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// 选项/档位分布
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distribution: Option<HashMap<String, f64>>,
    /// 诚实性落点：是否经过正式校准。
    /// 远端 Jev（System One 专用模型）= true；本地模型自评与启发式 = false。
    pub calibrated: bool,
}

/// 决策整体响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub answers: HashMap<String, DecisionAnswer>,
    pub engine: EngineId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub elapsed_ms: u64,
    pub samples: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// 门禁判定来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateSource {
    Diff,
    File,
    Text,
}

/// 门禁判定最终结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateOutcome {
    pub passed: bool,
    pub probability: f64,
    pub threshold: f64,
    pub criteria: String,
    pub engine: EngineId,
    pub calibrated: bool,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
