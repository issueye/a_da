use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agent_base::domain::{
    AgentEvent, AgentEventBody, ToolCall, ToolDescriptor, ToolReceipt, ToolStatus,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 单题的答复（多题路径按 `id` 对应回题目）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionSubAnswer {
    /// 题号（与提问里的题号对应）。缺省时按**顺序**对应题目。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswerPayload {
    pub choice: Option<String>,
    /// 多选答复：用户勾选的选项 id 列表（单选路径留空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    pub text: Option<String>,
    pub answered_by: String,
    /// 一次问多题时的**逐题答复**。单题路径留空，仍走上面的 `choice`/`choices`/`text`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<QuestionSubAnswer>>,
}

/// 一次提问里准备就绪的**一道题**（参数校验 + 选项 id 补齐之后）。
struct PreparedQuestion {
    /// 题号（`q1`、`q2`…；`questions[i].id` 给了就用给的）
    id: String,
    question: String,
    /// 已补齐 `c1, c2…` 的选项数组
    choices: serde_json::Value,
    allow_text: bool,
    multi_select: bool,
}

/// 一次提问最多几道题。再多就不是"同批确认"，而是把问卷丢给用户。
const MAX_QUESTIONS: usize = 10;
/// 单题文本上限（与单题路径保持一致）。
const MAX_QUESTION_CHARS: usize = 600;

/// 把 `ask_user` 的参数规整成题目列表：单题简写（顶层 `question`/`choices`）与
/// 多题（`questions: [...]`）两种写法都收，后者优先。
fn prepare_questions(args: &serde_json::Value) -> Result<Vec<PreparedQuestion>, String> {
    let raw_list: Vec<serde_json::Value> =
        match args.get("questions").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr.clone(),
            // 单题简写：整份参数就是那一题（`question`/`choices`/… 都在顶层）
            _ => vec![args.clone()],
        };

    if raw_list.len() > MAX_QUESTIONS {
        return Err(format!(
            "一次最多问 {MAX_QUESTIONS} 个问题（收到 {} 个）。请挑最要紧的先问。",
            raw_list.len()
        ));
    }

    let mut out = Vec::with_capacity(raw_list.len());
    for (idx, raw) in raw_list.iter().enumerate() {
        let text = raw
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let ordinal = idx + 1;

        if text.is_empty() {
            return Err(if raw_list.len() > 1 {
                format!(
                    "第 {ordinal} 个问题缺少 question 文本（`questions` 里每一项都要有 `question`）"
                )
            } else {
                "缺少 question 参数：要问用户什么？".to_string()
            });
        }
        if text.chars().count() > MAX_QUESTION_CHARS {
            return Err(format!(
                "第 {ordinal} 个问题太长（上限 {MAX_QUESTION_CHARS} 字）。请把背景压缩成几句后再问。"
            ));
        }

        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("q{ordinal}"));

        out.push(PreparedQuestion {
            id,
            question: text.to_string(),
            choices: normalize_choices(raw.get("choices").cloned().unwrap_or(json!([]))),
            allow_text: raw.get("allow_text").and_then(|v| v.as_bool()).unwrap_or(true),
            multi_select: raw
                .get("multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }

    Ok(out)
}

/// 把**一道题**的答复渲染成一句话。
///
/// 单选/多选/纯文本三条路都要能落到同一句里：模型该看到"选择 [方案 A, 方案 C]"，
/// 而不是一串它自己编的 id（它得靠这个继续推理）。
fn render_answer(
    choices: &serde_json::Value,
    choice: Option<&str>,
    chosen: Option<&[String]>,
    text: Option<&str>,
) -> String {
    let label_of = |id: &str| -> String {
        choices
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))
            })
            .and_then(|c| c.get("label").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .unwrap_or_else(|| id.to_string())
    };

    let picked: Option<String> = match chosen {
        Some(list) if !list.is_empty() => {
            Some(list.iter().map(|id| label_of(id)).collect::<Vec<_>>().join("、"))
        }
        _ => choice.map(|c| label_of(c)),
    };

    match (picked, text) {
        (Some(c), Some(t)) => format!("选择 [{c}]，说明：{t}"),
        (Some(c), None) => format!("选择 [{c}]"),
        (None, Some(t)) => t.to_string(),
        (None, None) => "未作答".to_string(),
    }
}

pub type QuestionWaiterFuture = Pin<Box<dyn Future<Output = Result<QuestionAnswerPayload, String>> + Send>>;
pub type QuestionRegistrarFn = Arc<dyn Fn(&str) -> QuestionWaiterFuture + Send + Sync>;

static QUESTION_REGISTRAR: std::sync::RwLock<Option<QuestionRegistrarFn>> =
    std::sync::RwLock::new(None);

/// 注册全局提问等待器工厂（由拥有会话与问答管理器的上层节点注入）。
pub fn set_question_registrar(registrar: QuestionRegistrarFn) {
    if let Ok(mut lock) = QUESTION_REGISTRAR.write() {
        *lock = Some(registrar);
    }
}

pub fn clear_question_registrar() {
    if let Ok(mut lock) = QUESTION_REGISTRAR.write() {
        *lock = None;
    }
}

pub fn get_question_registrar() -> Option<QuestionRegistrarFn> {
    QUESTION_REGISTRAR.read().ok().and_then(|guard| guard.clone())
}

/// 给缺失 `id` 的选项补 `c1, c2…`。
///
/// schema 里承诺过"如不填自动生成 c1, c2 等"，但实现从没做过：界面按 **id** 勾选与回传，
/// 缺 id 的选项点了也没反应（value 是 undefined）。这里补齐，让承诺成真。
fn normalize_choices(raw: serde_json::Value) -> serde_json::Value {
    let Some(arr) = raw.as_array() else {
        return raw;
    };
    let mut out = Vec::with_capacity(arr.len());
    for (idx, c) in arr.iter().enumerate() {
        let mut obj = c.as_object().cloned().unwrap_or_default();
        let has_id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !has_id {
            obj.insert("id".to_string(), json!(format!("c{}", idx + 1)));
        }
        out.push(serde_json::Value::Object(obj));
    }
    json!(out)
}

/// `ask_user` 工具：在交互界面向用户发起选择（单选/多选）或补充说明提问并阻塞等待答复。
pub struct AskUserTool {
    descriptor: ToolDescriptor,
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AskUserTool {
    pub fn new() -> Self {
        let descriptor = crate::registry::find_tool_descriptor("ask_user")
            .expect("ask_user descriptor must be registered in agent-toolkit registry")
            .clone();

        Self { descriptor }
    }
}

impl Tool for AskUserTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    /// 合法地长时间运行 / 等待外部输入：不受引擎策略超时约束（见 registry::LONG_RUNNING_TOOLS）。
    fn long_running(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            // 1. 参数校验与对齐：单题简写（顶层 question/choices）与多题（questions[]）都收
            let questions = match prepare_questions(&call.args) {
                Ok(list) => list,
                Err(msg) => return ToolReceipt::error(msg, started_at, started_at),
            };

            // 2. 取消检查
            if ctx.cancel.is_cancelled() {
                let finished_at = started_at;
                return ToolReceipt::new(
                    ToolStatus::Aborted,
                    "操作在发起提问前已中止。",
                    started_at,
                    finished_at,
                );
            }

            // 3. 通道检查与提问挂起
            let Some(reg) = get_question_registrar() else {
                let finished_at = started_at;
                return ToolReceipt::error(
                    "当前环境未接入交互提问协调器（无事件问答通道），无法向用户提问。请直接基于现有上下文作出合理推断并继续。",
                    started_at,
                    finished_at,
                );
            };

            // 注册等待通道
            let waiter = reg(&call.id);

            // 发射提问事件（通知界面/上层协调者）
            //
            // 顶层 `question`/`choices`/`multi_select` 在多题时填**首题**：老的渲染路径
            // 不认识 `questions`，至少还能显示第一题，而不是一片空白。
            let top = &questions[0];
            let mut question_payload = json!({
                "question": top.question,
                "choices": top.choices,
                "allow_text": top.allow_text,
                "multi_select": top.multi_select,
            });
            // `questions` 只在**真的多题**时带上：单题的线上格式与以前逐字节一致，零回归。
            if questions.len() > 1 {
                question_payload["questions"] = json!(questions
                    .iter()
                    .map(|q| json!({
                        "id": q.id,
                        "question": q.question,
                        "choices": q.choices,
                        "allow_text": q.allow_text,
                        "multi_select": q.multi_select,
                    }))
                    .collect::<Vec<_>>());
            }

            ctx.events.emit(AgentEvent::new(
                0,
                0,
                ctx.thread_id,
                AgentEventBody::QuestionAsked {
                    call_id: call.id.clone(),
                    question: question_payload,
                },
            ));

            // 等待作答或取消
            let answer = tokio::select! {
                ans = waiter => ans,
                _ = async {
                    while !ctx.cancel.is_cancelled() {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } => {
                    let finished_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    return ToolReceipt::new(
                        ToolStatus::Aborted,
                        "操作在等待提问答复时被中止。",
                        started_at,
                        finished_at,
                    );
                }
            };

            let finished_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            match answer {
                Ok(ans) => {
                    if ans.answered_by == "aborted" {
                        return ToolReceipt::new(
                            ToolStatus::Aborted,
                            "提问已被取消。",
                            started_at,
                            finished_at,
                        );
                    }
                    // 多题（`answers` 有内容）→ 逐题一行；否则走单题的老路径。
                    // 单题那段的措辞保持不变：它是既有测试与给人看的稳定输出。
                    let body = match ans.answers.as_deref().filter(|list| !list.is_empty()) {
                        Some(list) => {
                            let mut lines = Vec::with_capacity(questions.len());
                            for (idx, q) in questions.iter().enumerate() {
                                // 先按题号对，对不上就按**顺序**对（客户端可能没回题号）
                                let hit = list
                                    .iter()
                                    .find(|a| a.id.as_deref() == Some(q.id.as_str()))
                                    .or_else(|| list.get(idx));
                                let rendered = match hit {
                                    Some(a) => render_answer(
                                        &q.choices,
                                        a.choice.as_deref(),
                                        a.choices.as_deref(),
                                        a.text.as_deref(),
                                    ),
                                    None => "未作答".to_string(),
                                };
                                lines.push(format!("{}. {} → {}", idx + 1, q.question, rendered));
                            }
                            format!("\n{}", lines.join("\n"))
                        }
                        None => {
                            // 单题：选项 id 还原成 label，模型该看到"选择 [方案 A, 方案 C]"，
                            // 而不是一串它自己编的 id（它得靠这个继续推理）
                            let rendered = render_answer(
                                &top.choices,
                                ans.choice.as_deref(),
                                ans.choices.as_deref(),
                                ans.text.as_deref(),
                            );
                            // 老路径里"什么都没答"（如 approval 兜底按批准放行）历史上报"已确认"
                            let rendered = if rendered == "未作答" {
                                "已确认".to_string()
                            } else {
                                rendered
                            };
                            format!("：{rendered}")
                        }
                    };
                    ToolReceipt::success(
                        format!("答复已记录（来源: {}）{body}", ans.answered_by),
                        started_at,
                        finished_at,
                    )
                }
                Err(err) => ToolReceipt::error(format!("提问失败：{err}"), started_at, finished_at),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::DenialKind;
    use agent_base::testing::{ManualCancel, RecordingSink};
    use agent_base::ports::Scope;
    use std::path::PathBuf;

    struct DummyScope;
    impl Scope for DummyScope {
        fn id(&self) -> &str {
            "dummy"
        }
        fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
            Ok(PathBuf::from(raw))
        }
    }

    #[tokio::test]
    async fn test_ask_user_schema_and_parameter_alignment() {
        let tool = AskUserTool::new();
        let desc = tool.descriptor();
        assert_eq!(desc.name, "ask_user");
        assert!(desc.is_readonly());

        // 验证参数 Schema 对齐
        let schema = &desc.schema;
        assert_eq!(schema["type"], "object");
        let props = &schema["properties"];
        assert!(props.get("question").is_some());
        assert!(props.get("choices").is_some());
        assert!(props.get("allow_text").is_some());
        assert!(props.get("multi_select").is_some(), "多选必须能在 schema 里声明");
        assert!(props.get("questions").is_some(), "多题数组必须能在 schema 里声明");
        // 单题（question）与多题（questions）两种写法都要被认可，但至少得给一个
        assert_eq!(
            schema["anyOf"],
            json!([{ "required": ["question"] }, { "required": ["questions"] }])
        );
    }

    /// 守门：`ask_user` 一次问多题——逐题作答必须**逐题带回**，一题都不许丢。
    #[tokio::test]
    async fn test_ask_user_multi_questions_returns_every_answer() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t_multi_q",
        };

        set_question_registrar(Arc::new(|_call_id: &str| {
            Box::pin(async {
                Ok(QuestionAnswerPayload {
                    choice: None,
                    choices: None,
                    text: None,
                    answered_by: "user".into(),
                    answers: Some(vec![
                        // 第一题按题号回，第二题只按顺序回
                        QuestionSubAnswer {
                            id: Some("q1".into()),
                            choice: Some("c2".into()),
                            choices: None,
                            text: None,
                        },
                        QuestionSubAnswer {
                            id: None,
                            choice: None,
                            choices: None,
                            text: Some("先按现有方案走".into()),
                        },
                    ]),
                })
            })
        }));

        let call = ToolCall {
            id: "call_multi_q".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "questions": [
                    {
                        "id": "q1",
                        "question": "发布节奏怎么定？",
                        "choices": [
                            {"id": "c1", "label": "每周一发"},
                            {"id": "c2", "label": "双周发"}
                        ]
                    },
                    { "question": "还有什么补充？" }
                ]
            }),
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success, "{}", receipt.output);
        // 两题都要出现在回执里，且选项要还原成 label
        assert!(receipt.output.contains("发布节奏怎么定？"), "{}", receipt.output);
        assert!(receipt.output.contains("双周发"), "{}", receipt.output);
        assert!(receipt.output.contains("还有什么补充？"), "{}", receipt.output);
        assert!(receipt.output.contains("先按现有方案走"), "{}", receipt.output);
        assert!(!receipt.output.contains("每周一发"), "没选的不得混进来：{}", receipt.output);

        // 事件里必须带 `questions` 数组，且题号缺省时自动补 `q2`
        let asked = events
            .snapshot()
            .into_iter()
            .find_map(|e| match e.body {
                AgentEventBody::QuestionAsked { question, .. } => Some(question),
                _ => None,
            })
            .expect("必须发出 QuestionAsked");
        let list = asked["questions"].as_array().expect("多题必须带 questions 数组");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], json!("q1"));
        assert_eq!(list[1]["id"], json!("q2"), "缺省题号要自动补");
        assert_eq!(list[0]["choices"][0]["id"], json!("c1"));

        clear_question_registrar();
    }

    /// 守门：多选答复必须**把选中的多个选项都带回模型**，且用 label 而不是 id。
    #[tokio::test]
    async fn test_ask_user_multi_select_returns_all_picked_labels() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t_multi",
        };

        set_question_registrar(Arc::new(|_call_id: &str| {
            Box::pin(async {
                Ok(QuestionAnswerPayload {
                    choice: None,
                    choices: Some(vec!["a1".into(), "a3".into()]),
                    text: Some("另外补一句".into()),
                    answers: None,
                    answered_by: "user".into(),
                })
            })
        }));

        let call = ToolCall {
            id: "call_multi_1".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "question": "请补充以下约束（可多选）",
                "multi_select": true,
                "choices": [
                    {"id": "a1", "label": "团队规模与资源"},
                    {"id": "a2", "label": "时间线与发布节奏"},
                    {"id": "a3", "label": "稳定性 vs 功能"}
                ]
            }),
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success, "{}", receipt.output);
        assert!(receipt.output.contains("团队规模与资源"), "{}", receipt.output);
        assert!(receipt.output.contains("稳定性 vs 功能"), "{}", receipt.output);
        assert!(!receipt.output.contains("时间线与发布节奏"), "没选的不得混进来：{}", receipt.output);
        assert!(receipt.output.contains("另外补一句"), "{}", receipt.output);

        // 事件里必须带上多选标记，界面才知道要渲染成勾选
        let recorded = events.snapshot();
        let asked = recorded
            .iter()
            .find_map(|e| match &e.body {
                AgentEventBody::QuestionAsked { question, .. } => Some(question.clone()),
                _ => None,
            })
            .expect("必须发出 QuestionAsked");
        assert_eq!(asked["multi_select"], json!(true), "提问事件要带 multi_select");

        clear_question_registrar();
    }

    #[tokio::test]
    async fn test_ask_user_without_channel_fails_instead_of_hanging() {
        clear_question_registrar();
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
        };

        let call = ToolCall {
            id: "call_1".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "question": "您希望使用哪种方案？",
                "choices": [{"label": "方案 A"}, {"label": "方案 B"}]
            }),
        };

        // 验证无通道时立即返回错误，绝不 hang 住
        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("当前环境未接入交互提问协调器"));
    }

    #[tokio::test]
    async fn test_ask_user_empty_or_too_long_question() {
        clear_question_registrar();
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t1",
        };

        let empty_call = ToolCall {
            id: "call_2".to_string(),
            name: "ask_user".to_string(),
            args: json!({ "question": "" }),
        };
        let receipt = tool.execute(&empty_call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("缺少 question 参数"));

        let long_question = "测".repeat(601);
        let long_call = ToolCall {
            id: "call_3".to_string(),
            name: "ask_user".to_string(),
            args: json!({ "question": long_question }),
        };
        let receipt = tool.execute(&long_call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Error);
        assert!(receipt.output.contains("上限 600 字"));
    }

    /// 守门：选项没给 `id` 时必须自动补 `c1, c2…`（schema 承诺过）。
    ///
    /// 界面按 **id** 勾选与回传，缺 id 的选项点了没反应（value 是 undefined）——
    /// 而 schema 一直写着"如不填自动生成 c1, c2 等"，实现却从没做过。
    #[tokio::test]
    async fn test_ask_user_generates_missing_choice_ids() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();
        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t_auto_id",
        };

        set_question_registrar(Arc::new(|_call_id: &str| {
            Box::pin(async {
                Ok(QuestionAnswerPayload {
                    choice: Some("c2".into()),
                    choices: None,
                    text: None,
                    answers: None,
                    answered_by: "user".into(),
                })
            })
        }));

        let call = ToolCall {
            id: "call_auto_id".to_string(),
            name: "ask_user".to_string(),
            // 故意不给 id
            args: json!({
                "question": "选一个",
                "choices": [{"label": "甲"}, {"label": "乙"}]
            }),
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success, "{}", receipt.output);
        assert!(receipt.output.contains("乙"), "id→label 还原要生效：{}", receipt.output);

        let asked = events
            .snapshot()
            .into_iter()
            .find_map(|e| match e.body {
                AgentEventBody::QuestionAsked { question, .. } => Some(question),
                _ => None,
            })
            .expect("必须发出 QuestionAsked");
        assert_eq!(asked["choices"][0]["id"], json!("c1"));
        assert_eq!(asked["choices"][1]["id"], json!("c2"));

        clear_question_registrar();
    }

    #[tokio::test]
    async fn test_ask_user_with_channel_emits_event_and_receives_answer() {
        let tool = AskUserTool::new();
        let scope = DummyScope;
        let cancel = ManualCancel::new();
        let events = RecordingSink::new();

        let ctx = ToolContext {
            scope: &scope,
            cancel: &cancel,
            events: &events,
            thread_id: "t_test_qa",
        };

        // 注册测试等待器
        set_question_registrar(Arc::new(|call_id: &str| {
            assert_eq!(call_id, "call_ask_1");
            Box::pin(async {
                Ok(QuestionAnswerPayload {
                    choice: Some("c1".into()),
                    choices: None,
                    text: Some("选用方案 A".into()),
                    answers: None,
                    answered_by: "main_agent".into(),
                })
            })
        }));

        let call = ToolCall {
            id: "call_ask_1".to_string(),
            name: "ask_user".to_string(),
            args: json!({
                "question": "应该选择方案 A 还是方案 B？",
                "choices": [{"id": "c1", "label": "方案 A"}, {"id": "c2", "label": "方案 B"}],
                "allow_text": true,
            }),
        };

        let receipt = tool.execute(&call, &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success);
        assert!(receipt.output.contains("方案 A"));
        assert!(receipt.output.contains("main_agent"));

        // 验证 QuestionAsked 事件确实被发射到了事件池中
        let recorded = events.snapshot();
        assert!(recorded.iter().any(|e| matches!(&e.body, AgentEventBody::QuestionAsked { call_id, .. } if call_id == "call_ask_1")));

        clear_question_registrar();
    }
}
