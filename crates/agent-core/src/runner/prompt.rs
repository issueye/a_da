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

/// 获取当前工作区的所有可用工具（包含内置核心工具与已启用的扩展插件工具）
pub fn get_all_tools_for_workspace(workspace: &str) -> Vec<ChatCompletionTool> {
    let mut tools = builtin_tools();

    let plugin_mgr = crate::plugins::PluginManager::new();
    let plugins = plugin_mgr.scan_plugins(Some(workspace));

    for item in plugins {
        if !item.enabled {
            continue;
        }
        for t in &item.tools {
            if tools.iter().any(|b| b.function.name == t.name) {
                continue;
            }

            let params = t.parameters.clone().unwrap_or_else(|| {
                json!({ "type": "object", "properties": {} })
            });

            tools.push(ChatCompletionTool {
                tool_type: "function".to_string(),
                function: ChatCompletionToolFunction {
                    name: t.name.clone(),
                    description: if t.description.is_empty() {
                        format!("已启用的扩展工具: {}", t.name)
                    } else {
                        t.description.clone()
                    },
                    parameters: params,
                },
            });
        }
    }

    tools
}

/// 构造系统提示词。
///
/// **兼容 shim（计划 R6）**：人格文本的**排版规则**已按 W1-T3 搬到
/// `agent_adapter::prompt::compose_system_prompt`（唯一真源）。这里只保留
/// "采集哪些扩展已启用"这件事——因为插件扫描器 `PluginManager` 还在 `agent-core`，
/// 把它也搬走属于 `plugin/` 适配器的任务，不在本批次。
///
/// 行为与搬运前**逐字节一致**，由 `test_prompt_shim_matches_adapter_composer_byte_for_byte` 钉住。
pub fn build_system_prompt(workspace: &str) -> String {
    let plugin_mgr = crate::plugins::PluginManager::new();
    let plugins = plugin_mgr.scan_plugins(Some(workspace));
    let mut enabled_extensions = Vec::new();
    for p in plugins {
        if !p.enabled {
            continue;
        }
        for t in p.tools {
            enabled_extensions.push((t.name, t.description));
        }
    }

    agent_adapter::prompt::compose_system_prompt(workspace, &enabled_extensions)
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
            reasoning_content: None,
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
                    reasoning_content: None,
                });
            }
            AgentMessage::Assistant {
                content,
                thinking,
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
                    // thinking 模式：上游要求把上一轮思考链原样带回，否则多轮工具调用直接 400
                    reasoning_content: thinking
                        .clone()
                        .filter(|t| !t.trim().is_empty()),
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
                    reasoning_content: None,
                });
            }
            AgentMessage::Unknown => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W1-T3 守门：提示词排版必须**只有一处真源**（`agent_adapter::prompt::compose_system_prompt`）。
    ///
    /// 为什么这条断言重要：提示词文本一变，模型看到的东西就变了（历史回放、截图、
    /// 用户预期全都会漂）。所以"搬运"这件事必须可证明没有改行为。
    ///
    /// 用**不存在的固定工作区**：扫描不到工作区级插件；`cfg(test)` 下 `app_home`
    /// 是临时目录（也不存在），所以扩展列表 = 9 个内置插件的工具，**确定性可断言**。
    #[test]
    fn test_prompt_shim_delegates_to_adapter_composer() {
        const WS: &str = "E:/a_da_prompt_test_ws";

        let shim = build_system_prompt(WS);
        let static_only = agent_adapter::prompt::compose_system_prompt(WS, &[]);

        // 1) 静态段必须**逐字节来自适配器**——证明 agent-core 里没有第二份排版
        assert!(
            shim.starts_with(&static_only),
            "shim 的静态段必须来自适配器的纯函数（不许在 agent-core 里另写一份）"
        );

        // 2) 内置插件恒存在 → 必须追加扩展段落，且标题/结尾与搬运前一致
        assert!(
            shim.len() > static_only.len(),
            "内置插件应当产生扩展段落（否则说明扫描链路断了）"
        );
        assert!(shim.contains("\n\n当前已启用的扩展工具与额外能力：\n"));
        assert!(shim.ends_with("你具备上述扩展工具所赋予的能力（如联网搜索等）。"));

        // 3) 静态文本冻结：改提示词必须是有意的改动，不是搬运事故
        assert!(
            static_only.starts_with(
                "你是 a-da，一个由 Rust 原生核心驱动的高性能 AI 编程智能体。\n\
                 当前工作区根目录为：E:/a_da_prompt_test_ws\n\
                 请遵循以下指引：\n"
            ),
            "人格/指引文本被改动了：{static_only}"
        );
    }
}
