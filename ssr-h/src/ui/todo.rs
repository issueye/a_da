//! 任务规划（`todo` 工具）的计划状态推演与 UI。
//!
//! 与 desktop-ui 的 `utils/todo-plan.ts` 保持同一套语义：
//!
//! - `set`：**替换**整份计划（基线）；
//! - `update`：**增量合并**（按 id 命中就改，没命中就追加）；
//! - `get`：**只读**，不改计划状态；
//! - 没写 `action`：当作 `set`。
//!
//! 为什么不"倒序找第一个能解析出 items 的卡片"：那样会把 `update` 的子集、
//! `get` 回的旧快照当成最新计划，面板会停在旧计划上（实测 7/12，正文却写着 13/13）。

use serde_json::Value;

use super::components::Render;
use super::icons::icon;
use super::theme::escape;

/// 一步任务的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn class(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "running",
            TodoStatus::Completed => "done",
        }
    }
}

/// 一步任务。
#[derive(Debug, Clone, Default)]
pub struct TodoStep {
    pub id: String,
    pub title: String,
    pub status: TodoStatus,
}

/// 整份计划。
#[derive(Debug, Clone, Default)]
pub struct TodoPlan {
    pub steps: Vec<TodoStep>,
    pub notes: Option<String>,
}

impl TodoPlan {
    pub fn completed(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == TodoStatus::Completed)
            .count()
    }

    pub fn all_done(&self) -> bool {
        !self.steps.is_empty() && self.completed() == self.steps.len()
    }

    /// 当前正在做的事：优先 in_progress，否则第一个未完成的。
    pub fn active_step(&self) -> Option<&TodoStep> {
        self.steps
            .iter()
            .find(|step| step.status == TodoStatus::InProgress)
            .or_else(|| self.steps.iter().find(|step| step.status != TodoStatus::Completed))
    }

    /// 复制用的 Markdown 任务清单。
    pub fn to_markdown(&self) -> String {
        let mut lines = vec![format!("### 任务规划步骤 ({}/{})", self.completed(), self.steps.len())];
        if let Some(notes) = &self.notes {
            if !notes.trim().is_empty() {
                lines.push(format!("> 备注：{notes}"));
            }
        }
        for step in &self.steps {
            let mark = if step.status == TodoStatus::Completed {
                "x"
            } else {
                " "
            };
            lines.push(format!("- [{mark}] {}", step.title));
        }
        lines.join("\n")
    }
}

fn normalize_status(step: &Value) -> TodoStatus {
    match step.get("status").and_then(Value::as_str) {
        Some("completed") => TodoStatus::Completed,
        Some("in_progress") => TodoStatus::InProgress,
        Some("pending") => TodoStatus::Pending,
        _ => {
            if step.get("done").and_then(Value::as_bool) == Some(true) {
                TodoStatus::Completed
            } else {
                TodoStatus::Pending
            }
        }
    }
}

fn parse_items(raw: Option<&Value>) -> Option<Vec<TodoStep>> {
    let array = raw?.as_array()?;
    Some(
        array
            .iter()
            .map(|entry| TodoStep {
                id: entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                title: entry
                    .get("title")
                    .or_else(|| entry.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status: normalize_status(entry),
            })
            .collect(),
    )
}

/// 从一张 `todo` 卡片里取出 `{action, items, notes}`。
///
/// 参数可能在 `args`（对象或 JSON 字符串）、`rawArgs`、输出或回执里，逐个试。
fn read_todo_call(item: &Value) -> Option<(String, Option<Vec<TodoStep>>, Option<String>)> {
    let mut sources: Vec<Value> = Vec::new();
    for key in ["args", "rawArgs", "output", "result", "details"] {
        if let Some(value) = item.get(key) {
            if !value.is_null() {
                sources.push(value.clone());
            }
        }
    }
    for source in sources {
        let parsed = match &source {
            Value::String(text) => {
                if !text.trim_start().starts_with('{') {
                    continue;
                }
                match serde_json::from_str::<Value>(text) {
                    Ok(value) => value,
                    Err(_) => continue,
                }
            }
            Value::Object(_) => source.clone(),
            _ => continue,
        };
        let items = parse_items(parsed.get("items")).or_else(|| parse_items(parsed.get("todos")));
        let action = parsed
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if items.is_none() && action.is_empty() {
            continue;
        }
        let notes = parsed
            .get("notes")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Some((action, items, notes));
    }
    None
}

/// 从一张 `todo` 卡片里读出步骤（工具行展开区用）。
pub fn read_todo_items(item: &Value) -> Option<(Vec<TodoStep>, Option<String>)> {
    let (_, items, notes) = read_todo_call(item)?;
    items.map(|steps| (steps, notes))
}

/// 推演整条会话**当前**的计划状态。返回 `None` = 这条会话没规划过。
pub fn derive_todo_plan(items: &[Value]) -> Option<TodoPlan> {
    if items.is_empty() {
        return None;
    }
    let mut plan: Vec<TodoStep> = Vec::new();
    let mut notes: Option<String> = None;
    let mut seen = false;

    for item in items {
        if str_at(item, "name") != "todo" && str_at(item, "tool") != "todo" {
            continue;
        }
        let Some((action, items, call_notes)) = read_todo_call(item) else {
            continue;
        };
        seen = true;
        if let Some(text) = call_notes {
            notes = Some(text);
        }
        let action = if action.is_empty() {
            "set".to_string()
        } else {
            action.to_lowercase()
        };
        if action == "get" {
            continue; // 只读：不改计划状态
        }
        let Some(steps) = items else { continue };
        if action == "set" {
            plan = steps;
            continue;
        }
        // update：按 id 增量合并；没有 id 的按标题匹配。
        for next in steps {
            let index = plan
                .iter()
                .position(|current| {
                    if !next.id.is_empty() {
                        current.id == next.id
                    } else {
                        current.title == next.title
                    }
                });
            match index {
                Some(index) => {
                    if !next.id.is_empty() {
                        plan[index].id = next.id.clone();
                    }
                    plan[index].title = next.title.clone();
                    plan[index].status = next.status;
                }
                None => plan.push(next),
            }
        }
    }

    if !seen || plan.is_empty() {
        return None;
    }
    Some(TodoPlan { steps: plan, notes })
}

fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// 步骤列表（工具行展开区里也用它）。
#[derive(Debug, Clone, Default)]
pub struct TodoList {
    pub steps: Vec<TodoStep>,
    pub notes: Option<String>,
}

impl Render for TodoList {
    fn render(&self) -> String {
        if self.steps.is_empty() {
            return String::new();
        }
        let notes = self
            .notes
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                format!(
                    "<p class=\"ada-todo__notes\"><span>备注：</span>{}</p>",
                    escape(text)
                )
            })
            .unwrap_or_default();
        let items = self
            .steps
            .iter()
            .map(|step| {
                let glyph = match step.status {
                    TodoStatus::Completed => icon("i-check-circle", 13),
                    TodoStatus::InProgress => icon("i-arrow-right", 13),
                    TodoStatus::Pending => icon("i-circle", 13),
                };
                format!(
                    "<li class=\"ada-todo__step is-{status}\">{glyph}<span>{title}</span></li>",
                    status = step.status.class(),
                    glyph = glyph,
                    title = escape(&step.title),
                )
            })
            .collect::<Vec<_>>()
            .join("");
        format!(
            "<div class=\"ada-todo__list-wrap\">{notes}<ol class=\"ada-todo__list\">{items}</ol></div>"
        )
    }
}

/// 悬浮任务规划面板：默认收起的药丸 + 展开的卡片。
#[derive(Debug, Clone, Default)]
pub struct TodoPanel {
    pub plan: TodoPlan,
    /// DOM id（同一页面只允许一个面板）。
    pub dom_id: String,
}
impl Render for TodoPanel {
    fn render(&self) -> String {
        let plan = &self.plan;
        let total = plan.steps.len();
        let done = plan.completed();
        let all_done = plan.all_done();
        let active = plan
            .active_step()
            .map(|step| step.title.clone())
            .unwrap_or_default();
        let state_class = if all_done { "is-done" } else { "is-active" };
        // 收起时只留"当前在做什么"；全部完成则不重复提示。
        let active_html = if all_done || active.is_empty() {
            String::new()
        } else {
            format!(
                "<span class=\"ada-todo__active\">{}</span>",
                escape(&active)
            )
        };
        // 展开态的头部：进度 + 复制 + 收起提示。
        let head_right = format!(
            "<span class=\"ada-todo__head-right\"><button class=\"ada-todo__copy\" type=\"button\" data-copy=\"{markdown}\" data-copy-label=\"复制\">{copy}<span>复制</span></button><span class=\"ada-todo__collapse\">{up}收起</span></span>",
            markdown = escape(&plan.to_markdown()),
            copy = icon("i-copy", 11),
            up = icon("i-chevron-up", 12),
        );
        format!(
            r#"<details class="ada-todo {state}" data-state="collapsed" data-done="{done}" data-total="{total}">
  <summary class="ada-todo__summary" title="点击展开任务规划步骤">
    <span class="ada-todo__head-left">{icon}<span class="ada-todo__title ada-todo__title--collapsed">任务规划</span><span class="ada-todo__title ada-todo__title--expanded">任务规划步骤</span><span class="ada-todo__count ada-todo__count--collapsed">{done}/{total}</span><span class="ada-todo__count ada-todo__count--expanded">{done} / {total} 已完成</span>{active}</span>
    {head_right}
  </summary>
  <div class="ada-todo__body">{body}</div>
</details>"#,
            state = state_class,
            done = done,
            total = total,
            icon = icon("i-list", 13),
            head_right = head_right,
            active = active_html,
            body = TodoList {
                steps: plan.steps.clone(),
                notes: plan.notes.clone(),
            }
            .render(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn todo_item(action: &str, items: Value) -> Value {
        json!({
            "kind": "tool",
            "name": "todo",
            "rawArgs": json!({ "action": action, "items": items }).to_string(),
        })
    }

    #[test]
    fn set_建立基线() {
        let items = json!([
            { "id": "t1", "title": "读列表", "done": true },
            { "id": "t2", "title": "写条目", "done": false }
        ]);
        let plan = derive_todo_plan(&[todo_item("set", items)]).expect("应当有计划");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.completed(), 1);
        assert!(!plan.all_done());
    }

    #[test]
    fn update_按_id_增量合并() {
        let base = json!([
            { "id": "t1", "title": "读列表", "done": false },
            { "id": "t2", "title": "写条目", "done": false }
        ]);
        let update = json!([{ "id": "t1", "title": "读列表", "done": true }]);
        let plan = derive_todo_plan(&[
            todo_item("set", base),
            todo_item("update", update),
        ])
        .expect("应当有计划");
        assert_eq!(plan.steps.len(), 2, "update 不该新增步骤");
        assert_eq!(plan.completed(), 1, "t1 应当被标记完成");
        assert_eq!(plan.steps[1].title, "写条目", "未提及的条目要保留");
    }

    #[test]
    fn update_没命中就追加() {
        let base = json!([{ "id": "t1", "title": "读列表", "done": false }]);
        let update = json!([{ "id": "t9", "title": "新任务", "done": false }]);
        let plan = derive_todo_plan(&[
            todo_item("set", base),
            todo_item("update", update),
        ])
        .expect("应当有计划");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[1].title, "新任务");
    }

    #[test]
    fn get_不改变计划() {
        let base = json!([{ "id": "t1", "title": "读列表", "done": false }]);
        let plan = derive_todo_plan(&[
            todo_item("set", base.clone()),
            todo_item("get", json!([])),
        ])
        .expect("应当有计划");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.completed(), 0);
    }

    #[test]
    fn 缺省_action_当作_set() {
        let mut item = json!({
            "kind": "tool",
            "name": "todo",
            "rawArgs": json!({ "items": [{ "id": "t1", "title": "读列表", "done": false }] }).to_string(),
        });
        // 先用 update 建立基线，再用缺省 action 整份替换
        let base = json!([{ "id": "t1", "title": "读列表", "done": false }]);
        let plan = derive_todo_plan(&[
            todo_item("update", base),
            item.take(),
        ])
        .expect("应当有计划");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn 没有_todo_就没有计划() {
        let items = vec![json!({ "kind": "assistant", "text": "你好" })];
        assert!(derive_todo_plan(&items).is_none());
    }

    #[test]
    fn 复制为_markdown_清单() {
        let items = json!([
            { "id": "t1", "title": "读列表", "done": true },
            { "id": "t2", "title": "写条目", "done": false }
        ]);
        let plan = derive_todo_plan(&[todo_item("set", items)]).expect("应当有计划");
        let markdown = plan.to_markdown();
        assert!(markdown.contains("- [x] 读列表"));
        assert!(markdown.contains("- [ ] 写条目"));
        assert!(markdown.contains("(1/2)"));
    }
}
