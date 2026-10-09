//! 会话 JSONL 的**行级读写与折叠语义**（唯一真源）。
//!
//! 为什么单独一层：`agent-core` 的 `SessionManager`（W3 之前的过渡实现）与适配器的
//! [`super::fs_store::FsSessionStore`]（端口实现）必须读写**同一套**格式与**同一套**折叠语义。
//! 折叠语义（`compact` 记录之后的上下文如何收敛）只能有一份，否则两处必然分叉（R2）。
//!
//! # ⚠️ 落盘格式的一个坑（已用断言钉住）
//!
//! `SessionEntry` 是 `#[serde(tag = "type")]` 的**内部标签枚举**，而各条目结构体自己也有一个
//! 重命名为 `type` 的字段。于是：
//!
//! - **写**：必须序列化**内层结构体**（`SessionMessageEntry` 等）→ 一行一个 `"type"` 键；
//! - **读**：走 `SessionEntry` 枚举反序列化（标签被枚举消费，内层字段回落到 `default`）。
//!
//! 反过来写（把 `SessionEntry::Message(x)` 整个序列化）会产出
//! `{"type":"message","type":"message",...}` —— **重复键**，读回时整行解析失败、
//! 被"坏行跳过"逻辑吃掉，表现为**会话静默变空**。
//! `test_written_line_has_exactly_one_type_key` 就是钉这个的。

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use agent_base::domain::AgentMessage;
use anyhow::{Context, Result};
use serde::Serialize;

use super::types::{
    SessionCompactEntry, SessionEntry, SessionHeader, SessionMessageEntry, SessionNoticeEntry,
};

/// 一次读取的结果：条目 + 被跳过的坏行数。
///
/// 坏行**不致命**（追加写遇上进程崩溃会留半行），但**必须可观测**——
/// 静默跳过就是"看起来读到了其实少了东西"。
#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub entries: Vec<SessionEntry>,
    pub skipped_lines: usize,
}

/// 追加一行条目（自动建父目录）。
fn append_serialized<T: Serialize>(path: &Path, entry: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建会话目录失败: {}", parent.display()))?;
        }
    }
    let line = serde_json::to_string(entry)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("打开会话文件失败: {}", path.display()))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// 追加一条 `message` 条目。
pub fn append_message(path: &Path, entry: &SessionMessageEntry) -> Result<()> {
    append_serialized(path, entry)
}

/// 追加一条 `compact` 条目。
pub fn append_compact(path: &Path, entry: &SessionCompactEntry) -> Result<()> {
    append_serialized(path, entry)
}

/// 追加一条 `notice` 条目。
pub fn append_notice(path: &Path, entry: &SessionNoticeEntry) -> Result<()> {
    append_serialized(path, entry)
}

/// 读取全部条目（坏行计数如实返回）。
pub fn read_entries(path: &Path) -> Result<ReadOutcome> {
    let file =
        File::open(path).with_context(|| format!("打开会话文件失败: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut outcome = ReadOutcome::default();

    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            outcome.skipped_lines += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionEntry>(trimmed) {
            Ok(entry) => outcome.entries.push(entry),
            Err(_) => outcome.skipped_lines += 1,
        }
    }
    Ok(outcome)
}

/// 把条目折叠成 `(header, messages)`。
///
/// 语义（与搬迁前的 `SessionManager::load_session` 逐条一致）：
/// - `Header` → 取最后一个作为会话头；
/// - `Message` → 进消息列表；
/// - `Notice` → **不进**模型上下文；
/// - `Compact` → **清空**此前的消息，只留一条压缩摘要续接消息。
pub fn fold_messages(entries: &[SessionEntry]) -> (Option<SessionHeader>, Vec<AgentMessage>) {
    let mut header: Option<SessionHeader> = None;
    let mut messages: Vec<AgentMessage> = Vec::new();

    for entry in entries {
        match entry {
            SessionEntry::Header(h) => header = Some(h.clone()),
            SessionEntry::Message(m) => messages.push(m.message.clone()),
            SessionEntry::Notice(_) => {}
            SessionEntry::Compact(c) => {
                messages.clear();
                messages.push(AgentMessage::User {
                    content: format!("[系统自动压缩摘要]\n{}", c.summary),
                    images: None,
                    timestamp: Some(c.timestamp),
                });
            }
        }
    }

    (header, messages)
}

/// 造一条 `message` 条目（id 规则与搬迁前一致：`msg_{毫秒}_{6 位随机}`）。
pub fn new_message_entry(now_ms: i64, message: AgentMessage) -> SessionMessageEntry {
    let random_suffix = uuid::Uuid::new_v4().to_string();
    SessionMessageEntry {
        entry_type: "message".to_string(),
        id: format!("msg_{}_{}", now_ms, &random_suffix[..6]),
        timestamp: now_ms,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("a_da_jsonl_{tag}_{}_{}.jsonl", std::process::id(), nanos))
    }

    fn message_entry(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            entry_type: "message".to_string(),
            id: id.to_string(),
            timestamp: 1,
            message: AgentMessage::User {
                content: text.to_string(),
                images: None,
                timestamp: Some(1),
            },
        }
    }

    fn compact_entry(summary: &str) -> SessionCompactEntry {
        SessionCompactEntry {
            entry_type: "compact".to_string(),
            id: "c1".to_string(),
            timestamp: 9,
            summary: summary.to_string(),
            pre_tokens: 100,
            post_tokens: 10,
            saved_tokens: 90,
            turns_summarized: 3,
            custom_instructions: None,
        }
    }

    /// **核心守门**：落盘行里 `"type"` 必须恰好出现一次。
    ///
    /// 这条断言是 W1-T2a 的真实收获：第一版实现把 `SessionEntry` 枚举整个序列化，
    /// 产出了 `{"type":"message","type":"message",...}`，读回时整行失败、
    /// 被"坏行跳过"吃掉 → 会话静默变空（6 个测试同时红）。
    #[test]
    fn test_written_line_has_exactly_one_type_key() {
        let path = tmp_file("one_type");
        append_message(&path, &message_entry("m1", "x")).expect("追加失败");
        let line = std::fs::read_to_string(&path).expect("读取失败");
        let line = line.trim();
        assert_eq!(
            line.matches("\"type\"").count(),
            1,
            "落盘行必须只有一个 type 键（重复键会让读回整行失败）：{line}"
        );
        assert!(line.starts_with("{\"type\":\"message\""), "行首形状不对: {line}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_append_then_read_roundtrip() {
        let path = tmp_file("roundtrip");
        append_message(&path, &message_entry("m1", "第一句")).expect("追加失败");
        append_message(&path, &message_entry("m2", "第二句")).expect("追加失败");

        let outcome = read_entries(&path).expect("读取失败");
        assert_eq!(outcome.skipped_lines, 0, "不该有坏行");
        assert_eq!(outcome.entries.len(), 2, "两行都应被读回");
        let (_, messages) = fold_messages(&outcome.entries);
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], AgentMessage::User { content, .. } if content == "第一句"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compact_clears_prior_context() {
        let path = tmp_file("compact");
        append_message(&path, &message_entry("m1", "很久以前")).expect("追加失败");
        append_compact(&path, &compact_entry("前面聊了很多")).expect("追加失败");

        let outcome = read_entries(&path).expect("读取失败");
        assert_eq!(outcome.entries.len(), 2, "两条都应被读回");
        let (_, messages) = fold_messages(&outcome.entries);
        assert_eq!(messages.len(), 1, "compact 之后只应剩一条续接消息");
        assert!(
            matches!(&messages[0], AgentMessage::User { content, .. } if content.contains("前面聊了很多")),
            "续接消息必须带压缩摘要"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_broken_line_is_skipped_but_counted() {
        let path = tmp_file("broken");
        append_message(&path, &message_entry("m1", "好行")).expect("追加失败");
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{\"type\":\"message\",\"id\":\"half\"").unwrap();
        }
        append_message(&path, &message_entry("m2", "又一条好行")).expect("追加失败");

        let outcome = read_entries(&path).expect("坏行不应导致整体失败");
        assert_eq!(outcome.entries.len(), 2, "好行应照常读回");
        assert_eq!(outcome.skipped_lines, 1, "坏行必须被计数（不许静默）");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_notice_does_not_enter_model_context() {
        let path = tmp_file("notice");
        append_notice(
            &path,
            &SessionNoticeEntry {
                entry_type: "notice".to_string(),
                id: "n1".to_string(),
                timestamp: 2,
                text: "提示".to_string(),
                level: "info".to_string(),
            },
        )
        .expect("追加失败");

        let outcome = read_entries(&path).expect("读取失败");
        assert_eq!(outcome.entries.len(), 1, "notice 行本身要读得回来");
        let (_, messages) = fold_messages(&outcome.entries);
        assert!(messages.is_empty(), "Notice 不得进入模型上下文");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_header_line_is_readable_and_folded() {
        let path = tmp_file("header");
        append_serialized(
            &path,
            &super::super::types::SessionHeader {
                entry_type: "session".to_string(),
                version: 1,
                id: "s1".to_string(),
                title: Some("标题".to_string()),
                workspace: "E:/proj".to_string(),
                created_at: 1,
                updated_at: 2,
                parent_id: None,
                subagent_id: None,
                plugin_data: None,
            },
        )
        .expect("追加失败");

        let outcome = read_entries(&path).expect("读取失败");
        assert_eq!(outcome.skipped_lines, 0, "header 行必须可读");
        let (header, messages) = fold_messages(&outcome.entries);
        let header = header.expect("应解析出会话头");
        assert_eq!(header.id, "s1");
        assert_eq!(header.title.as_deref(), Some("标题"));
        assert!(messages.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_message_entry_id_shape() {
        let e = new_message_entry(1234, AgentMessage::Unknown);
        assert!(e.id.starts_with("msg_1234_"), "id 前缀与规则不符: {}", e.id);
        assert_eq!(e.id.len(), "msg_1234_".len() + 6, "随机后缀应为 6 位");
        assert_eq!(e.entry_type, "message");
        assert_eq!(e.timestamp, 1234);
    }
}
