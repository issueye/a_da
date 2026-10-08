//! 决策引擎抽象与启发式实现（AGENTS.md §9: 绝不捏造确定性，calibrated 严格标为 false）

use std::collections::HashMap;
use std::time::Instant;
use crate::decision::types::{
    DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionResponse, EngineId, QuestionType,
};

/// 决策引擎契约 trait
pub trait DecisionEngine: Send + Sync {
    fn id(&self) -> EngineId;
    fn evaluate(&self, req: &DecisionRequest) -> Result<DecisionResponse, String>;
}

/// 确定性启发式决策引擎（无外部网络/模型依赖，离线快速确定性求解）
#[derive(Debug, Default, Clone)]
pub struct HeuristicDecisionEngine;

impl HeuristicDecisionEngine {
    pub fn new() -> Self {
        Self
    }

    fn state_to_text(state: &serde_json::Value) -> String {
        match state {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        }
    }

    fn evaluate_noul(state_text: &str, instructions: &str) -> (f64, Option<f64>) {
        let state_lower = state_text.to_lowercase();
        let _inst_lower = instructions.to_lowercase();

        // 正向与负向指标关键词
        let positive_signals = ["pass", "success", "ok", "true", "valid", "完成", "通过", "符合", "正确", "通过验收"];
        let negative_signals = ["fail", "error", "bug", "broken", "false", "invalid", "失败", "报错", "未通过", "不符合", "未完成"];

        let mut pos_count = 0;
        let mut neg_count = 0;

        for p in positive_signals {
            if state_lower.contains(p) {
                pos_count += 1;
            }
        }
        for n in negative_signals {
            if state_lower.contains(n) {
                neg_count += 1;
            }
        }

        // 默认基础概率
        let prob = if pos_count == 0 && neg_count == 0 {
            if state_text.trim().is_empty() {
                0.1 // 空材料给低概率
            } else {
                0.6 // 正常材料无报错倾向通过
            }
        } else {
            let total = (pos_count + neg_count) as f64;
            (pos_count as f64) / total
        };

        let confidence = ((pos_count + neg_count) as f64 * 0.2).min(0.9).max(0.5);
        (prob, Some(confidence))
    }

    fn evaluate_choice(
        state_text: &str,
        criteria: &HashMap<String, Option<String>>,
    ) -> (String, HashMap<String, f64>) {
        let state_lower = state_text.to_lowercase();
        let mut scores: HashMap<String, f64> = HashMap::new();

        for (key, desc) in criteria {
            let mut score = 0.0;
            if state_lower.contains(&key.to_lowercase()) {
                score += 2.0;
            }
            if let Some(desc_str) = desc {
                for word in desc_str.split_whitespace() {
                    if state_lower.contains(&word.to_lowercase()) {
                        score += 1.0;
                    }
                }
            }
            scores.insert(key.clone(), score);
        }

        let total_score: f64 = scores.values().sum();
        let mut dist = HashMap::new();
        let mut best_key = criteria.keys().next().cloned().unwrap_or_default();
        let mut max_score = -1.0;

        for (k, s) in &scores {
            let p = if total_score > 0.0 { *s / total_score } else { 1.0 / criteria.len().max(1) as f64 };
            dist.insert(k.clone(), (p * 100.0).round() / 100.0);
            if *s > max_score {
                max_score = *s;
                best_key = k.clone();
            }
        }

        (best_key, dist)
    }

    fn evaluate_score(state_text: &str, criteria: &[String]) -> (String, HashMap<String, f64>) {
        if criteria.is_empty() {
            return ("未知".into(), HashMap::new());
        }
        let state_lower = state_text.to_lowercase();
        let mut matched_level = criteria.last().unwrap().clone();

        for level in criteria {
            if state_lower.contains(&level.to_lowercase()) {
                matched_level = level.clone();
                break;
            }
        }

        let mut dist = HashMap::new();
        for level in criteria {
            dist.insert(level.clone(), if level == &matched_level { 0.8 } else { 0.2 / criteria.len() as f64 });
        }

        (matched_level, dist)
    }
}

impl DecisionEngine for HeuristicDecisionEngine {
    fn id(&self) -> EngineId {
        EngineId::Heuristic
    }

    fn evaluate(&self, req: &DecisionRequest) -> Result<DecisionResponse, String> {
        let start = Instant::now();
        let state_text = Self::state_to_text(&req.state);
        let mut answers = HashMap::new();

        for (id, q) in &req.questions {
            match q {
                DecisionQuestion::Noul { instructions } => {
                    let (prob, conf) = Self::evaluate_noul(&state_text, instructions);
                    answers.insert(
                        id.clone(),
                        DecisionAnswer {
                            question_type: QuestionType::Noul,
                            value: serde_json::json!(prob),
                            confidence: conf,
                            distribution: None,
                            // 关键约束：启发式判定绝非校准结果，必须是 false
                            calibrated: false,
                        },
                    );
                }
                DecisionQuestion::Choice { instructions: _, criteria } => {
                    let (best, dist) = Self::evaluate_choice(&state_text, criteria);
                    answers.insert(
                        id.clone(),
                        DecisionAnswer {
                            question_type: QuestionType::Choice,
                            value: serde_json::json!(best),
                            confidence: Some(0.6),
                            distribution: Some(dist),
                            calibrated: false,
                        },
                    );
                }
                DecisionQuestion::Score { instructions: _, criteria } => {
                    let (level, dist) = Self::evaluate_score(&state_text, criteria);
                    answers.insert(
                        id.clone(),
                        DecisionAnswer {
                            question_type: QuestionType::Score,
                            value: serde_json::json!(level),
                            confidence: Some(0.6),
                            distribution: Some(dist),
                            calibrated: false,
                        },
                    );
                }
            }
        }

        Ok(DecisionResponse {
            answers,
            engine: EngineId::Heuristic,
            model: None,
            elapsed_ms: start.elapsed().as_millis() as u64,
            samples: 1,
            notes: vec!["启发式引擎分析结果未经深度模型校准 (calibrated=false)".into()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_engine_noul() {
        let engine = HeuristicDecisionEngine::new();
        let mut questions = HashMap::new();
        questions.insert(
            "q1".into(),
            DecisionQuestion::Noul {
                instructions: "测试是否通过".into(),
            },
        );

        let req_pass = DecisionRequest {
            state: serde_json::json!("all tests pass, 100% ok"),
            questions: questions.clone(),
            threshold: None,
        };
        let res_pass = engine.evaluate(&req_pass).expect("判定必须成功");
        let ans_pass = &res_pass.answers["q1"];
        assert!(!ans_pass.calibrated, "必须绝不捏造校准确定性");
        assert!(ans_pass.value.as_f64().unwrap() >= 0.7);

        let req_fail = DecisionRequest {
            state: serde_json::json!("compilation failed, 3 errors found"),
            questions,
            threshold: None,
        };
        let res_fail = engine.evaluate(&req_fail).expect("判定必须成功");
        let ans_fail = &res_fail.answers["q1"];
        assert!(ans_fail.value.as_f64().unwrap() <= 0.3);
    }
}
