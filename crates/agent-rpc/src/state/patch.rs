//! **增量状态帧的差量计算**（`evt.state.patch` 的心脏）。
//!
//! 全量快照的问题（实测）：一条长会话 822 个 item、单帧 **1852 KB**；合成负载 800 item、
//! 单帧 **3705 KB**。16ms 一帧（大帧已降到 100ms）也要每秒几十 MB 的序列化 + IPC + 前端解析，
//! 渲染进程内存被顶到 GB 级。真正的大头是 `items`——所以只推**变过**的 item 就能拿掉绝大部分。
//!
//! 这里刻意做成**纯函数 + 不接生产路径**：先把"怎么算差量"用测试钉死，
//! 再接宿主发送与客户端合并（顺序见 docs/state-patch-plan.md）。
//!
//! 指纹复用 `store::item_visible_key`——它本来就是为"判断这条 item 可不可能变了"设计的
//! （id + 文本长度 + 状态 + 是否流式 + 完成时刻），比序列化整条 item 再哈希便宜得多。

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};

use agent_proto::Item;

/// item 的身份键：`(threadId, itemId)`。
pub type ItemKey = (String, String);

/// 取一条 item 的身份 id（`Item` 的六个变体都带 `id`，但没有公共字段）。
pub fn item_id(it: &Item) -> &str {
    match it {
        Item::User { id, .. }
        | Item::Thinking { id, .. }
        | Item::Assistant { id, .. }
        | Item::Tool { id, .. }
        | Item::Notice { id, .. }
        | Item::Compact { id, .. } => id,
    }
}

/// 一次差量的结果。
#[derive(Debug, Default)]
pub struct ItemDelta {
    /// 新增或变更的 item（按 `(threadId, itemId)` 排序，保证帧内容确定）
    pub upserted: Vec<(String, Item)>,
    /// 已消失的 item（按身份键排序）
    pub removed: Vec<ItemKey>,
}

impl ItemDelta {
    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.removed.is_empty()
    }
}

/// 一条 item 的可见指纹（不可见的内部改动不算变化，避免无谓刷帧）。
pub fn fingerprint(item: &Item) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::state::store::item_visible_key(item).hash(&mut hasher);
    hasher.finish()
}

/// 算出"相对上次发给这个客户端的版本"的差量。
///
/// - `prev`：上次发出去的 `{(threadId,itemId) → 指纹}`（新连接传空表 → 全部当新增）；
/// - `current`：本次要投影的 item 列表（**同一个 thread 的 item 也要带 threadId**）。
///
/// 语义：
/// - 指纹相同 → **不发**（这是省掉 99% 字节的地方）；
/// - `prev` 里有、`current` 里没有 → `removed`（卡片被删/会话被清）；
/// - 其余 → `upserted`。
pub fn diff_items(prev: &BTreeMap<ItemKey, u64>, current: &[(String, Item)]) -> ItemDelta {
    let mut delta = ItemDelta::default();
    let mut seen: HashSet<ItemKey> = HashSet::new();

    for (thread_id, item) in current {
        let key = (thread_id.clone(), item_id(item).to_string());
        // 同一帧里出现重复 id：以最后一条为准，但不重复推送
        if !seen.insert(key.clone()) {
            if let Some(slot) = delta
                .upserted
                .iter_mut()
                .find(|(tid, it)| tid == thread_id && item_id(it) == item_id(item))
            {
                *slot = (thread_id.clone(), item.clone());
            }
            continue;
        }
        match prev.get(&key) {
            Some(fp) if *fp == fingerprint(item) => {} // 没变：不推
            _ => delta.upserted.push((thread_id.clone(), item.clone())),
        }
    }

    for key in prev.keys() {
        if !seen.contains(key) {
            delta.removed.push(key.clone());
        }
    }

    // 帧内容必须**确定**：顺序取决于 map 迭代顺序的话，客户端 diff 与日志都不可复现
    delta
        .upserted
        .sort_by(|a, b| (a.0.as_str(), item_id(&a.1)).cmp(&(b.0.as_str(), item_id(&b.1))));
    delta.removed.sort();
    delta
}

/// 用本次差量更新"已发版本"，供下一次 `diff_items` 使用。
pub fn advance_versions(prev: &mut BTreeMap<ItemKey, u64>, current: &[(String, Item)]) {
    let live: HashSet<ItemKey> = current
        .iter()
        .map(|(tid, it)| (tid.clone(), item_id(it).to_string()))
        .collect();
    prev.retain(|k, _| live.contains(k));
    for (thread_id, item) in current {
        prev.insert((thread_id.clone(), item_id(item).to_string()), fingerprint(item));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::Item;

    fn user(id: &str, text: &str) -> Item {
        Item::User {
            id: id.to_string(),
            at: 1,
            text: text.to_string(),
            images: None,
            queued: None,
        }
    }

    /// 没变就不推——这是省字节的全部意义所在。
    #[test]
    fn test_unchanged_items_are_not_resent() {
        let items = vec![
            ("t1".to_string(), user("a", "第一句")),
            ("t1".to_string(), user("b", "第二句")),
        ];
        let mut prev = BTreeMap::new();
        advance_versions(&mut prev, &items);

        let delta = diff_items(&prev, &items);
        assert!(delta.is_empty(), "完全没变就一字节都不该推：{delta:?}");
    }

    /// 只有变过的那一条进 upserted。
    #[test]
    fn test_only_changed_item_is_upserted() {
        let before = vec![
            ("t1".to_string(), user("a", "第一句")),
            ("t1".to_string(), user("b", "第二句")),
        ];
        let mut prev = BTreeMap::new();
        advance_versions(&mut prev, &before);

        let after = vec![
            ("t1".to_string(), user("a", "第一句")),
            ("t1".to_string(), user("b", "第二句改了")),
        ];
        let delta = diff_items(&prev, &after);
        assert_eq!(delta.upserted.len(), 1, "只该推变过的那一条：{delta:?}");
        assert_eq!(item_id(&delta.upserted[0].1), "b");
        assert!(delta.removed.is_empty());
    }

    /// 消失的 item 必须显式 removed（否则客户端会一直留着一张幽灵卡片）。
    #[test]
    fn test_missing_item_is_reported_as_removed() {
        let before = vec![
            ("t1".to_string(), user("a", "甲")),
            ("t1".to_string(), user("b", "乙")),
        ];
        let mut prev = BTreeMap::new();
        advance_versions(&mut prev, &before);

        let after = vec![("t1".to_string(), user("a", "甲"))];
        let delta = diff_items(&prev, &after);
        assert_eq!(delta.removed, vec![("t1".to_string(), "b".to_string())]);
        assert!(delta.upserted.is_empty());
    }

    /// 新连接：prev 空 → 全部当新增（等价于一次全量 items，但走同一条合并路径）。
    #[test]
    fn test_empty_prev_sends_everything_once() {
        let items = vec![
            ("t1".to_string(), user("a", "甲")),
            ("t2".to_string(), user("a", "另一个会话")),
        ];
        let delta = diff_items(&BTreeMap::new(), &items);
        assert_eq!(delta.upserted.len(), 2);
        assert!(delta.removed.is_empty());
    }

    /// 帧内容必须确定：同样的输入两次得到同样的顺序。
    #[test]
    fn test_delta_is_deterministic() {
        let items = vec![
            ("t2".to_string(), user("z", "1")),
            ("t1".to_string(), user("b", "2")),
            ("t1".to_string(), user("a", "3")),
        ];
        let d1 = diff_items(&BTreeMap::new(), &items);
        let d2 = diff_items(&BTreeMap::new(), &items);
        assert_eq!(format!("{d1:?}"), format!("{d2:?}"), "顺序不确定会让日志/ diff 不可复现");
        assert_eq!(
            d1.upserted
                .iter()
                .map(|(t, i)| format!("{t}/{}", item_id(i)))
                .collect::<Vec<_>>(),
            vec!["t1/a", "t1/b", "t2/z"]
        );
    }

    /// 同一个 item 的进度更新（指纹里的文本长度/状态变了）必须被推。
    #[test]
    fn test_streaming_growth_is_pushed() {
        let mut prev = BTreeMap::new();
        advance_versions(&mut prev, &[("t1".to_string(), user("a", "半句"))]);
        let delta = diff_items(&prev, &[("t1".to_string(), user("a", "半句加了下半句"))]);
        assert_eq!(delta.upserted.len(), 1, "流式增长必须推给界面");
    }
}
