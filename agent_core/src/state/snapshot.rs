use crate::protocol::*;
use crate::state::store::AgentStore;
use std::collections::HashSet;

/// 根据当前内存状态组装一份高轻量、高性能的可渲染快照
pub fn generate_snapshot(store: &AgentStore) -> ClientSnapshot {
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
                items: if need_full { t.items.clone() } else { Vec::new() },
                messages: Vec::new(),
                mode: t.mode,
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
    }
}
