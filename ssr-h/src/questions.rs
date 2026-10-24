//! 提问（`ask_user`）的归一化、提取与作答表单解析。
//!
//! 三件事，各有一个入口，语义都对齐 desktop-ui 的 `question-data.ts`（不要在
//! 托盘/插件里再各自推演一遍）：
//!
//! 1. [`normalize_question`]：把 `details.question` 摊成统一的 [`PendingQuestion`]。
//!    引擎发的是 snake_case（`allow_text`/`multi_select`），线协议是 camelCase，
//!    单题简写与多题（`questions[]`）两种写法都收；
//! 2. [`pending_questions_from_items`]：从当前会话 items 里取待答提问
//!    （`ask_user` + `awaiting` 的工具卡，题目在 `details.question`）；
//! 3. [`answer_params_from_form`]：把作答表单解析成 `question.answer` 的参数。
//!    字段约定见 [docs/PLAN-question-panel.md](../docs/PLAN-question-panel.md) §4，
//!    核心托盘与 `thread.tray` 插件共用这一套。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// 归一化后的一个选项。`value` 是回传 `question.answer` 的口径：**选项 id 优先**
/// （引擎保底 `c1,c2…`），缺省回落 label——引擎拿 id 反查 label 渲染回给模型。
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// 归一化后的一道题。
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionItem {
    /// 题号（`q1`…；多题作答按它对应回题目）。
    pub id: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub allow_text: bool,
    pub multi_select: bool,
}

/// 当前会话一条待答的提问（`call_id` 全局唯一，作答路由按它寻址）。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuestion {
    pub call_id: String,
    /// 提问卡片的 `startedAt`（绝对 epoch ms），算"已等待 Ns"用；
    /// 0 = 不知道（不显示）。不拿排序序号 `at` 当时间戳。
    pub asked_at: u64,
    pub items: Vec<QuestionItem>,
}

impl PendingQuestion {
    /// 是否只有一个问题（渲染侧决定出单题表单还是多题分组表单）。
    pub fn is_single(&self) -> bool {
        self.items.len() <= 1
    }
}

fn text_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 归一化一个选项：字符串项就是 value=label；对象项按
/// `value = id || label`、`label = label || title || value`、`description` 直取。
fn normalize_option(raw: &Value) -> Option<QuestionOption> {
    if let Some(text) = raw.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return Some(QuestionOption {
            value: text.to_string(),
            label: text.to_string(),
            description: None,
        });
    }
    if !raw.is_object() {
        return None;
    }
    let id = text_at(raw, "id");
    let label = text_at(raw, "label")
        .or_else(|| text_at(raw, "title"))
        .or_else(|| text_at(raw, "value"))
        .or_else(|| id.clone());
    let value = text_at(raw, "value")
        .or(id)
        .or_else(|| text_at(raw, "label"));
    let value = value?;
    let label = label.unwrap_or_else(|| value.clone());
    Some(QuestionOption {
        value,
        label,
        description: text_at(raw, "description"),
    })
}

fn normalize_options(raw: Option<&Value>) -> Vec<QuestionOption> {
    let list = raw
        .and_then(|value| {
            value
                .get("options")
                .or_else(|| value.get("choices"))
                .and_then(Value::as_array)
        })
        .cloned()
        .unwrap_or_default();
    list.iter().filter_map(normalize_option).collect()
}

/// 布尔字段：`allowText`/`allow_text` 两种拼写都收（缺省见各调用点）。
fn bool_at(raw: &Value, camel: &str, snake: &str) -> Option<bool> {
    raw.get(camel)
        .and_then(Value::as_bool)
        .or_else(|| raw.get(snake).and_then(Value::as_bool))
}

/// 把一条提问（单题或多题）归一化成 [`PendingQuestion`]。
///
/// 归一化之后 `items` **恒非空**：单题也会被包成一项，渲染侧只认一种结构。
/// `asked_at` 事件本身不带，由提取方补——用工具卡的 `startedAt`（绝对毫秒），
/// **不能**用 `at`（那是卡片排序序号，不是时间戳）。
pub fn normalize_question(question_val: &Value, call_id: &str) -> Option<PendingQuestion> {
    if !question_val.is_object() {
        return None;
    }
    let raw_list = match question_val
        .get("questions")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
    {
        Some(list) => list.clone(),
        // 单题简写：整份 payload 就是那一题
        None => vec![question_val.clone()],
    };
    let items = raw_list
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let fallback = format!("q{}", index + 1);
            QuestionItem {
                id: text_at(raw, "id").unwrap_or(fallback),
                // 引擎保证题干非空；兜底文案只防手滑的 payload，不给空卡片
                question: text_at(raw, "question").unwrap_or_else(|| "请回复智能体的提问".to_string()),
                options: normalize_options(Some(raw)),
                allow_text: bool_at(raw, "allowText", "allow_text").unwrap_or(true),
                multi_select: bool_at(raw, "multiSelect", "multi_select").unwrap_or(false),
            }
        })
        .collect();
    Some(PendingQuestion {
        call_id: call_id.to_string(),
        asked_at: 0,
        items,
    })
}

/// 从当前会话 items 里提取待答提问（时间正序）。
///
/// 逆序扫 `ask_user` + `awaiting` 的工具卡（`details.question` 优先，回落 `args`）；
/// 遇到**非排队**的 user 消息就停——提问已翻篇（desktop-ui 同款判据），但排队中的
/// 消息不算翻篇（它还在等当前回答结束）。
pub fn pending_questions_from_items(raw_items: &[Value]) -> Vec<PendingQuestion> {
    let mut found = Vec::new();
    for item in raw_items.iter().rev() {
        let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "user" && item.get("queued").and_then(Value::as_bool) != Some(true) {
            break;
        }
        if kind != "tool"
            || item.get("name").and_then(Value::as_str) != Some("ask_user")
            || item.get("status").and_then(Value::as_str) != Some("awaiting")
        {
            continue;
        }
        let call_id = item
            .get("callId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if call_id.is_empty() {
            continue;
        }
        let question_val = item
            .get("details")
            .and_then(|details| details.get("question"))
            .filter(|value| value.is_object())
            .or_else(|| item.get("args").filter(|value| value.is_object()));
        let Some(question_val) = question_val else {
            continue;
        };
        let Some(mut question) = normalize_question(question_val, &call_id) else {
            continue;
        };
        // 等待时长只认 `startedAt`（绝对毫秒）：`at` 是排序序号，拿它当时间戳
        // 会和行内 `elapsed_label` 得出两套互相矛盾的数字。
        question.asked_at = item
            .get("startedAt")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        found.push(question);
    }
    found.reverse();
    found
}

/// 一道题从表单里收集到的作答。
#[derive(Debug, Default)]
struct SubAnswer {
    choice: Option<String>,
    choices: Vec<String>,
    text: Option<String>,
}

impl SubAnswer {
    fn is_empty(&self) -> bool {
        self.choice.is_none() && self.choices.is_empty() && self.text.is_none()
    }
}

fn trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 把作答表单解析成 `question.answer` 的参数（不含 `callId`，路由从 path 里拿）。
///
/// 字段约定（核心托盘与 `thread.tray` 插件共用）：
///
/// - 单题：`choice`（radio）/ `choices`（checkbox 同名多值）/ `text`；
/// - 多题：每题按题号前缀 `q_{id}__choice` / `q_{id}__choices` / `q_{id}__text`；
///   出现任意 `q_` 前缀字段即走多题路径。
///
/// 全空提交报错（不发 RPC）；多题里没作答的题直接跳过（引擎按题号对应）。
pub fn answer_params_from_form(form: &[(String, String)]) -> Result<Value, String> {
    let mut single = SubAnswer::default();
    let mut order: Vec<String> = Vec::new();
    let mut multi: BTreeMap<String, SubAnswer> = BTreeMap::new();

    for (key, value) in form {
        if key == "thread_id" {
            continue;
        }
        if let Some(rest) = key.strip_prefix("q_") {
            let Some((id, field)) = rest.rsplit_once("__") else {
                continue;
            };
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            let entry = multi.entry(id.to_string()).or_insert_with(|| {
                order.push(id.to_string());
                SubAnswer::default()
            });
            match field {
                "choice" => entry.choice = trimmed(value).or(entry.choice.take()),
                "choices" => {
                    if let Some(text) = trimmed(value) {
                        entry.choices.push(text);
                    }
                }
                "text" => entry.text = trimmed(value).or(entry.text.take()),
                _ => {}
            }
            continue;
        }
        match key.as_str() {
            "choice" => single.choice = trimmed(value).or(single.choice.take()),
            "choices" => {
                if let Some(text) = trimmed(value) {
                    single.choices.push(text);
                }
            }
            "text" => single.text = trimmed(value).or(single.text.take()),
            _ => {}
        }
    }

    if !multi.is_empty() {
        let answers: Vec<Value> = order
            .iter()
            .filter(|id| !multi[*id].is_empty())
            .map(|id| {
                let answer = &multi[id];
                let mut entry = Map::new();
                entry.insert("id".to_string(), json!(id));
                if let Some(choice) = &answer.choice {
                    entry.insert("choice".to_string(), json!(choice));
                }
                if !answer.choices.is_empty() {
                    entry.insert("choices".to_string(), json!(answer.choices));
                }
                if let Some(text) = &answer.text {
                    entry.insert("text".to_string(), json!(text));
                }
                json!(entry)
            })
            .collect();
        if answers.is_empty() {
            return Err("请先选择或填写回答".to_string());
        }
        return Ok(json!({ "answers": answers }));
    }

    if single.is_empty() {
        return Err("请先选择或填写回答".to_string());
    }
    let mut params = Map::new();
    if let Some(choice) = single.choice {
        params.insert("choice".to_string(), json!(choice));
    }
    if !single.choices.is_empty() {
        params.insert("choices".to_string(), json!(single.choices));
    }
    if let Some(text) = single.text {
        params.insert("text".to_string(), json!(text));
    }
    Ok(json!(params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_single_question_snake_case() {
        let raw = json!({
            "question": "选哪个？",
            "choices": [
                {"id": "c1", "label": "方案一"},
                {"id": "c2", "label": "方案二", "description": "稳妥"}
            ],
            "allow_text": true,
            "multi_select": false,
        });
        let question = normalize_question(&raw, "call_q1").expect("question");
        assert!(question.is_single());
        assert_eq!(question.call_id, "call_q1");
        assert_eq!(question.items[0].id, "q1");
        assert_eq!(question.items[0].options.len(), 2);
        assert_eq!(question.items[0].options[0].value, "c1");
        assert_eq!(question.items[0].options[0].label, "方案一");
        assert_eq!(
            question.items[0].options[1].description.as_deref(),
            Some("稳妥")
        );
        assert!(question.items[0].allow_text);
        assert!(!question.items[0].multi_select);
    }

    #[test]
    fn normalizes_multi_question_and_fills_first() {
        let raw = json!({
            "question": "首题",
            "choices": [],
            "allow_text": true,
            "multi_select": false,
            "questions": [
                {"id": "env", "question": "用哪个环境？", "choices": ["dev", "prod"]},
                {"question": "备注？", "allow_text": true, "multi_select": false}
            ],
        });
        let question = normalize_question(&raw, "call_q2").expect("question");
        assert_eq!(question.items.len(), 2);
        assert_eq!(question.items[0].id, "env");
        // 字符串选项：value = label = 原文
        assert_eq!(question.items[0].options[1].value, "prod");
        assert!(question.items[1].allow_text);
    }

    #[test]
    fn camel_case_aliases_are_accepted() {
        let raw = json!({
            "question": "选哪些？",
            "choices": [{"value": "a"}, {"value": "b"}],
            "allowText": true,
            "multiSelect": true,
        });
        let question = normalize_question(&raw, "call_q3").expect("question");
        assert!(question.items[0].multi_select);
        // 没有 id 的对象选项：value 回落 value/label 字段
        assert_eq!(question.items[0].options[0].value, "a");
    }

    #[test]
    fn non_object_payload_is_rejected() {
        assert!(normalize_question(&json!("选哪个？"), "call_x").is_none());
        assert!(normalize_question(&Value::Null, "call_x").is_none());
    }

    fn tool_item(call_id: &str, question: Value, status: &str, at: u64) -> Value {
        json!({
            "kind": "tool", "id": format!("item_{call_id}"), "at": at,
            // 等待时长按 startedAt（绝对毫秒）算；at 是排序序号
            "startedAt": at,
            "callId": call_id, "name": "ask_user", "args": {},
            "rawArgs": "{}", "status": status,
            "details": {"question": question},
        })
    }

    #[test]
    fn extracts_awaiting_ask_user_cards_in_order() {
        let raw = json!({
            "question": "选哪个？",
            "choices": [{"id": "c1", "label": "A"}],
        });
        let items = vec![
            json!({"kind": "user", "id": "u1", "at": 1, "text": "开始"}),
            tool_item("call_q1", raw.clone(), "awaiting", 2),
        ];
        let pending = pending_questions_from_items(&items);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].call_id, "call_q1");
        assert_eq!(pending[0].asked_at, 2);
        assert_eq!(pending[0].items[0].options[0].value, "c1");
    }

    #[test]
    fn stops_at_newest_user_message_but_not_queued_ones() {
        let raw = json!({"question": "选哪个？", "choices": []});
        // 排队消息在提问之后出现（引擎阻塞在提问上时用户继续发消息）：
        // 逆序扫描要穿过排队消息，找到还挂着的提问。
        let queued_scenario = vec![
            json!({"kind": "user", "id": "u1", "at": 1, "text": "开始"}),
            tool_item("call_q1", raw.clone(), "awaiting", 2),
            json!({"kind": "user", "id": "u2", "at": 3, "text": "排队消息", "queued": true}),
        ];
        let pending = pending_questions_from_items(&queued_scenario);
        let ids: Vec<&str> = pending.iter().map(|q| q.call_id.as_str()).collect();
        assert_eq!(ids, vec!["call_q1"]);
        // 真实 user 消息在提问之后：提问已翻篇，不再渲染。
        let stale_scenario = vec![
            tool_item("call_q1", raw.clone(), "awaiting", 2),
            json!({"kind": "user", "id": "u2", "at": 3, "text": "真实消息"}),
        ];
        assert!(pending_questions_from_items(&stale_scenario).is_empty());
    }

    #[test]
    fn ignores_done_and_non_question_tools() {
        let raw = json!({"question": "选哪个？", "choices": []});
        let items = vec![
            tool_item("call_q1", raw.clone(), "done", 2),
            json!({
                "kind": "tool", "id": "t2", "at": 3, "callId": "call_r1",
                "name": "read_file", "args": {}, "rawArgs": "{}", "status": "awaiting",
                "details": {"question": raw},
            }),
        ];
        assert!(pending_questions_from_items(&items).is_empty());
    }

    #[test]
    fn single_form_builds_choice_and_text_params() {
        let form = vec![
            ("thread_id".to_string(), "t1".to_string()),
            ("choice".to_string(), "c2".to_string()),
            ("text".to_string(), "  补充说明  ".to_string()),
        ];
        let params = answer_params_from_form(&form).expect("params");
        assert_eq!(params["choice"], json!("c2"));
        assert_eq!(params["text"], json!("补充说明"));
        assert!(params.get("answers").is_none());
    }

    #[test]
    fn multi_select_form_collects_repeated_choices() {
        let form = vec![
            ("choices".to_string(), "c1".to_string()),
            ("choices".to_string(), "c3".to_string()),
        ];
        let params = answer_params_from_form(&form).expect("params");
        assert_eq!(params["choices"], json!(["c1", "c3"]));
    }

    #[test]
    fn multi_question_form_groups_by_question_id() {
        let form = vec![
            ("thread_id".to_string(), "t1".to_string()),
            ("q_env__choice".to_string(), "c1".to_string()),
            ("q_note__text".to_string(), "无".to_string()),
            // 全空的题：跳过
            ("q_skip__choice".to_string(), " ".to_string()),
        ];
        let params = answer_params_from_form(&form).expect("params");
        let answers = params["answers"].as_array().expect("answers");
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0]["id"], json!("env"));
        assert_eq!(answers[0]["choice"], json!("c1"));
        assert_eq!(answers[1]["id"], json!("note"));
        assert_eq!(answers[1]["text"], json!("无"));
    }

    #[test]
    fn empty_form_is_an_error_without_rpc() {
        let form = vec![("thread_id".to_string(), "t1".to_string())];
        assert!(answer_params_from_form(&form).is_err());
        let empty_multi = vec![("q_env__choice".to_string(), "  ".to_string())];
        assert!(answer_params_from_form(&empty_multi).is_err());
    }
}
