//! 对话行的视图模型：把 agent 的 `Item` 翻译成界面行（工具/思考/压缩/消息）。
//!
//! 全是纯函数，不碰 IO，也不依赖页面状态——方便单测，也能被 live 轮询复用。

use askama::Template;
use serde_json::Value;

use crate::templates::*;
use crate::ui::{self, Render, Tone};

use super::super::{array_at, running_ids, short_title, str_at, thread_row, truncate};
pub(super) fn tool_status_label(raw: &str) -> String {
    match raw {
        "waiting_approval" => "等待批准",
        "awaiting" => "待答复",
        "running" => "执行中",
        "done" | "completed" | "success" => "完成",
        "error" | "failed" => "失败",
        "timeout" => "已超时",
        "queued" => "排队中",
        "" => "",
        other => other,
    }
    .to_string()
}

/// 工具状态的语义色：warn（等待批准）/ brand（执行中、待答复）/ danger（失败）。
///
/// 完成态不给色——默认就是"完成"，到处挂绿标反而吵。
pub(super) fn tool_status_tone(raw: &str) -> String {
    match raw {
        "waiting_approval" => "warn",
        "awaiting" | "running" | "queued" => "brand",
        "error" | "failed" | "timeout" => "danger",
        _ => "",
    }
    .to_string()
}

/// 工具名 → 界面中文标签。
///
/// 与 desktop-ui 的 `TOOL_LABEL`（`desktop-ui/src/utils/tool-labels.ts`）保持同一套文案：
/// 工具名真源在 agent 的注册表里，这里只做"能查到就翻译，查不到就显示原名"。
pub(super) fn tool_label(name: &str) -> String {
    match name {
        "list_files" => "列出文件",
        "read_file" => "读取文件",
        "search_files" => "搜索代码",
        "write_file" => "写入文件",
        "edit_file" => "修改文件",
        "run_command" => "执行命令",
        "run_background" => "后台命令",
        "check_task" => "查看后台任务",
        "kill_task" => "停止后台任务",
        "todo" => "任务规划",
        "invoke_subagent" => "委派子智能体",
        "ask_user" => "向用户提问",
        "git" => "Git 操作",
        "code_outline" => "代码大纲",
        "run_tests" => "运行测试",
        "batch_write" => "批量写入",
        "batch_replace" => "批量替换",
        "project_inspect" => "项目诊断",
        "create_plugin" => "创建插件",
        "news_query" => "查询资讯",
        "weather" => "查询天气",
        "finish" => "结束回合",
        "" => "工具",
        other => other,
    }
    .to_string()
}

/// 从调用参数里抽出"这个工具在动什么"的主文本。
pub(super) fn args_text(args: &Value) -> String {
    if let Some(text) = args.as_str() {
        return text.replace(['\r', '\n'], " ").trim().to_string();
    }
    for key in [
        "path", "file", "filePath", "command", "cmd", "task", "query", "keyword", "pattern", "url",
        "question", "message", "id",
    ] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return value.replace(['\r', '\n'], " ").trim().to_string();
            }
        }
    }
    String::new()
}

/// 参数归一化：agent 有时把参数写成 JSON 字符串（`rawArgs`），这里统一成对象。
pub(super) fn normalize_args(value: Value) -> Value {
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() {
                return parsed;
            }
        }
    }
    value
}

/// 参数的可读化展示：对象按 JSON 美化，字符串尽量解析后美化。
pub(super) fn args_display(args: &Value) -> String {
    if args.is_null() {
        return String::new();
    }
    if let Some(text) = args.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() {
                return serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| text.to_string());
            }
        }
        return text.to_string();
    }
    serde_json::to_string_pretty(args).unwrap_or_default()
}

/// 工具结果的展示：结果条目列表优先铺成"标题 + 来源"，其余按 JSON 美化。
///
/// 资讯类工具的结果是几十条新闻的 JSON 数组，直接甩给用户读没有意义；
/// 展平成标题列表才看得懂"它到底查到了什么"。
pub(super) fn details_display(details: &Value) -> String {
    if let Some(items) = details.get("items").and_then(Value::as_array) {
        let mut lines: Vec<String> = Vec::new();
        for entry in items.iter().take(8) {
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if title.is_empty() {
                continue;
            }
            let source = entry
                .get("source")
                .or_else(|| entry.get("sourceLabel"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if source.is_empty() {
                lines.push(title.to_string());
            } else {
                lines.push(format!("{title}  —  {source}"));
            }
        }
        if !lines.is_empty() {
            if items.len() > lines.len() {
                lines.push(format!("（共 {} 条）", items.len()));
            }
            return lines.join("\n");
        }
    }
    serde_json::to_string_pretty(details).unwrap_or_default()
}

/// 路径类工具：把路径拆成"文件名 + 目录"，目录更淡、文件名更实。
pub(super) const PATH_TOOLS: [&str; 5] = [
    "read_file",
    "write_file",
    "edit_file",
    "list_files",
    "code_outline",
];

/// 工具目标对象（文件名 / 命令 / 查询词）与它的目录部分。
///
/// 对齐 desktop-ui `toolTarget` 的思路：`git` 用 action 翻译成人话，
/// 路径类工具拆成"文件名 + 目录"，其余直接取参数里的主字段。
pub(super) fn tool_target(name: &str, args: &Value) -> (String, String) {
    if name == "git" {
        let action = {
            let raw = str_at(args, "action").to_lowercase();
            if raw.is_empty() {
                "status".to_string()
            } else {
                raw
            }
        };
        let file = str_at(args, "file");
        let path = str_at(args, "path");
        let reference = str_at(args, "ref");
        let branch = str_at(args, "branch");
        let message = str_at(args, "message");
        let file = if file.is_empty() { path } else { file };
        let target = match action.as_str() {
            "status" => "查看仓库改动".to_string(),
            "diff" => {
                if file.is_empty() {
                    "查看未暂存差异".to_string()
                } else {
                    format!("查看 {file} 的改动")
                }
            }
            "log" => "近期提交".to_string(),
            "show" => format!("查看提交 {}", pick(reference, "HEAD")),
            "blame" => format!("追溯 {} 的逐行作者", pick(file, "文件")),
            "add" => "暂存改动".to_string(),
            "commit" => {
                if message.is_empty() {
                    "创建提交".to_string()
                } else {
                    format!("提交：{}", short_title(message, 40))
                }
            }
            "checkout" => format!("切换到 {}", pick(branch, "分支")),
            "restore" => format!("恢复 {}", pick(file, "文件")),
            "reset" => "重置到 HEAD".to_string(),
            "stash" => "stash 列表".to_string(),
            "branch" => "列出分支".to_string(),
            "merge" => format!("合并 {}", pick(branch, "分支")),
            "push" => "推送到远端".to_string(),
            "pull" => "拉取远端".to_string(),
            other => format!("git {other}"),
        };
        return (target, String::new());
    }
    match name {
        "run_tests" => return ("执行测试套件".to_string(), String::new()),
        "project_inspect" => return ("探测技术栈与项目环境".to_string(), String::new()),
        "create_plugin" => {
            let id = str_at(args, "id");
            return (
                if id.is_empty() {
                    "生成插件骨架".to_string()
                } else {
                    format!("插件 {id}")
                },
                String::new(),
            );
        }
        "batch_write" | "batch_replace" => {
            let count = array_at(args, "files").len();
            return (
                if name == "batch_write" {
                    format!("写入 {count} 个文件")
                } else {
                    format!("在 {count} 个文件中替换")
                },
                String::new(),
            );
        }
        "invoke_subagent" => {
            let role = {
                let raw = str_at(args, "subagent_id");
                if raw.is_empty() {
                    str_at(args, "role")
                } else {
                    raw
                }
            };
            let role = if role.is_empty() {
                "子智能体"
            } else {
                role
            };
            let task = short_title(str_at(args, "task"), 40);
            return (
                if task.is_empty() {
                    format!("[{role}]")
                } else {
                    format!("[{role}] {task}")
                },
                String::new(),
            );
        }
        _ => {}
    }

    let summary = args_text(args);
    if summary.is_empty() {
        return (String::new(), String::new());
    }
    if !PATH_TOOLS.contains(&name) {
        return (summary, String::new());
    }
    let normalized = summary.replace('\\', "/");
    match normalized.rfind('/') {
        Some(cut) => (
            normalized[cut + 1..].to_string(),
            normalized[..=cut].to_string(),
        ),
        None => (normalized, String::new()),
    }
}

pub(super) fn pick<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

/// 工具图标 key：与 desktop-ui `renderToolIcon` 的同一套语义。
pub(super) fn tool_icon(name: &str) -> String {
    match name {
        "list_files" => "folder",
        "read_file" | "write_file" | "edit_file" | "batch_write" | "code_outline" => "file",
        "batch_replace" => "edit",
        "search_files" | "project_inspect" => "search",
        "create_plugin" => "package",
        "find_symbol" => "code",
        "run_command" | "run_background" => "terminal",
        "run_tests" => "check",
        "check_task" => "clock",
        "kill_task" => "xcircle",
        "todo" => "list",
        "invoke_subagent" | "check_subagent" => "bot",
        "ask_user" => "sparkles",
        "git" => "git",
        _ => "terminal",
    }
    .to_string()
}

/// 思考文本的末行预览：折叠态单行显示"它在想什么"。
pub(super) fn last_line_preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() > 42 {
        let mut out: String = line.chars().take(42).collect();
        out.push('…');
        out
    } else {
        line.to_string()
    }
}

/// 持续时长文案。
///
/// - 工具：`durationMs` 是真实耗时，按 desktop-ui 的做法显示毫秒值；
/// - 思考：只有起止时间差 ≥1 秒才显示——恢复出来的历史会话里 `at` 与
///   `endedAt` 往往是同一个时间戳，显示"0 秒"只是噪声，不如不显示。
pub(super) fn elapsed_label(value: &Value) -> String {
    if let Some(ms) = value.get("durationMs").and_then(Value::as_u64) {
        return format!("{ms}ms");
    }
    let start = value
        .get("at")
        .and_then(Value::as_u64)
        .or_else(|| value.get("createdAt").and_then(Value::as_u64));
    let end = value
        .get("endedAt")
        .and_then(Value::as_u64)
        .or_else(|| value.get("ended_at").and_then(Value::as_u64));
    match (start, end) {
        (Some(start), Some(end)) if end.saturating_sub(start) >= 1000 => {
            let secs = (end - start) / 1000;
            if secs >= 60 {
                format!("{} 分 {} 秒", secs / 60, secs % 60)
            } else {
                format!("{secs} 秒")
            }
        }
        _ => String::new(),
    }
}

/// 压缩卡片的正文是 `summary`（不是 `text`），并带上真实的 token 变化。
pub(super) fn compact_text(value: &Value) -> String {
    let summary = str_at(value, "summary");
    let pre = value.get("preTokens").and_then(Value::as_u64);
    let post = value.get("postTokens").and_then(Value::as_u64);
    match (pre, post) {
        (Some(pre), Some(post)) => {
            format!("{summary}\n\n上下文 {pre} → {post} tokens")
        }
        _ => summary.to_string(),
    }
}

/// 把 agent 快照里的一条 item 归一化成组件库的行视图。
pub(super) fn row_view(value: &Value) -> ui::RowView {
    let kind = str_at(value, "kind").to_string();
    let tool_name = str_at(value, "name").to_string();
    let raw_text = if kind == "compact" {
        compact_text(value)
    } else {
        str_at(value, "text").to_string()
    };
    let images = value
        .get("images")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let key = str_at(value, "id").to_string();
    let raw_output = value
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            // 工具结果也可能落在 `details`（结构化对象）里。
            value
                .get("details")
                .map(details_display)
                .unwrap_or_default()
        });
    let text = truncate(&raw_text, 8000);
    let output = truncate(&raw_output, 4000);
    let html = if kind == "assistant" {
        crate::markdown::to_html(&raw_text)
    } else {
        String::new()
    };
    let label = match kind.as_str() {
        "user" => "你".to_string(),
        "assistant" => "助手".to_string(),
        "thinking" => "思考".to_string(),
        "tool" => tool_label(&tool_name),
        "notice" => "提示".to_string(),
        "compact" => "压缩".to_string(),
        other => other.to_string(),
    };
    let raw_status = str_at(value, "status");
    let args_value = value
        .get("args")
        .cloned()
        .or_else(|| value.get("rawArgs").cloned())
        .map(normalize_args)
        .unwrap_or(Value::Null);
    let (target, dir) = tool_target(&tool_name, &args_value);
    let preview = if kind == "thinking" {
        last_line_preview(&text)
    } else {
        String::new()
    };
    // 工具输出：JSON 走等宽原文，其它按 Markdown 渲染（链接自动成链、列表有排版）。
    let output_html = if looks_like_json(&output) {
        String::new()
    } else {
        crate::markdown::to_html(&output)
    };
    // todo 工具：把任务清单读出来，工具行展开时直接排版显示。
    let (todo_steps, todo_notes) = if tool_name == "todo" {
        ui::read_todo_items(value).unwrap_or_default()
    } else {
        (Vec::new(), None)
    };
    let status_tone = match tool_status_tone(raw_status).as_str() {
        "warn" => Tone::Warn,
        "brand" => Tone::Brand,
        "danger" => Tone::Danger,
        "success" => Tone::Success,
        _ => Tone::Neutral,
    };
    ui::RowView {
        kind,
        key,
        images,
        text,
        html,
        label,
        target,
        dir,
        icon: tool_icon(&tool_name),
        status: tool_status_label(raw_status),
        status_tone,
        elapsed: elapsed_label(value),
        args: truncate(&args_display(&args_value), 2000),
        output,
        output_html,
        preview,
        level: str_at(value, "level").to_string(),
        todo_steps,
        todo_notes,
    }
}

/// 渲染一行对话（组件库）。
pub(super) fn item_row(value: &Value, thread_id: &str) -> ItemRow {
    ItemRow {
        html: ui::render_row(&row_view(value), thread_id),
    }
}

/// 输出看起来是不是 JSON（对象或数组）——是就用等宽原文展示，不做 Markdown 解析。
pub(super) fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<Value>(trimmed).is_ok()
}

pub(super) fn model_of(snapshot: &Value, config: &Value) -> String {
    snapshot
        .get("config")
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            config
                .get("saved")
                .and_then(|saved| saved.get("model"))
                .and_then(Value::as_str)
        })
        .or_else(|| config.get("model").and_then(Value::as_str))
        .unwrap_or("未配置")
        .to_string()
}

/// 审批档位的真实字段是 `snapshot.config.approval`。
///
/// 历史实现找的是 `approvalMode`（不存在的字段），所以"完全访问"一直显示成
/// "自动批准"、危险档也不显红——这是安全信息，不能靠猜。
pub(super) fn approval_mode(snapshot: &Value, config: &Value) -> String {
    snapshot
        .get("config")
        .and_then(|cfg| cfg.get("approval"))
        .or_else(|| config.get("saved").and_then(|saved| saved.get("approval")))
        .or_else(|| config.get("approval"))
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string()
}

/// 审批档位的中文标签（与 desktop-ui `APPROVAL_OPTIONS` 同一套文案）。
pub(super) fn approval_label(mode: &str) -> String {
    match mode {
        "full" | "yolo" => "完全访问",
        "ask" | "manual" => "每次询问",
        "readonly" => "严格只读",
        "auto" | "" => "自动批准",
        other => other,
    }
    .to_string()
}

/// 审批档位的语义色：完全访问是危险档（红），每次询问是提醒档（琥珀）。
pub(super) fn approval_tone(mode: &str) -> String {
    match mode {
        "full" | "yolo" => "danger",
        "ask" | "manual" | "readonly" => "warn",
        _ => "neutral",
    }
    .to_string()
}

/// 工作区短名：路径最后一段（`E:/code/app` → `app`），供输入框展示。
pub(super) fn workspace_label(workspace: &str) -> String {
    let trimmed = workspace.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

pub(super) fn waited_label(value: &Value) -> String {
    let at = value.get("at").and_then(Value::as_u64).unwrap_or(0);
    if at == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(at);
    let seconds = now.saturating_sub(at) / 1000;
    if seconds < 60 {
        format!("已等待 {seconds}s")
    } else if seconds < 3600 {
        format!("已等待 {}m {:02}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        format!("已等待 {}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("已等待 {}d {}h", seconds / 86400, (seconds % 86400) / 3600)
    }
}

pub(super) fn approval_rows(raw_items: &[Value]) -> Vec<ApprovalRow> {
    let mut rows = Vec::new();
    for raw in raw_items.iter() {
        // 用原始 status 判定；渲染后的行里 status 已经中文化了。
        if str_at(raw, "kind") != "tool" || str_at(raw, "status") != "waiting_approval" {
            continue;
        }
        let call_id = str_at(raw, "callId").to_string();
        if call_id.is_empty() {
            continue;
        }
        let args = raw
            .get("rawArgs")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                raw.get("args")
                    .map(|args| serde_json::to_string_pretty(args).unwrap_or_default())
            })
            .unwrap_or_default();
        let raw_name = str_at(raw, "name");
        rows.push(ApprovalRow {
            call_id,
            tool_name: if raw_name.is_empty() {
                "工具".to_string()
            } else {
                tool_label(raw_name)
            },
            args: truncate(&args, 1200),
            waited: waited_label(raw),
        });
    }
    rows
}

pub(super) fn queue_rows(snapshot: &Value, thread_id: &str) -> Vec<QueueRow> {
    array_at(snapshot, "queue")
        .iter()
        .enumerate()
        .filter(|(_, item)| str_at(item, "threadId") == thread_id)
        .map(|(index, item)| QueueRow {
            index,
            order: index + 1,
            text: truncate(str_at(item, "text"), 240),
        })
        .collect()
}

/// 渲染好的对话片段（SSR 与 live 共用）。
pub(super) fn transcript_html(items: Vec<ItemRow>) -> String {
    render_string(&TranscriptPartial {
        items,
        transcript_empty: ui::render_empty_thread(),
    })
}

/// 侧栏会话列表（SSR 与 live 共用）。
pub(super) fn thread_rows(snapshot: &Value, active_thread_id: &str) -> Vec<ThreadRow> {
    let running = running_ids(snapshot);
    // 侧栏只保留有内容的会话，以及当前会话/正在运行的会话：
    // agent 会先落一个空“新会话”占位，全部列出来会淹没真正有内容的会话。
    array_at(snapshot, "threads")
        .iter()
        .map(|value| thread_row(value, &running))
        .filter(|thread| thread.item_count > 0 || thread.id == active_thread_id || thread.running)
        .collect()
}

/// 审批 + 排队托盘。返回 `(html, 待批准数, 排队数)`。
pub(super) fn trays_html(
    snapshot: &Value,
    raw_items: &[Value],
    thread_id: &str,
) -> (String, usize, usize) {
    let approvals = approval_rows(raw_items);
    let approvals_html = approvals
        .iter()
        .map(|row| {
            ui::ApprovalTray {
                call_id: row.call_id.clone(),
                thread_id: thread_id.to_string(),
                tool_name: row.tool_name.clone(),
                args: row.args.clone(),
                waited: row.waited.clone(),
            }
            .render()
        })
        .collect::<Vec<_>>()
        .join("");
    let queue = queue_rows(snapshot, thread_id);
    let queue_html = if queue.is_empty() {
        String::new()
    } else {
        ui::QueueTray {
            items: queue
                .iter()
                .map(|row| ui::QueueItem {
                    index: row.index,
                    order: row.order,
                    text: row.text.clone(),
                    thread_id: thread_id.to_string(),
                })
                .collect(),
        }
        .render()
    };
    (
        format!("{approvals_html}{queue_html}"),
        approvals.len(),
        queue.len(),
    )
}

/// 任务规划：按 set/update/get 的动作推演当前计划（不是“倒序找最后一张卡片”）。
pub(super) fn todo_html(raw_items: &[Value]) -> String {
    ui::derive_todo_plan(raw_items)
        .map(|plan| {
            ui::TodoPanel {
                plan,
                dom_id: "ada-todo".to_string(),
            }
            .render()
        })
        .unwrap_or_default()
}

pub(super) fn render_string<T: Template>(template: &T) -> String {
    template.render().unwrap_or_else(|error| {
        tracing::error!("模板渲染失败: {error}");
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approval_mode_reads_snapshot_config_approval() {
        let snapshot = json!({"config":{"approval":"full","model":"deepseek-chat"}});
        assert_eq!(approval_mode(&snapshot, &Value::Null), "full");
        assert_eq!(approval_label("full"), "完全访问");
        assert_eq!(approval_tone("full"), "danger");
    }

    #[test]
    fn approval_mode_falls_back_to_saved_config() {
        let snapshot = json!({});
        let config = json!({"saved":{"approval":"ask"}});
        assert_eq!(approval_mode(&snapshot, &config), "ask");
        assert_eq!(approval_mode(&snapshot, &Value::Null), "auto");
    }

    #[test]
    fn compact_row_uses_summary_not_text() {
        let row = row_view(&json!({
            "kind":"compact",
            "id":"c1",
            "summary":"压缩了 6 轮对话",
            "preTokens":90000,
            "postTokens":12000
        }));
        assert_eq!(row.key, "c1");
        assert!(row.text.contains("压缩了 6 轮对话"));
        assert!(row.text.contains("90000"));
        assert!(row.text.contains("12000"));
        assert!(ui::render_row(&row, "t1").contains("压缩了 6 轮对话"));
    }

    #[test]
    fn user_row_marks_images_instead_of_silently_dropping_them() {
        let row = row_view(&json!({
            "kind":"user",
            "id":"u1",
            "text":"看一下这张图",
            "images":["data:image/png;base64,AA"]
        }));
        assert_eq!(row.images, 1);
        assert!(ui::render_row(&row, "t1").contains("含 1 张图片"));
    }
}
