use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};
use tracing::warn;

use super::types::{
    SubagentMode, SubagentProfile, SubagentRunResult, SubagentStepUpdate,
};
use crate::ai::{
    stream_model_chat, ChatCompletionTool, ModelChatOptions, ProviderConfig, StreamDelta,
};
use crate::checkpoint::CheckpointManager;
use crate::runner::executor::execute_tool_call;
use crate::runner::prompt::format_messages_for_model;
use crate::session::AgentMessage;
use crate::tools::is_write_tool;

/// 子智能体永远禁止调用的套娃/递归工具
pub const NEVER_FOR_SUBAGENT: &[&str] = &[
    "invoke_subagent",
    "check_subagent",
    "send_subagent_message",
    "resume_subagent",
    "await_subagents",
];

/// 解析并过滤子智能体可用工具
pub fn resolve_subagent_tools(
    profile: &SubagentProfile,
    all_tools: &[ChatCompletionTool],
) -> Vec<ChatCompletionTool> {
    let allowed_set: HashSet<&str> = profile.allowed_tools.iter().map(|s| s.as_str()).collect();
    let mut disallowed_set: HashSet<&str> = NEVER_FOR_SUBAGENT.iter().copied().collect();
    if let Some(ref list) = profile.disallowed_tools {
        for item in list {
            disallowed_set.insert(item.as_str());
        }
    }

    all_tools
        .iter()
        .filter(|tool| {
            let name = tool.function.name.as_str();
            // 1. 递归黑名单与显式黑名单
            if disallowed_set.contains(name) {
                return false;
            }
            // 2. 白名单检查（支持 '*' 通配）
            if !allowed_set.contains("*") && !allowed_set.contains(name) {
                return false;
            }
            // 3. 只读安全防护：只读模式严格禁止任何写工具 (AGENTS.md §2)
            if profile.mode == SubagentMode::Readonly && is_write_tool(name) {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

pub struct RunSubagentOptions {
    pub profile: SubagentProfile,
    pub task: String,
    pub additional_context: Option<String>,
    pub workspace: std::path::PathBuf,
    pub parent_config: ProviderConfig,
    pub checkpoint_mgr: Option<Arc<CheckpointManager>>,
    pub abort_rx: Option<watch::Receiver<bool>>,
    pub update_tx: Option<mpsc::Sender<SubagentStepUpdate>>,
}

/// 纯 Rust 子智能体隔离执行器
pub async fn run_subagent(options: RunSubagentOptions) -> SubagentRunResult {
    let start_time = Instant::now();
    let max_steps = options.profile.max_steps.unwrap_or(25);

    if !options.profile.enabled {
        return SubagentRunResult {
            ok: false,
            summary: format!("子智能体 [{}] 已被禁用", options.profile.name),
            steps_executed: 0,
            duration_ms: start_time.elapsed().as_millis() as u64,
            tool_calls_count: 0,
            output_file: None,
            error_message: Some("子智能体已被禁用".to_string()),
        };
    }

    // 1. 构建可用工具列表并进行安全白名单裁切
    let all_tools = crate::runner::prompt::builtin_tools();
    let authorized_tools = resolve_subagent_tools(&options.profile, &all_tools);

    // 2. 覆盖模型配置（如有）
    let mut config = options.parent_config.clone();
    if let Some(ref override_cfg) = options.profile.model_override {
        if let Some(ref m) = override_cfg.model {
            config.model = m.clone();
        }
    }

    // 3. 初始化独立上下文（遵守 AGENTS.md §3：只拿到 profile.system_prompt）
    let mut user_prompt = format!("【委派任务】\n{}", options.task);
    if let Some(ref ctx) = options.additional_context {
        if !ctx.trim().is_empty() {
            user_prompt.push_str(&format!("\n\n【补充上下文/参考信息】\n{}", ctx.trim()));
        }
    }
    user_prompt.push_str("\n\n请针对上述任务要求，自主使用工具调研或处理。完成后直接给出结构化、高信息密度的最终总结与建议。");

    let mut history_messages = vec![AgentMessage::User {
        content: user_prompt,
        images: None,
        timestamp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
    }];

    let mut steps_executed = 0;
    let mut total_tool_calls = 0;
    let mut final_summary = String::new();

    // 4. 独立决策与工具执行流
    for step in 0..max_steps {
        steps_executed = step + 1;

        if let Some(ref tx) = options.update_tx {
            let _ = tx
                .send(SubagentStepUpdate {
                    thread_id: None,
                    step: steps_executed,
                    max_steps: Some(max_steps),
                    status: "running".to_string(),
                    current_action: Some("思考与规划中...".to_string()),
                    tool_call_summary: None,
                })
                .await;
        }

        let chat_messages =
            format_messages_for_model(&options.profile.system_prompt, &history_messages);

        let model_options = ModelChatOptions {
            tools: Some(authorized_tools.clone()),
            system_prompt: None,
            temperature: Some(0.2),
            effort: None,
            max_retries: Some(2),
        };

        let mut stream_rx = stream_model_chat(
            config.clone(),
            chat_messages,
            model_options,
            options.abort_rx.clone(),
        )
        .await;

        let mut accumulated_text = String::new();
        let mut tool_calls = Vec::new();

        while let Some(delta) = stream_rx.recv().await {
            match delta {
                StreamDelta::Text { text } => {
                    accumulated_text.push_str(&text);
                }
                StreamDelta::ToolCall { call } => {
                    tool_calls.push(call);
                }
                StreamDelta::Error { error } => {
                    warn!("[SubagentRunner] stream error: {}", error);
                }
                _ => {}
            }
        }

        final_summary = accumulated_text.clone();

        // 如果模型没有调用任何工具，代表任务回答完成，直接收敛收尾
        if tool_calls.is_empty() {
            break;
        }

        total_tool_calls += tool_calls.len();

        // 记录助手消息
        let assistant_tool_calls: Vec<crate::session::ToolCallBlock> = tool_calls
            .iter()
            .map(|tc| crate::session::ToolCallBlock {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: serde_json::from_str(&tc.args).unwrap_or(serde_json::Value::Null),
                raw_arguments: tc.args.clone(),
            })
            .collect();

        history_messages.push(AgentMessage::Assistant {
            content: accumulated_text,
            thinking: None,
            tool_calls: Some(assistant_tool_calls),
            stop_reason: Some("tool_calls".to_string()),
            error_message: None,
            timestamp: None,
            usage: None,
            duration_ms: None,
            turn_duration_ms: None,
        });

        // 依次执行工具调用
        for call in &tool_calls {
            // 安全双重防线：只读模式拦截
            if options.profile.mode == SubagentMode::Readonly && is_write_tool(&call.name) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let err_text = format!("安全拦截：子智能体 [{}] 为只读模式，严禁调用写工具 [{}]", options.profile.name, call.name);
                let structured = serde_json::json!({
                    "status": "error",
                    "ok": false,
                    "duration_ms": 0,
                    "durationMs": 0,
                    "started_at": now,
                    "startedAt": now,
                    "finished_at": now,
                    "finishedAt": now,
                    "output": err_text,
                });
                let content = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| err_text);
                let err_msg = AgentMessage::ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content,
                    is_error: Some(true),
                    details: None,
                    patch: None,
                    checkpoint_id: None,
                    timestamp: Some(now),
                    status: Some("error".to_string()),
                    duration_ms: Some(0),
                    started_at: Some(now),
                    finished_at: Some(now),
                };
                history_messages.push(err_msg);
                continue;
            }

            let result_msg = execute_tool_call(
                &options.workspace,
                "subagent_thread",
                call,
                options.checkpoint_mgr.as_ref(),
            )
            .await;

            history_messages.push(result_msg);
        }
    }

    if let Some(ref tx) = options.update_tx {
        let _ = tx
            .send(SubagentStepUpdate {
                thread_id: None,
                step: steps_executed,
                max_steps: Some(max_steps),
                status: "done".to_string(),
                current_action: Some("执行完毕".to_string()),
                tool_call_summary: None,
            })
            .await;
    }

    SubagentRunResult {
        ok: true,
        summary: if final_summary.is_empty() {
            "子智能体已完成委派步骤。".to_string()
        } else {
            final_summary
        },
        steps_executed,
        duration_ms: start_time.elapsed().as_millis() as u64,
        tool_calls_count: total_tool_calls,
        output_file: None,
        error_message: None,
    }
}
