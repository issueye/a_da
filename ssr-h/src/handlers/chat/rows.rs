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
    // 🔴 这里必须返回**注册过的**图标键（`icons.rs` 里以 `i-` 开头）。
    // 早先返回的是 "folder" / "terminal" 这类裸名，查表永远落空，
    // 于是每条工具行的图标位都是空的——文案对、位置对，就是没有图。
    match name {
        "list_files" => "i-folder",
        "read_file" => "i-file",
        "write_file" | "edit_file" | "batch_write" | "batch_replace" => "i-edit",
        "code_outline" => "i-code",
        "find_symbol" => "i-search",
        "search_files" => "i-search",
        "project_inspect" => "i-grid",
        "create_plugin" => "i-plug",
        "run_command" | "run_background" => "i-terminal",
        "run_tests" => "i-check-circle",
        "check_task" => "i-clock",
        "kill_task" => "i-x-circle",
        "todo" => "i-list",
        "invoke_subagent" | "check_subagent" => "i-bot",
        "ask_user" => "i-chat",
        "git" => "i-git-branch",
        "weather" => "i-sun",
        "news_query" => "i-chart",
        "finish" => "i-check",
        // 没登记的工具用扳手：既不是文件夹也不是终端，别假装知道。
        _ => "i-wrench",
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
pub(super) fn elapsed_label(value: &Value, status: &str) -> String {
    if let Some(ms) = value.get("durationMs").and_then(Value::as_u64) {
        return duration_text(ms);
    }
    // 执行中：起点用 `startedAt`，客户端每秒接着走（服务端只负责给对的起点）。
    // 不用 `at` 兜底——它是排序用的序号，不是时间戳，拿它算会得出天文数字。
    if running_status(status) {
        if let Some(start) = started_ms(value) {
            let elapsed = now_ms() - start;
            // 起点明显不合理（时钟漂移、单位写错）就不要显示天文数字。
            if (0..86_400_000).contains(&elapsed) {
                let prefix = if status == "waiting_approval" {
                    "待批 "
                } else {
                    "已 "
                };
                return format!("{prefix}{}", duration_text(elapsed as u64));
            }
        }
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

/// 输入框下方那行遥测：速度 / 缓存命中 / 上下文占用。
///
/// 三个数都来自**最近一次模型调用自己报回的** usage 与耗时，不估算：
/// - `tok/s`  = completionTokens ÷ (durationMs ÷ 1000)
/// - 缓存命中 = cachedTokens ÷ promptTokens
/// - 上下文   = promptTokens ÷ contextWindow
///
/// 拿不到就留空——宁可少一个数，也别摆一个编出来的指标。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TurnStats {
    pub tokens_per_sec: Option<u64>,
    pub cache_hit: Option<u64>,
    pub context: Option<u64>,
}

pub(super) fn turn_stats(snapshot: &Value, thread: &Value) -> TurnStats {
    let latest = array_at(thread, "items")
        .iter()
        .rev()
        .find(|item| str_at(item, "kind") == "assistant" && prompt_tokens(item) > 0);
    let Some(item) = latest else {
        return TurnStats::default();
    };
    let usage = item.get("usage").cloned().unwrap_or(Value::Null);
    let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let prompt = number("promptTokens");
    let completion = number("completionTokens");
    // 缓存字段**缺失** ≠ 缓存为 0：缺失时算不出命中率，就别摆一个 0%（那会读成
    // "一点都没命中"）。模型真的报 0 时仍然显示 0%。
    let cached = usage.get("cachedTokens").and_then(Value::as_u64);
    let duration = item.get("durationMs").and_then(Value::as_u64).unwrap_or(0);
    let window = snapshot
        .get("config")
        .and_then(|config| config.get("contextWindow"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let percent_of = |part: u64, whole: u64| {
        (whole > 0).then(|| {
            let scaled = part.saturating_mul(100).saturating_add(whole / 2);
            (scaled / whole).min(100)
        })
    };
    TurnStats {
        // 太短的调用算不出有意义的速率（一个 200ms 的回复会算出三位数）。
        tokens_per_sec: (duration >= 200 && completion > 0)
            .then(|| (completion.saturating_mul(1000) / duration).max(1)),
        cache_hit: cached.and_then(|cached| percent_of(cached, prompt)),
        context: percent_of(prompt, window),
    }
}

fn prompt_tokens(item: &Value) -> u64 {
    item.get("usage")
        .and_then(|usage| usage.get("promptTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// 毫秒 → 人读的时长：`820ms` / `1.2s` / `1 分 12 秒`。
fn duration_text(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{} 分 {} 秒", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn started_ms(value: &Value) -> Option<i64> {
    value.get("startedAt").and_then(Value::as_i64)
}

/// 还在跑的状态（等待批准也算：用户要看到自己已经犹豫了多久）。
fn running_status(raw: &str) -> bool {
    matches!(raw, "running" | "queued" | "awaiting" | "waiting_approval")
}

/// 已经有结论的状态：只有这些才配结果尾标。
fn finished_status(raw: &str) -> bool {
    matches!(
        raw,
        "done" | "completed" | "success" | "error" | "failed" | "denied" | "timeout"
    )
}

fn failed_status(raw: &str) -> bool {
    matches!(raw, "error" | "failed" | "denied" | "timeout")
}

/// 折叠行尾标：一句话说清「这一步得到了什么」。
///
/// 全部来自工具自己的返回值（`output` / `patch` / `details`），不做估算；
/// 拿不到就**留空**——编一个"完成"上去比空着更糟。
fn result_summary(name: &str, args: &Value, item: &Value, todo: &[ui::TodoStep]) -> (String, Tone) {
    let status = str_at(item, "status");
    if !finished_status(status) {
        return (String::new(), Tone::Neutral);
    }
    let output = item.get("output").and_then(Value::as_str).unwrap_or("");
    let neutral = if failed_status(status) {
        Tone::Danger
    } else {
        Tone::Neutral
    };
    let outcome: Option<(String, Tone)> = match name {
        "read_file" => count_lines(output).map(|lines| (format!("{lines} 行"), neutral)),
        "list_files" => count_entries(output).map(|items| (format!("{items} 项"), neutral)),
        "write_file" => args
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.trim().is_empty())
            .map(|content| (format!("写入 {} 行", content.lines().count()), neutral)),
        "edit_file" | "batch_replace" | "batch_write" => item
            .get("patch")
            .and_then(Value::as_str)
            .and_then(patch_stats)
            .map(|(added, removed)| {
                let tone = if failed_status(status) {
                    Tone::Danger
                } else {
                    Tone::Success
                };
                (format!("+{added} -{removed}"), tone)
            }),
        "run_command" | "run_background" => exit_code(item).map(|code| {
            let tone = if code != 0 { Tone::Danger } else { neutral };
            (format!("exit {code}"), tone)
        }),
        "run_tests" => test_summary(output),
        "search_files" => count_entries(output).map(|hits| (format!("{hits} 处"), neutral)),
        "todo" => todo_summary(todo),
        // 这几个的结果本身就在正文里，尾标重复一遍只是噪声。
        "invoke_subagent" | "ask_user" | "finish" => None,
        _ => count_lines(output).map(|lines| (format!("{lines} 行"), neutral)),
    };
    outcome
        .filter(|(text, _)| !text.is_empty())
        .unwrap_or_else(|| (String::new(), Tone::Neutral))
}

/// 行数（空文本返回 `None`，避免给出"0 行"这种没信息量的尾标）。
fn count_lines(text: &str) -> Option<usize> {
    let lines = text.lines().count();
    (lines > 0).then_some(lines)
}

/// 非空行数：目录条目、搜索命中都是一行一条。
fn count_entries(text: &str) -> Option<usize> {
    let entries = text.lines().filter(|line| !line.trim().is_empty()).count();
    (entries > 0).then_some(entries)
}

/// 退出去码：只认工具结果里的结构化字段，不猜。
fn exit_code(item: &Value) -> Option<i64> {
    let details = item.get("details")?;
    details
        .get("exitCode")
        .or_else(|| details.get("exit_code"))
        .and_then(Value::as_i64)
}

/// 测试汇总：cargo 每个测试二进制都会打一行 `test result: ok. 128 passed; 0 failed`。
fn test_summary(output: &str) -> Option<(String, Tone)> {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut seen = false;
    for line in output.lines() {
        if let Some(count) = number_before(line, "passed") {
            passed += count;
            seen = true;
        }
        if let Some(count) = number_before(line, "failed") {
            failed += count;
            seen = true;
        }
    }
    if !seen {
        return None;
    }
    let tone = if failed > 0 {
        Tone::Danger
    } else {
        Tone::Success
    };
    Some((format!("{passed} 通过 · {failed} 失败"), tone))
}

/// `带 128 passed` → `128`：取关键词前面紧邻的数字。
fn number_before(line: &str, word: &str) -> Option<usize> {
    let head = line[..line.find(word)?].trim_end();
    let digits: String = head
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().ok()
}

/// 统一 diff 的增删行数。
fn patch_stats(patch: &str) -> Option<(usize, usize)> {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added + removed > 0).then_some((added, removed))
}

/// 任务规划的完成度：`3/12`。
fn todo_summary(steps: &[ui::TodoStep]) -> Option<(String, Tone)> {
    if steps.is_empty() {
        return None;
    }
    let done = steps
        .iter()
        .filter(|step| step.status == ui::TodoStatus::Completed)
        .count();
    Some((format!("{done}/{}", steps.len()), Tone::Neutral))
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
    // 行数按截断前的原文统计：按钮上写"共 42 行"就不能是截断后的 12 行。
    let output_lines = raw_output.lines().count();
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
    let (result, result_tone) = result_summary(&tool_name, &args_value, value, &todo_steps);
    let running = running_status(raw_status);
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
        result,
        result_tone,
        running,
        started_at: if running { started_ms(value) } else { None },
        output_lines,
        elapsed: elapsed_label(value, raw_status),
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
///
/// 外面包一层 `.ada-transcript__item`：SSE 流式更新按 `data-item-id` 定位、
/// 按 `data-at` 插入到正确位置（服务端推来的 item 可能比界面已有的更新，
/// 但顺序仍由 `at` 决定）。包装层不参与排版，只提供身份与定位。
pub(super) fn item_row(value: &Value, thread_id: &str) -> ItemRow {
    let id = str_at(value, "id");
    let at = value.get("at").and_then(Value::as_u64).unwrap_or(0);
    let html = format!(
        r#"<div class="ada-transcript__item" data-item-id="{}" data-at="{at}">{}</div>"#,
        crate::ui::theme::escape(id),
        ui::render_row(&row_view(value), thread_id),
    );
    ItemRow { html }
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

/// 推理等级的真值：`low | medium | high | max`（与 desktop-ui 设置项同源）。
///
/// 快照优先（运行时值），其次是 `/api/config` 的 `saved` / 顶层（旧网关）。
/// 缺省 `max` = 自动（与 [`crate::ui::model_picker::normalize_effort`] 口径一致）。
pub(super) fn effort_of(snapshot: &Value, config: &Value) -> String {
    snapshot
        .get("config")
        .and_then(|cfg| cfg.get("effort"))
        .or_else(|| config.get("saved").and_then(|saved| saved.get("effort")))
        .or_else(|| config.get("effort"))
        .and_then(Value::as_str)
        .map(|effort| effort.to_string())
        .unwrap_or_else(|| "max".to_string())
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
    ui::waited_from_at(at)
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
///
/// 过滤规则：
/// - 有内容 / 运行中 / 当前会话：全部保留；
/// - **没有真实会话的工作区**补一条最新的空会话作为占位。agent 启动时会先落一个空的
///   "新会话"，全列出来会淹没真实会话；但把空会话一律丢掉又会让
///   "刚添加、还没说话的 beta 工作区"在树里消失——所以只给空工作区留一条。
pub(crate) fn thread_rows(snapshot: &Value, active_thread_id: &str) -> Vec<ThreadRow> {
    let running = running_ids(snapshot);
    let rows: Vec<ThreadRow> = array_at(snapshot, "threads")
        .iter()
        .map(|value| thread_row(value, &running))
        .collect();

    let mut content_workspaces: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for thread in &rows {
        if thread.item_count > 0 || thread.running || thread.id == active_thread_id {
            content_workspaces.insert(super::super::workspaces::workspace_path_key(
                &thread.workspace,
            ));
        }
    }

    let mut newest_empty: std::collections::HashMap<String, (u64, String)> =
        std::collections::HashMap::new();
    for thread in &rows {
        if thread.item_count > 0 || thread.running || thread.id == active_thread_id {
            continue;
        }
        let key = super::super::workspaces::workspace_path_key(&thread.workspace);
        if content_workspaces.contains(&key) {
            continue;
        }
        let better = match newest_empty.get(&key) {
            Some((at, id)) => {
                thread.created_at > *at
                    || (thread.created_at == *at && thread.id.as_str() > id.as_str())
            }
            None => true,
        };
        if better {
            newest_empty.insert(key, (thread.created_at, thread.id.clone()));
        }
    }

    rows.into_iter()
        .filter(|thread| {
            thread.item_count > 0
                || thread.running
                || thread.id == active_thread_id
                || newest_empty
                    .get(&super::super::workspaces::workspace_path_key(
                        &thread.workspace,
                    ))
                    .map(|(_, id)| id == &thread.id)
                    .unwrap_or(false)
        })
        .collect()
}

/// 审批 + 提问 + 排队托盘。返回 `(html, 待批准数, 排队数)`。
///
/// `plugin_questions_html` 是 `thread.tray` 插槽的输出：**非空时替换核心提问托盘**
/// （审批/排队不受影响）；插件停用、加载失败或没渲染出东西时传 `None`/空串，
/// 自动退回核心实现（[docs/PLAN-question-panel.md](../../docs/PLAN-question-panel.md) D5）。
pub(super) fn trays_html(
    snapshot: &Value,
    raw_items: &[Value],
    thread_id: &str,
    plugin_questions_html: Option<&str>,
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
    let questions_html = match plugin_questions_html {
        Some(html) if !html.trim().is_empty() => html.to_string(),
        _ => crate::questions::pending_questions_from_items(raw_items)
            .iter()
            .map(|question| {
                ui::QuestionTray {
                    question: question.clone(),
                    thread_id: thread_id.to_string(),
                }
                .render()
            })
            .collect::<Vec<_>>()
            .join(""),
    };
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
        format!("{approvals_html}{questions_html}{queue_html}"),
        approvals.len(),
        queue.len(),
    )
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

    #[test]
    fn tool_row_reports_how_many_lines_were_read() {
        let row = row_view(&json!({
            "kind":"tool","id":"t1","name":"read_file","status":"done",
            "args":{"path":"rows.rs"},"output":"a\nb\nc"
        }));
        assert_eq!(row.result, "3 行");
        assert_eq!(row.result_tone, Tone::Neutral);
    }

    #[test]
    fn command_row_reports_exit_code_and_colors_failure() {
        let ok = row_view(&json!({
            "kind":"tool","id":"t1","name":"run_command","status":"done",
            "args":{"command":"cargo build"},"details":{"exitCode":0}
        }));
        assert_eq!(ok.result, "exit 0");
        assert_eq!(ok.result_tone, Tone::Neutral);

        let failed = row_view(&json!({
            "kind":"tool","id":"t2","name":"run_command","status":"error",
            "args":{"command":"cargo build"},"details":{"exitCode":101}
        }));
        assert_eq!(failed.result, "exit 101");
        assert_eq!(failed.result_tone, Tone::Danger);
    }

    #[test]
    fn edit_row_counts_added_and_removed_lines() {
        let row = row_view(&json!({
            "kind":"tool","id":"t1","name":"edit_file","status":"done",
            "args":{"path":"rows.rs"},
            "patch":"--- a/rows.rs\n+++ b/rows.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+more\n"
        }));
        assert_eq!(row.result, "+2 -1");
        assert_eq!(row.result_tone, Tone::Success);
    }

    #[test]
    fn running_tool_has_no_result_but_keeps_a_live_start_point() {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0)
            - 3000;
        let row = row_view(&json!({
            "kind":"tool","id":"t1","name":"run_command","status":"running",
            "args":{"command":"npm run build"},"startedAt":started
        }));
        assert!(row.result.is_empty(), "执行中不该给结果尾标");
        assert!(row.running);
        assert_eq!(row.started_at, Some(started));
        assert!(row.elapsed.starts_with("已 "), "{}", row.elapsed);
    }

    #[test]
    fn test_runner_row_sums_passed_and_failed() {
        let row = row_view(&json!({
            "kind":"tool","id":"t1","name":"run_tests","status":"done",
            "output":"test result: ok. 128 passed; 0 failed; 3 ignored\n"
        }));
        assert_eq!(row.result, "128 通过 · 0 失败");
        assert_eq!(row.result_tone, Tone::Success);
    }

    #[test]
    fn every_tool_icon_key_is_registered() {
        // 回归：曾经返回 "folder"/"terminal" 这类裸名，图标表只认 `i-*`，
        // 于是每条工具行的图标位都是空的（肉眼看着像"没画图"）。
        let tools = [
            "list_files",
            "read_file",
            "write_file",
            "edit_file",
            "batch_write",
            "batch_replace",
            "code_outline",
            "find_symbol",
            "search_files",
            "project_inspect",
            "create_plugin",
            "run_command",
            "run_background",
            "run_tests",
            "check_task",
            "kill_task",
            "todo",
            "invoke_subagent",
            "check_subagent",
            "ask_user",
            "git",
            "weather",
            "news_query",
            "finish",
            "some_unknown_tool",
        ];
        for name in tools {
            let key = tool_icon(name);
            assert!(
                crate::ui::icons::has(&key),
                "{name} → {key} 不是已注册的图标键"
            );
            assert!(key.starts_with("i-"), "{name} → {key} 缺少 i- 前缀");
        }
    }

    #[test]
    fn stats_read_speed_cache_and_context_from_the_last_turn() {
        let snapshot = json!({"config": {"contextWindow": 200000}});
        let thread = json!({"items": [
            {"kind": "user", "text": "hi"},
            {"kind": "assistant", "text": "ok", "durationMs": 2000,
             "usage": {"promptTokens": 50000, "completionTokens": 400, "cachedTokens": 45000}}
        ]});
        let stats = turn_stats(&snapshot, &thread);
        assert_eq!(stats.tokens_per_sec, Some(200));
        assert_eq!(stats.cache_hit, Some(90));
        assert_eq!(stats.context, Some(25));
    }

    #[test]
    fn stats_skip_numbers_they_cannot_trust() {
        // 太短的一回合算不出有意义的速率；没有 usage 就一个数都不给。
        let snapshot = json!({"config": {"contextWindow": 0}});
        let short = json!({"items": [
            {"kind": "assistant", "text": "ok", "durationMs": 120,
             "usage": {"promptTokens": 10, "completionTokens": 2, "cachedTokens": 0}}
        ]});
        let stats = turn_stats(&snapshot, &short);
        assert_eq!(stats.tokens_per_sec, None);
        assert_eq!(stats.context, None, "窗口没配就不该编百分比");
        assert_eq!(stats.cache_hit, Some(0));

        let empty = turn_stats(&snapshot, &json!({"items": []}));
        assert_eq!(empty.tokens_per_sec, None);
        assert_eq!(empty.cache_hit, None);

        // 模型没报 `cachedTokens`：是"算不出来"，不是"命中 0%"。
        let no_cache = json!({"items": [
            {"kind": "assistant", "durationMs": 2000,
             "usage": {"promptTokens": 100, "completionTokens": 10}}
        ]});
        assert_eq!(turn_stats(&snapshot, &no_cache).cache_hit, None);
    }
}
