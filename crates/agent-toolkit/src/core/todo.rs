//! `todo` 工具：清单的**全量替换**。
//!
//! # 状态在哪
//!
//! **权威清单不在 Rust 侧**：模型每次调用都带全量 `todos`，界面据这次调用的参数渲染。
//! 工具只负责解析、归一化并回执，不维护跨调用状态。
//!
//! 原来的 `beforeTodoUpdate` / `afterTodoUpdate` 点位已移除：待办更新的拦截/旁注
//! 后续由插件事件系统实现，核心工具不再跑钩子。

use serde::{Deserialize, Serialize};

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

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
    pub fn is_done(&self) -> bool {
        self.status.eq_ignore_ascii_case("completed") || self.status.eq_ignore_ascii_case("done")
    }
}

/// `todo` 工具。
pub struct TodoTool {
    descriptor: ToolDescriptor,
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

        Self { descriptor }
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
        _ctx: &'a ToolContext<'a>,
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

            let (steps, parse_problem) = Self::parse_steps(&call.args);
            if let Some(problem) = parse_problem {
                return ToolReceipt::error(problem, started_at, crate::now_ms());
            }

            let output = if steps.is_empty() {
                "待办清单已清空。".to_string()
            } else {
                format!("待办清单状态已更新（共 {} 条）。", steps.len())
            };

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
