use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use serde_json::Value;

use crate::ai::{ProviderConfig, ToolCallInfo};
use crate::checkpoint::CheckpointManager;
use crate::session::AgentMessage;
use crate::subagents::SubagentManager;
use crate::tools::{
    check_workspace_sandbox, edit_file, list_files, read_file, run_command, search_files,
    write_file, EditPair, ToolResult,
};

/// 派发并执行单个工具调用
pub fn execute_tool_call<'a>(
    workspace: &'a Path,
    thread_id: &'a str,
    call: &'a ToolCallInfo,
    checkpoint_mgr: Option<&'a Arc<CheckpointManager>>,
) -> BoxFuture<'a, AgentMessage> {
    execute_tool_call_extended(workspace, thread_id, call, checkpoint_mgr, None, None)
}

/// 派发并执行单个工具调用（扩展支持模型配置与子智能体管理）
pub fn execute_tool_call_extended<'a>(
    workspace: &'a Path,
    thread_id: &'a str,
    call: &'a ToolCallInfo,
    checkpoint_mgr: Option<&'a Arc<CheckpointManager>>,
    parent_config: Option<&'a ProviderConfig>,
    subagent_mgr: Option<&'a Arc<SubagentManager>>,
) -> BoxFuture<'a, AgentMessage> {
    async move {
        let args: Value = serde_json::from_str(&call.args).unwrap_or_else(|_| serde_json::json!({}));
        let mut checkpoint_id: Option<String> = None;

    let result = match call.name.as_str() {
        "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let offset = args.get("offset").and_then(|v| v.as_u64()).map(|n| n as usize);
            let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);
            read_file(workspace, path, offset, limit)
        }
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

            // 拍摄写前检查点
            if let Some(mgr) = checkpoint_mgr {
                if let Ok(abs) = check_workspace_sandbox(workspace, path) {
                    if let Ok(record) = mgr.capture(thread_id, &call.id, &[(path, &abs)]) {
                        checkpoint_id = Some(record.id);
                    }
                }
            }

            write_file(workspace, path, content)
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old_string = args.get("old_string").and_then(|v| v.as_str());
            let new_string = args.get("new_string").and_then(|v| v.as_str());

            let edits = args.get("edits").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let o = item.get("old_string").and_then(|v| v.as_str())?;
                        let n = item.get("new_string").and_then(|v| v.as_str())?;
                        Some(EditPair {
                            old_string: o.to_string(),
                            new_string: n.to_string(),
                        })
                    })
                    .collect()
            });

            // 拍摄写前检查点
            if let Some(mgr) = checkpoint_mgr {
                if let Ok(abs) = check_workspace_sandbox(workspace, path) {
                    if let Ok(record) = mgr.capture(thread_id, &call.id, &[(path, &abs)]) {
                        checkpoint_id = Some(record.id);
                    }
                }
            }

            edit_file(workspace, path, old_string, new_string, edits)
        }
        "list_files" => {
            let path = args.get("path").and_then(|v| v.as_str());
            let depth = args.get("depth").and_then(|v| v.as_u64()).map(|n| n as usize);
            list_files(workspace, path, depth)
        }
        "search_files" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let glob = args.get("glob").and_then(|v| v.as_str());
            let path = args.get("path").and_then(|v| v.as_str());
            let literal = args.get("literal").and_then(|v| v.as_bool()).unwrap_or(false);
            let case_sensitive = args.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(false);
            let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            search_files(workspace, pattern, glob, path, literal, case_sensitive, context)
        }
        "run_command" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = args.get("cwd").and_then(|v| v.as_str());
            let timeout = args.get("timeout").and_then(|v| v.as_u64());
            run_command(workspace, command, cwd, timeout, None).await
        }
        "invoke_subagent" => {
            let subagent_id = args.get("subagent_id").and_then(|v| v.as_str()).unwrap_or("");
            let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
            let additional_context = args
                .get("additional_context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let default_mgr = SubagentManager::new();
            let profile = if let Some(mgr) = subagent_mgr {
                mgr.get_profile(subagent_id, Some(workspace))
            } else {
                default_mgr.get_profile(subagent_id, Some(workspace))
            };

            if let Some(profile) = profile {
                if !profile.enabled {
                    ToolResult::error(format!("子智能体 [{}] 已被禁用", profile.name))
                } else {
                    let config = parent_config.cloned().unwrap_or_else(|| ProviderConfig {
                        id: "gemini".to_string(),
                        name: "Gemini".to_string(),
                        protocol: crate::ai::ModelProtocol::OpenAiChat,
                        api_key: String::new(),
                        base_url: String::new(),
                        model: "gemini-2.5-flash".to_string(),
                        max_output_tokens: Some(8192),
                        custom_headers: None,
                    });

                    let res = crate::subagents::run_subagent(crate::subagents::RunSubagentOptions {
                        profile,
                        task: task.to_string(),
                        additional_context,
                        workspace: workspace.to_path_buf(),
                        parent_config: config,
                        checkpoint_mgr: checkpoint_mgr.map(|m| Arc::clone(m)),
                        abort_rx: None,
                        update_tx: None,
                    })
                    .await;

                    if res.ok {
                        ToolResult::success(res.summary)
                    } else {
                        ToolResult::error(res.error_message.unwrap_or(res.summary))
                    }
                }
            } else {
                ToolResult::error(format!("找不到指定的子智能体配置: {}", subagent_id))
            }
        }
        unknown => {
            let plugin_mgr = crate::plugins::PluginManager::new();
            let plugins = plugin_mgr.scan_plugins(Some(workspace.to_str().unwrap_or("")));
            let mut target_plugin_path = None;

            for item in plugins {
                if !item.enabled {
                    continue;
                }
                if item.tools.iter().any(|t| t.name == unknown) {
                    target_plugin_path = Some(std::path::PathBuf::from(item.file_path));
                    break;
                }
            }

            if let Some(p_path) = target_plugin_path {
                match crate::plugins::PluginSandbox::call_tool(&p_path, unknown, args, workspace, 30).await {
                    Ok(res) => res,
                    Err(e) => ToolResult::error(format!("插件工具 [{}] 执行失败: {}", unknown, e)),
                }
            } else {
                ToolResult::error(format!("未知工具: {}", unknown))
            }
        }
    };

        AgentMessage::ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: result.output,
            is_error: if result.ok { None } else { Some(true) },
            details: result.details,
            patch: result.patch,
            checkpoint_id,
            timestamp: Some(now_ms()),
        }
    }
    .boxed()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
