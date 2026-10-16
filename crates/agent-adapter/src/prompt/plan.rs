//! 提示词计划：**有序片段**，而不是一段拼好的大字符串（P2 · SP-41）。
//!
//! # 为什么拆成片段
//!
//! 原来提示词是 `compose_system_prompt_full` 里一串 `push_str` 拼出来的：**谁加的、
//! 占多少字节**都埋在那段代码里，插件要加一段话只能改适配器。拆成片段之后：
//! 每段有 `id` / `source` / `order`，于是"这段是谁加的、多少字节"变成**可读数据**
//! （埋点 SP-43 与诊断面板直接读它），插件片段（SP-44）也只是往计划里追加一条。
//!
//! # 分隔符归**片段自己**
//!
//! `render()` 是**纯拼接**（`join("")`），不自己插空行。理由：这样"提示词一个字节都不变"
//! 是**可验证**的——黄金夹具（`tests/fixtures/prompt/*.txt`，取自重构前的输出）逐字节比对；
//! 若由拼接规则插入分隔符，就得靠"新代码的规则恰好等于老代码的排版"这种口算来保证。

/// 提示词里的一个片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFragment {
    /// 稳定标识（`persona` / `workspace` / `guidelines` / `language` / `extensions` /
    /// `skills_header` / `skill:<名字>`…）——埋点与诊断按它指认"是哪一段"
    pub id: String,
    /// 来源标签（`builtin:core` / `plugin:<id>`）；适配器不依赖 runtime 的类型，所以是字符串
    pub source: String,
    /// 排序权重（小的在前；同值按 `id` 稳定兜底）
    pub order: i32,
    /// 片段正文（**含它自己的分隔符**，见模块说明）
    pub content: String,
}

/// 一条片段的字节报告（SP-43 的埋点直接读它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSegmentReport {
    pub id: String,
    pub source: String,
    pub bytes: usize,
}

/// 提示词计划。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptPlan {
    fragments: Vec<PromptFragment>,
}

impl PromptPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一段（调用方保证顺序；`render` 前会按 `order` → `id` 稳定排序）。
    pub fn push(&mut self, fragment: PromptFragment) {
        self.fragments.push(fragment);
    }

    /// 拼成最终提示词：**纯拼接**，空片段贡献 0 字节。
    ///
    /// 排序是**稳定**的（`order` → `id`）：片段顺序变了就是行为变更，不是重构。
    pub fn render(&self) -> String {
        let mut sorted: Vec<&PromptFragment> = self.fragments.iter().collect();
        sorted.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        sorted
            .into_iter()
            .map(|f| f.content.as_str())
            .collect::<String>()
    }

    /// 片段清单（**按渲染顺序**，与 `render()` 一致）。
    pub fn fragments(&self) -> Vec<&PromptFragment> {
        let mut sorted: Vec<&PromptFragment> = self.fragments.iter().collect();
        sorted.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        sorted
    }

    pub fn byte_report(&self) -> Vec<PromptSegmentReport> {
        self.fragments()
            .into_iter()
            .map(|f| PromptSegmentReport {
                id: f.id.clone(),
                source: f.source.clone(),
                bytes: f.content.len(),
            })
            .collect()
    }

    pub fn total_bytes(&self) -> usize {
        self.fragments.iter().map(|f| f.content.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_is_plain_concatenation_in_stable_order() {
        let mut plan = PromptPlan::new();
        // 乱序加入 + 同 order 的两段（按 id 兜底）
        plan.push(PromptFragment {
            id: "b".into(),
            source: "builtin:core".into(),
            order: 10,
            content: "B".into(),
        });
        plan.push(PromptFragment {
            id: "a".into(),
            source: "builtin:core".into(),
            order: 10,
            content: "A".into(),
        });
        plan.push(PromptFragment {
            id: "z".into(),
            source: "plugin:p1".into(),
            order: 20,
            content: "Z".into(),
        });
        plan.push(PromptFragment {
            id: "empty".into(),
            source: "plugin:p1".into(),
            order: 30,
            content: String::new(),
        });

        assert_eq!(plan.render(), "ABZ", "纯拼接 + order→id 稳定排序 + 空段零贡献");
        let report = plan.byte_report();
        assert_eq!(report[0].id, "a");
        assert_eq!(report[0].source, "builtin:core");
        assert_eq!(report[0].bytes, 1);
        assert_eq!(plan.total_bytes(), 3);
    }
}
