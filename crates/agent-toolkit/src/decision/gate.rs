//! 门禁判定核心（Fail-close 安全默认：验收标准不可被静默绕过）

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use crate::decision::engine::DecisionEngine;
use crate::decision::types::{DecisionQuestion, DecisionRequest, GateOutcome, GateSource};

pub const DEFAULT_GATE_THRESHOLD: f64 = 0.65;
pub const MAX_DIFF_CHARS: usize = 20_000;

#[derive(Debug, Clone)]
pub struct GateOptions {
    pub criteria: String,
    pub source: GateSource,
    pub file: Option<String>,
    pub text: Option<String>,
    pub threshold: Option<f64>,
    pub fail_open: bool,
    pub workspace: PathBuf,
}

/// 执行 git diff 获取代码变更
pub fn read_git_diff(workspace: &Path) -> String {
    let try_diff = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .ok()?;
        if output.status.success() {
            String::from_utf8(output.stdout).ok()
        } else {
            None
        }
    };

    if let Some(working) = try_diff(&["diff", "HEAD"]) {
        if !working.trim().is_empty() {
            return working;
        }
    }

    if let Some(staged) = try_diff(&["diff", "--cached"]) {
        if !staged.trim().is_empty() {
            return staged;
        }
    }

    String::new()
}

/// 解析门禁材料
pub fn resolve_gate_state(options: &GateOptions) -> Result<(String, Option<String>), String> {
    match options.source {
        GateSource::Text => Ok((options.text.clone().unwrap_or_default(), None)),
        GateSource::File => {
            let rel = options.file.as_deref().ok_or("source=file 时必须提供 file 参数")?;
            if rel.contains("..") {
                return Err("禁止跨目录访问沙箱外文件".into());
            }
            let full = options.workspace.join(rel);
            let content = std::fs::read_to_string(&full)
                .map_err(|e| format!("读取门禁目标文件失败 ({}): {}", full.display(), e))?;
            Ok((content, None))
        }
        GateSource::Diff => {
            let diff = read_git_diff(&options.workspace);
            if diff.trim().is_empty() {
                return Ok((
                    String::new(),
                    Some("工作区未检测到 git 改动（git diff 与 --cached 均为空）".into()),
                ));
            }
            if diff.len() > MAX_DIFF_CHARS {
                let note = format!("diff 过大 ({} 字符)，已截断至 {} 字符", diff.len(), MAX_DIFF_CHARS);
                let truncated = diff.chars().take(MAX_DIFF_CHARS).collect();
                return Ok((truncated, Some(note)));
            }
            Ok((diff, None))
        }
    }
}

/// 执行门禁校验
pub fn run_gate(options: GateOptions, engine: &dyn DecisionEngine) -> GateOutcome {
    let start = Instant::now();
    let threshold = options.threshold.unwrap_or(DEFAULT_GATE_THRESHOLD);

    let (state_text, prep_note) = match resolve_gate_state(&options) {
        Ok(res) => res,
        Err(e) => {
            // 解析失败：默认 fail-close
            return GateOutcome {
                passed: options.fail_open,
                probability: 0.0,
                threshold,
                criteria: options.criteria,
                engine: engine.id(),
                calibrated: false,
                elapsed_ms: start.elapsed().as_millis() as u64,
                note: Some(format!("材料准备失败: {} (fail_open={})", e, options.fail_open)),
            };
        }
    };

    let mut questions = HashMap::new();
    questions.insert(
        "gate".into(),
        DecisionQuestion::Noul {
            instructions: options.criteria.clone(),
        },
    );

    let req = DecisionRequest {
        state: serde_json::Value::String(state_text),
        questions,
        threshold: Some(threshold),
    };

    match engine.evaluate(&req) {
        Ok(res) => {
            let ans = &res.answers["gate"];
            let prob = ans.value.as_f64().unwrap_or(0.0);
            let passed = prob >= threshold;

            let mut notes = Vec::new();
            if let Some(n) = prep_note {
                notes.push(n);
            }
            notes.extend(res.notes);

            GateOutcome {
                passed,
                probability: (prob * 1000.0).round() / 1000.0,
                threshold,
                criteria: options.criteria,
                engine: res.engine,
                calibrated: ans.calibrated,
                elapsed_ms: start.elapsed().as_millis() as u64,
                note: if notes.is_empty() { None } else { Some(notes.join("; ")) },
            }
        }
        Err(e) => GateOutcome {
            // 引擎判定失败：默认安全策略为 fail-close（不放行）
            passed: options.fail_open,
            probability: 0.0,
            threshold,
            criteria: options.criteria,
            engine: engine.id(),
            calibrated: false,
            elapsed_ms: start.elapsed().as_millis() as u64,
            note: Some(format!("引擎评估失败: {} (fail_close 默认拦截)", e)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::engine::HeuristicDecisionEngine;

    #[test]
    fn test_gate_fail_close_on_error() {
        let engine = HeuristicDecisionEngine::new();
        let options = GateOptions {
            criteria: "代码必须编译无报错".into(),
            source: GateSource::File,
            file: Some("non_existent_file.rs".into()),
            text: None,
            threshold: None,
            fail_open: false, // 严格默认
            workspace: PathBuf::from("."),
        };

        let outcome = run_gate(options, &engine);
        assert!(!outcome.passed, "文件读取失败时默认必须 fail-close 不放行");
        assert!(outcome.note.unwrap().contains("材料准备失败"));
    }

    #[test]
    fn test_gate_pass_with_positive_text() {
        let engine = HeuristicDecisionEngine::new();
        let options = GateOptions {
            criteria: "单元测试全部通过".into(),
            source: GateSource::Text,
            file: None,
            text: Some("running 10 tests... test result: ok. 10 passed; 0 failed".into()),
            threshold: Some(0.6),
            fail_open: false,
            workspace: PathBuf::from("."),
        };

        let outcome = run_gate(options, &engine);
        assert!(outcome.passed, "达标文本必须通过门禁");
        assert!(!outcome.calibrated, "绝不伪造校准确定性");
    }
}
