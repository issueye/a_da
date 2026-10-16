//! 待办清单的**差异**（P3b · SP-63）：纯函数，可独立测。
//!
//! # 为什么是纯函数
//!
//! `afterTodoUpdate` 要报"这次改动到底改了什么"。而"改了什么"最容易写错的地方是
//! **同一位置的前后对应**（按标题匹配还是按 id？完成又被改回未完成算不算单独一类？）。
//! 把它抽成不碰任何状态的纯函数，就能把这些歧义一条条钉在用例里，
//! 而不是等界面上出现一个说不清的 diff 再去猜。
//!
//! # 匹配口径
//!
//! 按 **id 优先、否则按标题** 配对：
//! - 有 `id` 的清单（模型通常给 claude 风格的 `{id, title, status}`）按 id 配对，
//!   于是"改标题"能被识别成 `title_changed` 而不是"删一条加一条"；
//! - 没有 id 时退回按标题配对（旧格式的清单就是 `{title, status}`）。
//!
//! # 设计里点名要单列的一类
//!
//! **完成又被改回未完成**（`reopened`）：这正是"清单被来回改"最值得警惕的信号
//! （设计 §5.4 明确要求把它从 `status_changed` 里单列出来）。

use serde::{Deserialize, Serialize};

/// 一条待办（工具收到的形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub status: String,
}

impl TodoStep {
    /// 配对键：有 id 用 id，否则用标题。
    fn key(&self) -> String {
        match &self.id {
            Some(id) if !id.trim().is_empty() => format!("id:{}", id.trim()),
            _ => format!("title:{}", self.title.trim()),
        }
    }

    pub fn is_done(&self) -> bool {
        self.status.eq_ignore_ascii_case("completed") || self.status.eq_ignore_ascii_case("done")
    }
}

/// 两份清单之间的差异。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoDiff {
    /// 新增的标题
    pub added: Vec<String>,
    /// 删掉的标题
    pub removed: Vec<String>,
    /// 标题被改了：`(旧, 新)`
    pub title_changed: Vec<(String, String)>,
    /// 状态被改了：`(标题, 旧, 新)`
    pub status_changed: Vec<(String, String, String)>,
    /// **完成又被改回未完成**（单列，见模块说明）
    pub reopened: Vec<String>,
}

impl TodoDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.title_changed.is_empty()
            && self.status_changed.is_empty()
            && self.reopened.is_empty()
    }

    /// 一行话的描述（进 `afterTodoUpdate` 的旁注，模型也会看到）。
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "清单无变化".to_string();
        }
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("新增 {} 条（{}）", self.added.len(), self.added.join("、")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("移除 {} 条（{}）", self.removed.len(), self.removed.join("、")));
        }
        if !self.title_changed.is_empty() {
            let items: Vec<String> = self
                .title_changed
                .iter()
                .map(|(a, b)| format!("{a} → {b}"))
                .collect();
            parts.push(format!("改标题 {} 条（{}）", items.len(), items.join("；")));
        }
        if !self.status_changed.is_empty() {
            let items: Vec<String> = self
                .status_changed
                .iter()
                .map(|(t, a, b)| format!("{t}: {a} → {b}"))
                .collect();
            parts.push(format!("改状态 {} 条（{}）", items.len(), items.join("；")));
        }
        if !self.reopened.is_empty() {
            parts.push(format!(
                "⚠ 完成又被改回未完成 {} 条（{}）",
                self.reopened.len(),
                self.reopened.join("、")
            ));
        }
        parts.join("；")
    }
}

/// 比较两份清单。
pub fn diff_todos(old: &[TodoStep], new: &[TodoStep]) -> TodoDiff {
    use std::collections::BTreeMap;

    let mut diff = TodoDiff::default();
    let old_map: BTreeMap<String, &TodoStep> = old.iter().map(|s| (s.key(), s)).collect();
    let new_map: BTreeMap<String, &TodoStep> = new.iter().map(|s| (s.key(), s)).collect();

    for (key, step) in &new_map {
        match old_map.get(key) {
            None => diff.added.push(step.title.clone()),
            Some(prev) => {
                if prev.title.trim() != step.title.trim() {
                    diff.title_changed
                        .push((prev.title.clone(), step.title.clone()));
                }
                if prev.status != step.status {
                    if prev.is_done() && !step.is_done() {
                        diff.reopened.push(step.title.clone());
                    } else {
                        diff.status_changed.push((
                            step.title.clone(),
                            prev.status.clone(),
                            step.status.clone(),
                        ));
                    }
                }
            }
        }
    }
    for (key, step) in &old_map {
        if !new_map.contains_key(key) {
            diff.removed.push(step.title.clone());
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(title: &str, status: &str) -> TodoStep {
        TodoStep {
            id: None,
            title: title.to_string(),
            status: status.to_string(),
        }
    }

    fn step_id(id: &str, title: &str, status: &str) -> TodoStep {
        TodoStep {
            id: Some(id.to_string()),
            title: title.to_string(),
            status: status.to_string(),
        }
    }

    /// 增删改三类都要如实分类（含"改标题"而不是"删一条加一条"）。
    #[test]
    fn test_classifies_add_remove_and_title_change() {
        let old = vec![step("甲", "pending"), step("乙", "pending")];
        let new = vec![step("甲改", "pending"), step("丙", "pending")];
        let d = diff_todos(&old, &new);
        // 无 id 时按**标题**配对，所以"甲→甲改"必然表现为删+加——这不是缺陷，
        // 而是"没有稳定标识就认不出同一条"。有 id 时的差别见下面半段。
        assert_eq!(d.added, vec!["丙".to_string(), "甲改".to_string()]);
        assert_eq!(d.removed, vec!["乙".to_string(), "甲".to_string()]);
        assert!(d.title_changed.is_empty());

        let old = vec![step_id("1", "甲", "pending")];
        let new = vec![step_id("1", "甲改", "pending")];
        let d = diff_todos(&old, &new);
        assert_eq!(d.title_changed, vec![("甲".to_string(), "甲改".to_string())]);
        assert!(d.added.is_empty() && d.removed.is_empty(), "有 id 时不该算增删");
    }

    /// **完成又被改回未完成**必须单列（设计 §5.4 点名的那一类）。
    #[test]
    fn test_reopened_is_singled_out() {
        let old = vec![step("甲", "completed")];
        let new = vec![step("甲", "pending")];
        let d = diff_todos(&old, &new);
        assert_eq!(d.reopened, vec!["甲".to_string()]);
        assert!(
            d.status_changed.is_empty(),
            "往回改不算普通状态变更（否则它会被淹在正常推进里）"
        );
        assert!(d.describe().contains("完成又被改回"), "{}", d.describe());

        // 正向推进（未完成 → 完成）走普通分类
        let d2 = diff_todos(&new, &old);
        assert!(d2.reopened.is_empty());
        assert_eq!(d2.status_changed.len(), 1);
    }

    /// 完全一样 → 空差异（`afterTodoUpdate` 据此报"无变化"，而不是编点东西出来）。
    #[test]
    fn test_identical_lists_produce_no_diff() {
        let l = vec![step("甲", "pending")];
        let d = diff_todos(&l, &l);
        assert!(d.is_empty());
        assert_eq!(d.describe(), "清单无变化");
    }

    /// 顺序变了不算变化（按 key 配对，不是按位置）。
    #[test]
    fn test_reordering_is_not_a_change() {
        let old = vec![step("甲", "pending"), step("乙", "completed")];
        let new = vec![step("乙", "completed"), step("甲", "pending")];
        assert!(diff_todos(&old, &new).is_empty(), "顺序不是内容");
    }

    /// `done` 也认成"完成"（清单里两种写法都有人用）。
    #[test]
    fn test_done_synonym_counts_as_completed() {
        let old = vec![step("甲", "done")];
        let new = vec![step("甲", "pending")];
        assert_eq!(diff_todos(&old, &new).reopened, vec!["甲".to_string()]);
    }
}
