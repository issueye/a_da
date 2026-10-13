use agent_proto::*;
use crate::state::store::AgentStore;
use std::collections::HashSet;

/// 快照里**单个文本字段**的上限。
///
/// 为什么要有：合帧是**全量快照**，界面里每个工具卡片的输出都会随每一帧重发。
/// `cmd_tools` 的滑动窗口上限是 1MB——一条大构建日志就能让**每一帧**都背上 1MB，
/// 乘以每秒几十帧，序列化 + IPC + 前端解析全部爆掉（实测一帧 1.85 MB、渲染进程 1.5 GB）。
/// 这里只**限制快照投影**：内存与磁盘上的会话文件仍保留全文，不丢数据。
pub const SNAPSHOT_FIELD_LIMIT: usize = 32 * 1024;

/// 按上限截断一个字段，并如实说明"截了多少、全文在哪"。
fn capped(text: &str, field: &str) -> Option<String> {
    if text.len() <= SNAPSHOT_FIELD_LIMIT {
        return None;
    }
    // 按 UTF-8 字符边界切，别把多字节字符劈成乱码
    let mut cut = SNAPSHOT_FIELD_LIMIT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    Some(format!(
        "{}\n\n…（`{field}` 在本帧已截断：原始 {} KB，完整内容见会话文件）",
        &text[..cut],
        text.len() / 1024
    ))
}

/// 投影用的 item 瘦身：只截**可能极大**的字段（工具输出 / diff / 参数原文）。
pub(crate) fn shrink_item_for_snapshot(mut item: Item) -> Item {
    if let Item::Tool { output, patch, raw_args, .. } = &mut item {
        if let Some(o) = output.as_deref().and_then(|s| capped(s, "output")) {
            *output = Some(o);
        }
        if let Some(p) = patch.as_deref().and_then(|s| capped(s, "patch")) {
            *patch = Some(p);
        }
        if let Some(r) = capped(raw_args, "rawArgs") {
            *raw_args = r;
        }
    }
    item
}

/// 根据当前内存状态组装一份高轻量、高性能的可渲染快照（seq 默认为 0）
pub fn generate_snapshot(store: &AgentStore) -> ClientSnapshot {
    generate_snapshot_with_seq(store, 0)
}

/// 组装带单调递增版本序号的可渲染快照（M3-T2）
pub fn generate_snapshot_with_seq(store: &AgentStore, seq: u64) -> ClientSnapshot {
    let active_id = &store.active_id;
    let open_tabs: HashSet<&str> = store.ui.open_tab_ids.iter().map(|s| s.as_str()).collect();
    let running: HashSet<&str> = store.running_thread_ids.iter().map(|s| s.as_str()).collect();

    // 内存瘦身：
    // 1. messages 恒置空（UI 渲染层不消费模型底层的 messages 历史）
    // 2. 只有激活会话、打开的 Tab、运行中的会话或子智能体会话才带完整 items
    //    侧边栏折叠的历史会话仅保留元数据，避免海量历史卡片序列化垃圾
    let threads = store
        .threads
        .iter()
        .map(|t| {
            let need_full = t.id == *active_id
                || open_tabs.contains(t.id.as_str())
                || running.contains(t.id.as_str())
                || t.is_subagent.unwrap_or(false);

            Thread {
                id: t.id.clone(),
                title: t.title.clone(),
                created_at: t.created_at,
                workspace: t.workspace.clone(),
                items: if need_full {
                    t.items.iter().cloned().map(shrink_item_for_snapshot).collect()
                } else {
                    Vec::new()
                },
                messages: Vec::new(),
                mode: t.mode,
                agent_id: t.agent_id.clone(),
                parent_id: t.parent_id.clone(),
                subagent_id: t.subagent_id.clone(),
                is_subagent: t.is_subagent,
                plugin_data: t.plugin_data.clone(),
            }
        })
        .collect();

    // 日志精简：调试面板未展开时只给最近 10 条轻量条目
    let log = if store.ui.debug_open {
        store.log.clone()
    } else {
        let len = store.log.len();
        if len > 10 {
            store.log[len - 10..].to_vec()
        } else {
            store.log.clone()
        }
    };

    ClientSnapshot {
        seq,
        threads,
        active_thread_id: store.active_id.clone(),
        running_thread_ids: store.running_thread_ids.clone(),
        waiting_thread_ids: store.waiting_thread_ids.clone(),
        queue: store.queue.clone(),
        log,
        workspace: store.workspace.clone(),
        config: store.config.clone(),
        pending_questions: store.pending_questions.clone(),
        public_workspace: store.public_workspace.clone(),
        appearance: store.appearance.clone(),
        ui: store.ui.clone(),
        providers: store.providers.clone(),
        active_provider_id: store.active_provider_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentStore;

    /// 🔴 快照必须给超大字段限幅，但**内存里的原文不许动**。
    ///
    /// 实测：一帧 1.85 MB（一条长会话 822 个 item），其中单个 `run_command`
    /// 输出就能到 1 MB（`cmd_tools` 的滑动窗口上限）——全量快照每帧重发它，
    /// 每秒几十帧就是上百 MB/s，渲染进程内存被打到 1.5 GB。
    #[test]
    fn test_snapshot_caps_huge_tool_output_but_keeps_the_original() {
        let mut store = AgentStore::new("E:/snap_cap_ws".to_string());
        let tid = store.create_thread(Some("大输出".to_string()), None);
        let huge = "構".repeat(40_000); // 多字节：验证按字符边界切
        store.start_tool_call(&tid, "call_big", "run_command", "{}");
        store.finish_tool_call(
            &tid,
            "call_big",
            true,
            Some(huge.clone()),
            Some(9),
            Some(0),
            Some(9),
            None,
        );

        let snap = generate_snapshot(&store);
        let sent = snap
            .threads
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                Item::Tool { call_id, output, .. } if call_id == "call_big" => output.clone(),
                _ => None,
            })
            .expect("快照里应有该工具卡");
        assert!(
            sent.len() <= SNAPSHOT_FIELD_LIMIT + 256,
            "快照里的输出必须被限幅（实际 {} 字节）",
            sent.len()
        );
        assert!(sent.contains("本帧已截断"), "截断要如实说明：{sent:.80}");
        assert!(sent.contains("完整内容见会话文件"), "要指路哪里能看全文");

        // 内存里的原文**不许**被改（截断只作用于投影）
        let in_memory = store
            .threads
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                Item::Tool { call_id, output, .. } if call_id == "call_big" => output.clone(),
                _ => None,
            })
            .unwrap();
        assert_eq!(in_memory.len(), huge.len(), "内存里的全文必须原样保留（不丢数据）");
    }

    /// 小输出不该被动（不制造无谓的差异帧）
    #[test]
    fn test_snapshot_keeps_small_output_untouched() {
        let mut store = AgentStore::new("E:/snap_small_ws".to_string());
        let tid = store.create_thread(Some("小输出".to_string()), None);
        store.start_tool_call(&tid, "call_s", "run_command", "{}");
        store.finish_tool_call(&tid, "call_s", true, Some("ok".to_string()), Some(1), Some(0), Some(1), None);
        let snap = generate_snapshot(&store);
        let out = snap
            .threads
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                Item::Tool { call_id, output, .. } if call_id == "call_s" => output.clone(),
                _ => None,
            })
            .unwrap();
        assert_eq!(out, "ok", "没超限就不许改内容");
    }
}
