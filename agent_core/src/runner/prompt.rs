use serde_json::json;

use crate::ai::{ChatCompletionMessage, ChatCompletionTool, ChatCompletionToolFunction};
use crate::session::AgentMessage;

/// 构造标准的内置工具声明
pub fn builtin_tools() -> Vec<ChatCompletionTool> {
    vec![
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "read_file".to_string(),
                description: "读取工作区内的文本文件。支持按行号范围读取 (offset / limit)。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "相对于工作区根目录的文件路径。" },
                        "offset": { "type": "number", "description": "起始行号（1 起始，可选）。" },
                        "limit": { "type": "number", "description": "最多读取行数（可选，默认 400 行）。" }
                    },
                    "required": ["path"]
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "write_file".to_string(),
                description: "创建或覆写整个文件。若只需修改部分代码，优先使用 edit_file。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径（相对于工作区）。" },
                        "content": { "type": "string", "description": "文件的完整新内容。" }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "edit_file".to_string(),
                description: "在文件中用 new_string 精确替换 old_string。old_string 必须在文件中仅出现一次。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径（相对于工作区）。" },
                        "old_string": { "type": "string", "description": "待替换的原始文本（必须在文件中唯一）。" },
                        "new_string": { "type": "string", "description": "替换后的新文本。" }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "list_files".to_string(),
                description: "列出工作区内的文件与目录。在读取未知路径前请先使用此工具确认结构。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "相对于工作区根目录的子目录（可选）。" },
                        "depth": { "type": "number", "description": "遍历深度（默认 3）。" }
                    }
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "search_files".to_string(),
                description: "在工作区文件内容中搜索并返回带行号的匹配行。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "搜索文本或正则表达式。" },
                        "glob": { "type": "string", "description": "可选的文件扩展名过滤（如 tsx、rs）。" },
                        "path": { "type": "string", "description": "可选限定在某个子目录内。" }
                    },
                    "required": ["pattern"]
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "run_command".to_string(),
                description: "在工作区内执行 Shell 命令。输出受截断保护。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的 Shell 命令。" },
                        "cwd": { "type": "string", "description": "工作区内的相对子目录（可选）。" },
                        "timeout": { "type": "number", "description": "超时秒数（可选，默认 120 秒）。" }
                    },
                    "required": ["command"]
                }),
            },
        },
        ChatCompletionTool {
            tool_type: "function".to_string(),
            function: ChatCompletionToolFunction {
                name: "invoke_subagent".to_string(),
                description: "委派专职子智能体（如 researcher 调研专员、code_reviewer 审查专员、tester 测试专员）在隔离上下文中执行定向任务。".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "subagent_id": {
                            "type": "string",
                            "description": "子智能体标识符，如 researcher, code_reviewer, tester, general_purpose。"
                        },
                        "task": {
                            "type": "string",
                            "description": "委派给子智能体的具体任务目标与要求。"
                        },
                        "additional_context": {
                            "type": "string",
                            "description": "补充的参考上下文信息（可选）。"
                        }
                    },
                    "required": ["subagent_id", "task"]
                }),
            },
        },
    ]
}

/// 构造系统提示词
pub fn build_system_prompt(workspace: &str) -> String {
    format!(
        "你是 a-da，一个由 Rust 原生核心驱动的高性能 AI 编程智能体。\n\
         当前工作区根目录为：{}\n\
         请遵循以下指引：\n\
         1. 谨慎修改代码。在修改未知文件前，先用 list_files 或 read_file 确认文件结构。\n\
         2. 对于局部小修改，优先使用 edit_file 工具以保持代码精准并生成 Unified Diff。\n\
         3. 执行终端命令时注意避免执行可能导致死循环的阻塞指令。\n\
         4. 所有的回复都使用清晰、专业的中文表达。",
        workspace
    )
}

/// 将会话流水消息转换为 OpenAI 模型请求消息序列
pub fn format_messages_for_model(
    system_prompt: &str,
    history: &[AgentMessage],
) -> Vec<ChatCompletionMessage> {
    let mut out = Vec::new();

    if !system_prompt.trim().is_empty() {
        out.push(ChatCompletionMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in history {
        match msg {
            AgentMessage::User { content, .. } => {
                out.push(ChatCompletionMessage {
                    role: "user".to_string(),
                    content: Some(content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            AgentMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let tc_json = tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    "arguments": if !c.raw_arguments.is_empty() {
                                        c.raw_arguments.clone()
                                    } else {
                                        c.arguments.to_string()
                                    }
                                }
                            })
                        })
                        .collect()
                });

                out.push(ChatCompletionMessage {
                    role: "assistant".to_string(),
                    content: if content.is_empty() && tc_json.is_some() {
                        None
                    } else {
                        Some(content.clone())
                    },
                    tool_calls: tc_json,
                    tool_call_id: None,
                });
            }
            AgentMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                out.push(ChatCompletionMessage {
                    role: "tool".to_string(),
                    content: Some(content.clone()),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id.clone()),
                });
            }
            AgentMessage::Unknown => {}
        }
    }

    out
}
