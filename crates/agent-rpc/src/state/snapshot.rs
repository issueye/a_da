use agent_proto::*;
use crate::state::store::AgentStore;
use std::sync::Arc;

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
///
/// 就地版本：增量帧的 `upserted` 已经是克隆出来的可写副本，直接在它上面截，
/// 不必先构造一个新 Item（超大输出时那等于再复制一次几 MB）。
pub(crate) fn shrink_item_in_place(item: &mut Item) {
    if let Item::Tool { output, patch, raw_args, .. } = item {
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
}

/// 快照投影：**共享**未超限的卡片（只 clone 一个 `Arc`），超限的才复制一份再截断。
///
/// 这是 `Arc<Item>` 改造的收益所在：一条几百张卡的长会话，首屏快照不再逐张深拷贝；
/// 只有真正带超大工具输出的那几张（被限幅改写）需要一次 copy。
pub(crate) fn shrink_item_arc(item: &Arc<Item>) -> Arc<Item> {
    let needs_cap = match item.as_ref() {
        Item::Tool { output, patch, raw_args, .. } => {
            output.as_deref().map(|s| s.len() > SNAPSHOT_FIELD_LIMIT).unwrap_or(false)
                || patch.as_deref().map(|s| s.len() > SNAPSHOT_FIELD_LIMIT).unwrap_or(false)
                || raw_args.len() > SNAPSHOT_FIELD_LIMIT
        }
        _ => false,
    };
    if !needs_cap {
        return Arc::clone(item);
    }
    let mut owned = (**item).clone();
    shrink_item_in_place(&mut owned);
    Arc::new(owned)
}

/// 根据当前内存状态组装一份高轻量、高性能的可渲染快照（seq 默认为 0）
pub fn generate_snapshot(store: &AgentStore) -> ClientSnapshot {
    generate_snapshot_with_seq(store, 0)
}

/// 组装带单调递增版本序号的可渲染快照（M3-T2）
pub fn generate_snapshot_with_seq(store: &AgentStore, seq: u64) -> ClientSnapshot {
    generate_snapshot_inner(store, seq, true)
}

/// **只带元数据**的快照（各会话的 `items` 一律为空）。
///
/// 增量帧（`evt.state.patch`）的 `meta` 与全量快照同形，但客户端只消费会话元数据。
/// 用它就不必"先把所有 items 克隆一遍、再把 items 清空"——那正好是流式期间最贵的两笔。
/// `itemCount` / `updatedAt` 仍从内存里的完整 items 算（界面靠它们判断会话有没有内容）。
pub fn generate_snapshot_meta(store: &AgentStore, seq: u64) -> ClientSnapshot {
    generate_snapshot_inner(store, seq, false)
}

fn generate_snapshot_inner(store: &AgentStore, seq: u64, include_items: bool) -> ClientSnapshot {
    // 内存瘦身：
    // 1. messages 恒置空（UI 渲染层不消费模型底层的 messages 历史）
    // 2. 只有激活会话、打开的 Tab、运行中的会话或子智能体会话才带完整 items
    //    侧边栏折叠的历史会话仅保留元数据，避免海量历史卡片序列化垃圾
    //
    // 🔴 **被裁剪的两个替代字段必须照实填**：`itemCount` / `updatedAt`。
    //    界面（agent-web 侧栏）拿它们判断"这个会话有没有内容"和"最近活动"——
    //    一旦这两个字段也跟着缺省成 0，历史会话就会被当成空会话清出列表。
    let threads = store
        .threads
        .iter()
        .map(|t| {
            let need_full = include_items && store.projects_thread_items(t);

            Thread {
                id: t.id.clone(),
                title: t.title.clone(),
                created_at: t.created_at,
                workspace: t.workspace.clone(),
                items: if need_full {
                    t.items.iter().map(shrink_item_arc).collect()
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
                // 计数与时刻都来自**内存里的完整 items**（裁剪只发生在序列化这一步）
                item_count: t.items.len(),
                updated_at: t
                    .items
                    .iter()
                    .map(|item| item.at())
                    .max()
                    .unwrap_or(0)
                    .max(t.created_at),
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
            .find_map(|i| match i.as_ref() {
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
            .find_map(|i| match i.as_ref() {
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
            .find_map(|i| match i.as_ref() {
                Item::Tool { call_id, output, .. } if call_id == "call_s" => output.clone(),
                _ => None,
            })
            .unwrap();
        assert_eq!(out, "ok", "没超限就不许改内容");
    }

    /// 🔴 裁剪 `items` 时，`itemCount` / `updatedAt` 必须照实给。
    ///
    /// 侧栏靠这两个字段判断"这条会话有没有内容"和"最近活动"：一旦它们跟着缺省成 0，
    /// 全部历史会话都会被当成空会话清出列表——实测用户看到的就是
    /// "当前工作区只剩一条新会话"，以为历史丢了。
    #[test]
    fn test_snapshot_keeps_counts_and_activity_for_trimmed_threads() {
        let mut store = AgentStore::new("E:/snap_trim_ws".to_string());
        let old = store.create_thread(Some("历史会话".to_string()), None);
        let last_at = {
            let thread = store
                .threads
                .iter_mut()
                .find(|t| t.id == old)
                .expect("刚建的会话应在 store 里");
            // 卡片时刻取"创建之后"，否则会被空会话的 `created_at` 兜底盖住
            // （那正是本用例要防的退化）。
            let created = thread.created_at;
            let last_at = created + 2_000;
            thread.items.push(Arc::new(Item::User {
                id: "u1".to_string(),
                at: created + 1_000,
                text: "你好".to_string(),
                images: None,
                queued: None,
            }));
            thread.items.push(Arc::new(Item::Assistant {
                id: "a1".to_string(),
                at: last_at,
                text: "收到".to_string(),
                streaming: None,
                duration_ms: None,
                turn_duration_ms: None,
                usage: None,
            }));
            last_at
        };
        // 再建一条 → 它成为激活会话；把上一条从"打开的 Tab"里摘掉，
        // 它才会落进"非激活会话"那一档（items 被裁剪，只剩元数据）。
        let active = store.create_thread(Some("当前会话".to_string()), None);
        store.close_tab(&old);
        let snap = generate_snapshot(&store);

        let trimmed = snap
            .threads
            .iter()
            .find(|t| t.id == old)
            .expect("历史会话仍在快照里");
        assert!(trimmed.items.is_empty(), "非激活会话的 items 应被裁剪（体积）");
        assert_eq!(
            trimmed.item_count, 2,
            "条目数必须照实给：裁剪只作用于 items，计数不能跟着变 0"
        );
        assert_eq!(
            trimmed.updated_at, last_at,
            "最近活动取最后一条 item 的 at（不能退化成创建时刻）"
        );

        let current = snap
            .threads
            .iter()
            .find(|t| t.id == active)
            .expect("激活会话在快照里");
        assert_eq!(current.item_count, 0);
        assert!(
            current.updated_at >= current.created_at,
            "空会话的最近活动回退到创建时刻"
        );
    }

    /// 增量帧的 meta 快照：**一条 items 都不带**，但 `itemCount` / `updatedAt` 必须照实。
    ///
    /// 它替代了"先克隆全部 items、再把 items 清空"的老路径（流式期间每帧一笔大开销）。
    /// 如果计数/时刻跟着丢，侧栏会把有内容的会话判成空会话。
    #[test]
    fn test_meta_snapshot_drops_items_but_keeps_counts() {
        let mut store = AgentStore::new("E:/snap_meta_ws".to_string());
        let tid = store.create_thread(Some("带内容的会话".to_string()), None);
        let at = store
            .threads
            .iter()
            .find(|t| t.id == tid)
            .map(|t| t.created_at)
            .unwrap_or(1)
            + 5_000;
        store.threads.iter_mut().find(|t| t.id == tid).unwrap().items.push(Arc::new(Item::User {
            id: "u1".to_string(),
            at,
            text: "你好".to_string(),
            images: None,
            queued: None,
        }));

        let meta = generate_snapshot_meta(&store, 7);
        let thread = meta
            .threads
            .iter()
            .find(|t| t.id == tid)
            .expect("激活会话必须在 meta 里");
        assert!(thread.items.is_empty(), "meta 帧不许带 items（它只负责会话元数据）");
        assert_eq!(thread.item_count, 1, "条目数必须照实给");
        assert_eq!(thread.updated_at, at, "最近活动必须照实给");
        assert_eq!(meta.seq, 7);

        // 对照：完整快照仍然带 items（页面首屏/自愈要用）
        let full = generate_snapshot_with_seq(&store, 7);
        let thread = full.threads.iter().find(|t| t.id == tid).unwrap();
        assert_eq!(thread.items.len(), 1, "完整快照的激活会话必须带 items");
    }

    /// `Arc<Item>` 的全部意义：普通卡片在快照里与内存是**同一份**（零拷贝），
    /// 只有被限幅改写的超大卡片才复制一份。
    #[test]
    fn test_snapshot_shares_arcs_and_copies_only_capped_items() {
        let mut store = AgentStore::new("E:/snap_arc_ws".to_string());
        let tid = store.create_thread(Some("共享测试".to_string()), None);
        store.add_user_message(&tid, "你好");
        let huge = "x".repeat(SNAPSHOT_FIELD_LIMIT * 2);
        store.start_tool_call(&tid, "call_huge", "run_command", "{}");
        store.finish_tool_call(
            &tid,
            "call_huge",
            true,
            Some(huge.clone()),
            Some(1),
            Some(0),
            Some(1),
            None,
        );

        let snap = generate_snapshot(&store);
        let store_items = &store.threads.iter().find(|t| t.id == tid).unwrap().items;
        let snap_items = &snap.threads.iter().find(|t| t.id == tid).unwrap().items;
        assert_eq!(snap_items.len(), store_items.len(), "快照不许丢卡片");

        for (index, (sent, original)) in snap_items.iter().zip(store_items.iter()).enumerate() {
            let needs_cap = matches!(
                original.as_ref(),
                Item::Tool { output: Some(out), .. } if out.len() == huge.len()
            );
            if needs_cap {
                assert!(
                    !Arc::ptr_eq(sent, original),
                    "超限卡片必须先复制再截断（不能改到内存里的原文）"
                );
            } else {
                assert!(
                    Arc::ptr_eq(sent, original),
                    "第 {index} 张未超限卡片必须共享同一份 Arc（否则快照又变成整份深拷贝）"
                );
            }
        }

        let in_memory = store_items
            .iter()
            .find_map(|item| match item.as_ref() {
                Item::Tool { output: Some(out), .. } => Some(out.len()),
                _ => None,
            })
            .expect("内存里应有工具卡");
        assert_eq!(in_memory, huge.len(), "内存里的全文必须原样保留");
    }
}
