//! `PromptSource` 端口契约合规断言（INV-2）。
//!
//! 断言点：
//! 1. 系统提示词不可为空或纯空白——空提示词会让模型丢掉人格与工作区上下文；
//! 2. **幂等**：同一实例连续调用必须给出同一文本（不允许"每次调用都变"，那会让
//!    历史回放与截图对不上）；
//! 3. 不得残留未替换的模板占位（`{{` / `}}`）。

use agent_base::ports::PromptSource;

/// 验证 `PromptSource` 端口契约。
pub fn verify_prompt_source_contract(source: &dyn PromptSource) -> Result<(), String> {
    let first = source.system_prompt();
    if first.trim().is_empty() {
        return Err("系统提示词不可为空或纯空白".into());
    }

    let second = source.system_prompt();
    if first != second {
        return Err("system_prompt() 必须幂等：同一实例连续调用给出了不同文本".into());
    }

    if first.contains("{{") || first.contains("}}") {
        return Err("系统提示词里残留未替换的模板占位（{{ / }}）".into());
    }

    // P2·SP-43：逐段报告（若非空）必须与总长度**自洽**。
    //
    // 不自洽的拆分比不拆更坏：埋点会把"这段 200 字节"报出去，排查者照着去追一个
    // 不存在的差异。空 = 这一份不细分，是允许的（测试替身/子智能体 profile）。
    let segments = source.prompt_segments();
    if !segments.is_empty() {
        let sum: usize = segments.iter().map(|s| s.bytes).sum();
        if sum != first.len() {
            return Err(format!(
                "prompt_segments() 各段字节之和（{sum}）必须等于 system_prompt().len()（{}）",
                first.len()
            ));
        }
        if segments.iter().any(|s| s.id.trim().is_empty()) {
            return Err("prompt_segments() 里存在空 id——埋点要靠它指认是哪一段".into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::FixedPrompt;

    /// 真实实现（不是 double）也要过同一份契约。
    #[test]
    fn test_real_coding_prompt_source_conformance() {
        let source = agent_adapter::prompt::CodingPromptSource::new("E:/conformance_project");
        verify_prompt_source_contract(&source).expect("真实 CodingPromptSource 必须合规");
        assert!(
            source.system_prompt().contains("E:/conformance_project"),
            "提示词必须写明工作区根目录"
        );
    }

    #[test]
    fn test_double_conformance() {
        let source = FixedPrompt::new("你是测试助手。");
        verify_prompt_source_contract(&source).expect("替身必须合规");
    }

    /// 反向用例：契约必须能挡住空提示词与残留占位（守住"断言不是空转"）。
    #[test]
    fn test_contract_rejects_bad_implementations() {
        struct Blank;
        impl PromptSource for Blank {
            fn system_prompt(&self) -> String {
                "   \n\t ".to_string()
            }
            fn prompt_segments(&self) -> Vec<agent_base::ports::PromptSegment> {
                Vec::new()
            }
        }
        struct Unrendered;
        impl PromptSource for Unrendered {
            fn system_prompt(&self) -> String {
                "你好，{{persona}}".to_string()
            }
            fn prompt_segments(&self) -> Vec<agent_base::ports::PromptSegment> {
                Vec::new()
            }
        }

        assert!(
            verify_prompt_source_contract(&Blank).is_err(),
            "空提示词必须被判不合规"
        );
        assert!(
            verify_prompt_source_contract(&Unrendered).is_err(),
            "残留模板占位必须被判不合规"
        );
    }
}
