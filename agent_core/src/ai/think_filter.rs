#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ThinkFilterPart {
    Text(String),
    Thinking(String),
}

/// 流式提取正文中的 `<think>...</think>` 标签，将其分离为 thinking 与 text 增量。
/// 兼容本地 Ollama、vLLM、LM Studio 等直接在 content 字段输出思考标签的模型。
#[derive(Debug, Default)]
pub struct ThinkTagFilter {
    in_think: bool,
    buffer: String,
}

impl ThinkTagFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, content: &str) -> Vec<ThinkFilterPart> {
        let mut results = Vec::new();
        let mut text = format!("{}{}", self.buffer, content);
        self.buffer.clear();

        while !text.is_empty() {
            if !self.in_think {
                let lower = text.to_lowercase();
                if let Some(start_idx) = lower.find("<think>") {
                    if start_idx > 0 {
                        results.push(ThinkFilterPart::Text(text[..start_idx].to_string()));
                    }
                    self.in_think = true;
                    text = text[start_idx + 7..].to_string();
                } else {
                    // 检查末尾是否是不完整的 `<think>` 前缀
                    let mut found_prefix = false;
                    for prefix in &["<think", "<thin", "<thi", "<th", "<t", "<"] {
                        if lower.ends_with(prefix) {
                            let cut_idx = text.len() - prefix.len();
                            self.buffer = text[cut_idx..].to_string();
                            if cut_idx > 0 {
                                results.push(ThinkFilterPart::Text(text[..cut_idx].to_string()));
                            }
                            text.clear();
                            found_prefix = true;
                            break;
                        }
                    }
                    if !found_prefix {
                        results.push(ThinkFilterPart::Text(std::mem::take(&mut text)));
                    }
                }
            } else {
                let lower = text.to_lowercase();
                if let Some(end_idx) = lower.find("</think>") {
                    if end_idx > 0 {
                        results.push(ThinkFilterPart::Thinking(text[..end_idx].to_string()));
                    }
                    self.in_think = false;
                    text = text[end_idx + 8..].to_string();
                } else {
                    // 检查末尾是否是不完整的 `</think>` 前缀
                    let mut found_prefix = false;
                    for prefix in &["</think", "</thin", "</thi", "</th", "</t", "</", "<"] {
                        if lower.ends_with(prefix) {
                            let cut_idx = text.len() - prefix.len();
                            self.buffer = text[cut_idx..].to_string();
                            if cut_idx > 0 {
                                results.push(ThinkFilterPart::Thinking(text[..cut_idx].to_string()));
                            }
                            text.clear();
                            found_prefix = true;
                            break;
                        }
                    }
                    if !found_prefix {
                        results.push(ThinkFilterPart::Thinking(std::mem::take(&mut text)));
                    }
                }
            }

        }

        results
    }

    pub fn flush(&mut self) -> Vec<ThinkFilterPart> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let res = if self.in_think {
            vec![ThinkFilterPart::Thinking(self.buffer.clone())]
        } else {
            vec![ThinkFilterPart::Text(self.buffer.clone())]
        };
        self.buffer.clear();
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_think_tag_filter_plain_text() {
        let mut filter = ThinkTagFilter::new();
        let parts = filter.feed("Hello world");
        assert_eq!(parts, vec![ThinkFilterPart::Text("Hello world".to_string())]);
        assert_eq!(filter.flush(), vec![]);
    }

    #[test]
    fn test_think_tag_filter_extraction() {
        let mut filter = ThinkTagFilter::new();
        let parts = filter.feed("<think>Let me think</think>Hello!");
        assert_eq!(
            parts,
            vec![
                ThinkFilterPart::Thinking("Let me think".to_string()),
                ThinkFilterPart::Text("Hello!".to_string())
            ]
        );
    }

    #[test]
    fn test_think_tag_filter_split_across_chunks() {
        let mut filter = ThinkTagFilter::new();
        let p1 = filter.feed("Prefix <th");
        assert_eq!(p1, vec![ThinkFilterPart::Text("Prefix ".to_string())]);

        let p2 = filter.feed("ink>thinking body</th");
        assert_eq!(p2, vec![ThinkFilterPart::Thinking("thinking body".to_string())]);

        let p3 = filter.feed("ink>Final text");
        assert_eq!(p3, vec![ThinkFilterPart::Text("Final text".to_string())]);
    }
}
