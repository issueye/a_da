//! `todo` 工具：清单的**全量替换**，并跑自己的两个点位（P3b · SP-63）。
//!
//! # 状态在哪
//!
//! **权威清单不在 Rust 侧**：模型每次调用都带全量 `todos`，界面据这次调用的参数渲染。
//! 所以这里只保留**每个线程上一次见到的清单**（只用于算 diff，不落盘、不给它"权威"地位）。
//! 重启后第一份快照没有历史可比，此时如实说"首个快照"，而不是把整张清单报成"全是新增"
//! ——那是最像数据、其实最误导的一种输出。
//!
//! # 点位（设计 §5.4 把实施位置定在"todo 工具内部"）
//!
//! - `beforeTodoUpdate`：载荷是全量清单。可 `Deny`（不更新，理由作为普通工具结果回模型）、
//!   可 `Replace`（**改写后重新校验**：必须还是"对象数组 + 非空标题"）。
//! - `afterTodoUpdate`：只追加旁注（带 [`crate::core::todo_diff::TodoDiff`] 的差异）。

use std::collections::HashMap;
use std::sync::Mutex;

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use agent_base::ports::hooks::{points, AfterNote, AfterRequest, BeforeOutcome, BeforeRequest};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::core::todo_diff::{diff_todos, TodoStep};

/// `todo` 工具。
pub struct TodoTool {
    descriptor: ToolDescriptor,
    /// 每个线程**上一次见到的**清单（只用于算 diff，见模块说明）。
    last_seen: Mutex<HashMap<String, Vec<TodoStep>>>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoTool {
    pub fn new() -> Self {
        let descriptor = crate::registry::find_tool_descriptor("todo")
            .expect("todo descriptor must be registered in agent-toolkit registry")
            .clone();

        Self {
            descriptor,
            last_seen: Mutex::new(HashMap::new()),
        }
    }

    /// 从参数里取清单。**形状不对就如实报错**（而不是当空清单继续跑——
    /// 拿一份空清单去问插件，只会得到误导性的答复）。
    ///
    /// 🔴 描述符（模型看到的 schema）用的是 `items: [{id,title,done}]`，
    /// 而早期实现只认 `todos: [{title,status}]`：模型按 schema 发 `items` 时
    /// 工具会把它当成"空清单已清空"，界面却仍从 `args.items` 画出计划——
    /// 工具回执与界面互相矛盾。这里两个字段都认，并把 `done` 归一成 `status`。
    fn parse_steps(args: &serde_json::Value) -> (Vec<TodoStep>, Option<String>) {
        let Some(raw) = args.get("todos").or_else(|| args.get("items")) else {
            return (Vec::new(), None);
        };
        match serde_json::from_value::<Vec<TodoStep>>(normalize_steps(raw)) {
            Ok(mut steps) => {
                steps.retain(|s| !s.title.trim().is_empty());
                (steps, None)
            }
            Err(e) => (
                Vec::new(),
                Some(format!(
                    "`todos` 不是合法清单（{e}）：期望 `[{{title, status}}]` 或 `[{{title, done}}]`"
                )),
            ),
        }
    }

    /// 改写后的清单**重新校验**（§5.6：改写不是授权）。
    fn revalidate_replacement(args: &serde_json::Value) -> Result<Vec<TodoStep>, String> {
        let steps_raw = args
            .get("todos")
            .or_else(|| args.get("items"))
            .ok_or_else(|| "改写必须给出 `todos` / `items` 字段".to_string())?;
        let steps: Vec<TodoStep> =
            serde_json::from_value(normalize_steps(steps_raw)).map_err(|e| {
                format!(
                    "改写后的清单不合法（{e}）：期望 `[{{title, status}}]` 或 `[{{title, done}}]`"
                )
            })?;
        if steps.iter().any(|s| s.title.trim().is_empty()) {
            return Err("改写后的清单里有空标题".to_string());
        }
        Ok(steps)
    }

    /// 取走并写入"上一次见到的清单"（返回**上一版**，用于算 diff）。
    fn remember(&self, thread_id: &str, steps: &[TodoStep]) -> Option<Vec<TodoStep>> {
        let mut map = self.last_seen.lock().ok()?;
        map.insert(thread_id.to_string(), steps.to_vec())
    }
}

/// 把旁注追进工具结果（与 `afterToolCall` 同一口径：模型读到的就是那一份文本）。
fn append_notes(output: &mut String, notes: &[AfterNote]) {
    let texts: Vec<&str> = notes
        .iter()
        .filter_map(|n| match n {
            AfterNote::Note { text } => Some(text.as_str()),
            AfterNote::Observe { .. } => None,
        })
        .collect();
    if !texts.is_empty() {
        output.push_str("\n\n[插件旁注] ");
        output.push_str(&texts.join("\n"));
    }
}

/// 把 `{title, done}`（描述符 schema）归一成 `{title, status}`（实现/回执形状）。
fn normalize_steps(raw: &serde_json::Value) -> serde_json::Value {
    let Some(array) = raw.as_array() else {
        return raw.clone();
    };
    let mut out = Vec::with_capacity(array.len());
    for entry in array {
        let mut value = entry.clone();
        if let Some(map) = value.as_object_mut() {
            if !map.contains_key("status") {
                let done = map.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                map.insert(
                    "status".to_string(),
                    serde_json::json!(if done { "completed" } else { "pending" }),
                );
            }
        }
        out.push(value);
    }
    serde_json::Value::Array(out)
}

impl Tool for TodoTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = crate::now_ms();
            let action = call
                .args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("get");

            // `get` 不改清单 → 不跑点位（点位名就叫 beforeTodo**Update**）
            if action == "get" {
                return ToolReceipt::success("当前待办列表已同步就绪。", started_at, crate::now_ms());
            }
            if !matches!(action, "set" | "update") {
                return ToolReceipt::success(
                    format!("未知待办操作: {action}"),
                    started_at,
                    crate::now_ms(),
                );
            }

            let (mut steps, parse_problem) = Self::parse_steps(&call.args);
            if let Some(problem) = parse_problem {
                return ToolReceipt::error(problem, started_at, crate::now_ms());
            }

            // ── beforeTodoUpdate（P3b·SP-63）───────────────────────────────
            let mut replaced_by: Option<String> = None;
            if let Some(hooks) = &ctx.hooks {
                let outcome = hooks
                    .runner
                    .before(
                        BeforeRequest {
                            point: points::BEFORE_TODO_UPDATE.to_string(),
                            subject: Some("todo".to_string()),
                            payload: serde_json::json!({ "todos": steps }),
                        },
                        &hooks.ctx(ctx.thread_id, ctx.cancel, hooks.on_event),
                    )
                    .await;
                match outcome {
                    BeforeOutcome::Pass => {}
                    BeforeOutcome::Deny { reason } => {
                        // 拦下即定稿：**清单不变**，理由作为普通工具结果回模型（可纠正）
                        return ToolReceipt::denied(reason, started_at, crate::now_ms());
                    }
                    BeforeOutcome::AbortTurn { reason } => {
                        // 工具内部没有"结束整轮"的能力（那要引擎才能做）：如实说明、按拦下处理
                        return ToolReceipt::denied(
                            format!("{reason}（注：todo 点位不支持结束整轮，已按拦下处理）"),
                            started_at,
                            crate::now_ms(),
                        );
                    }
                    BeforeOutcome::Replace { args: new_args, by } => {
                        match Self::revalidate_replacement(&new_args) {
                            Ok(new_steps) => {
                                steps = new_steps;
                                replaced_by = Some(by);
                            }
                            Err(reason) => {
                                // 改写过不了校验：**不改**，如实说清（改写不是授权）
                                return ToolReceipt::error(
                                    format!("插件改写清单被拒：{reason}"),
                                    started_at,
                                    crate::now_ms(),
                                );
                            }
                        }
                    }
                }
            }

            // ── 应用 ───────────────────────────────────────────────────────
            let previous = self.remember(ctx.thread_id, &steps);

            let mut output = if steps.is_empty() {
                "待办清单已清空。".to_string()
            } else {
                format!("待办清单状态已更新（共 {} 条）。", steps.len())
            };
            if let Some(by) = &replaced_by {
                // 改写必须让模型看见：它得知道清单**不是自己写的那份**
                output.push_str(&format!("\n（清单被插件 `{by}` 改写后生效）"));
            }

            // ── afterTodoUpdate（P3b·SP-63）────────────────────────────────
            if let Some(hooks) = &ctx.hooks {
                let diff_text = match &previous {
                    Some(prev) => diff_todos(prev, &steps).describe(),
                    None => "（首个快照：没有上一版清单可比）".to_string(),
                };
                let notes = hooks
                    .runner
                    .after(
                        AfterRequest {
                            point: points::AFTER_TODO_UPDATE.to_string(),
                            subject: Some("todo".to_string()),
                            payload: serde_json::json!({ "todos": steps, "diff": diff_text }),
                        },
                        &hooks.ctx(ctx.thread_id, ctx.cancel, hooks.on_event),
                    )
                    .await;
                append_notes(&mut output, &notes);
            }

            ToolReceipt::success(output, started_at, crate::now_ms())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_items_shape_advertised_by_the_tool_schema() {
        // 描述符里写的就是 items + done；模型照 schema 发，工具必须认。
        let args = json!({
            "action": "set",
            "items": [
                { "id": "t1", "title": "读 README", "done": true },
                { "id": "t2", "title": "写总结", "done": false }
            ]
        });
        let (steps, problem) = TodoTool::parse_steps(&args);
        assert!(problem.is_none(), "items 形状不该报错: {problem:?}");
        assert_eq!(steps.len(), 2);
        assert!(steps[0].is_done());
        assert!(!steps[1].is_done());
    }

    #[test]
    fn keeps_accepting_the_legacy_todos_shape() {
        let args = json!({
            "action": "update",
            "todos": [{ "id": "t1", "title": "读 README", "status": "completed" }]
        });
        let (steps, problem) = TodoTool::parse_steps(&args);
        assert!(problem.is_none());
        assert_eq!(steps.len(), 1);
        assert!(steps[0].is_done());
    }

    #[test]
    fn empty_items_are_an_explicit_clear_not_a_parse_error() {
        let args = json!({ "action": "set", "items": [] });
        let (steps, problem) = TodoTool::parse_steps(&args);
        assert!(problem.is_none());
        assert!(steps.is_empty());
    }
}
