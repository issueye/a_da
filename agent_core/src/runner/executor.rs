use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use serde_json::Value;

use tokio::sync::{mpsc, watch};

use crate::ai::{ProviderConfig, ToolCallInfo};
use crate::checkpoint::CheckpointManager;
use crate::runner::builtin_tools::{execute_ask_user, execute_builtin_plugin_tool};
use crate::runner::AgentLoopEvent;
use crate::session::AgentMessage;
use crate::subagents::SubagentManager;
use crate::tools::{
    check_workspace_sandbox, edit_file, list_files, read_file, run_command, search_files,
    write_file, EditPair, ToolResult,
};

/// 依据 ToolDescriptor 声明的 RollbackPolicy 与 Access 拍摄写前检查点（INV-3：单一真源）
fn capture_tool_checkpoint(
    mgr: &Arc<CheckpointManager>,
    workspace: &Path,
    thread_id: &str,
    call_id: &str,
    tool_name: &str,
    args: &Value,
) -> Option<String> {
    if let Some(desc) = crate::tools::find_tool_descriptor(tool_name) {
        if let agent_base::domain::Access::Mutates { paths } = &desc.access {
            match paths {
                agent_base::domain::PathSelector::Single(field) => {
                    if let Some(path) = args.get(*field).and_then(|v| v.as_str()) {
                        if let Ok(abs) = check_workspace_sandbox(workspace, path) {
                            if let Ok(record) = mgr.capture(thread_id, call_id, &[(path, &abs)]) {
                                return Some(record.id);
                            }
                        }
                    }
                }
                agent_base::domain::PathSelector::Batch(field) => {
                    if let Some(arr) = args.get(*field).and_then(|v| v.as_array()) {
                        let mut abs_paths = Vec::new();
                        for item in arr {
                            let p_opt = item.as_str().or_else(|| item.get("path").and_then(|v| v.as_str()));
                            if let Some(p) = p_opt {
                                if let Ok(abs) = check_workspace_sandbox(workspace, p) {
                                    abs_paths.push((p, abs));
                                }
                            }
                        }
                        let targets: Vec<(&str, &Path)> = abs_paths.iter().map(|(p, abs)| (*p, abs.as_path())).collect();
                        if !targets.is_empty() {
                            if let Ok(record) = mgr.capture(thread_id, call_id, &targets) {
                                return Some(record.id);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// 派发并执行单个工具调用
pub fn execute_tool_call<'a>(
    workspace: &'a Path,
    thread_id: &'a str,
    call: &'a ToolCallInfo,
    checkpoint_mgr: Option<&'a Arc<CheckpointManager>>,
) -> BoxFuture<'a, AgentMessage> {
    execute_tool_call_extended(workspace, thread_id, call, checkpoint_mgr, None, None, None, None)
}

/// 派发并执行单个工具调用（扩展支持模型配置与子智能体管理、提问事件与中止信号）
pub fn execute_tool_call_extended<'a>(
    workspace: &'a Path,
    thread_id: &'a str,
    call: &'a ToolCallInfo,
    checkpoint_mgr: Option<&'a Arc<CheckpointManager>>,
    parent_config: Option<&'a ProviderConfig>,
    subagent_mgr: Option<&'a Arc<SubagentManager>>,
    event_tx: Option<&'a mpsc::Sender<AgentLoopEvent>>,
    abort_rx: Option<&'a watch::Receiver<bool>>,
) -> BoxFuture<'a, AgentMessage> {
    async move {
        let started_at = now_ms();
        let args: Value = serde_json::from_str(&call.args).unwrap_or_else(|_| serde_json::json!({}));
        let mut checkpoint_id: Option<String> = None;

        // INV-3 / M2-T1：统一由 ToolDescriptor 声明的 RollbackPolicy 与 Access 驱动拍摄检查点，彻底消除五处名单之手写检查点名单
        if let Some(mgr) = checkpoint_mgr {
            checkpoint_id = capture_tool_checkpoint(mgr, workspace, thread_id, &call.id, &call.name, &args);
        }

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
                        proxy_url: None,
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
        "ask_user" => {
            execute_ask_user(&call.id, &args, event_tx, abort_rx).await
        }
        unknown => {
            // M2-T3 / M2-T5：先判启用再执行（含内置插件工具，杜绝静默绕过）
            let plugin_mgr = crate::plugins::PluginManager::new();
            let plugins = plugin_mgr.scan_plugins(Some(workspace.to_str().unwrap_or("")));

            // 查找提供该工具的插件项
            let matched_plugin = plugins.iter().find(|item| {
                item.tools.iter().any(|t| t.name == unknown)
            });

            if let Some(plugin) = matched_plugin {
                if !plugin.enabled {
                    ToolResult::error(format!("插件 [{}] 已被禁用，无法执行工具 [{}]", plugin.name, unknown))
                } else if plugin.file_path.starts_with("(builtin):") {
                    // 内置插件原生工具执行
                    if let Some(res) = execute_builtin_plugin_tool(workspace, thread_id, unknown, &args, checkpoint_mgr).await {
                        res
                    } else {
                        ToolResult::error(format!("内置插件工具 [{}] 未注册有效执行器", unknown))
                    }
                } else {
                    // 第三方/工作区沙箱插件执行
                    let p_path = std::path::PathBuf::from(&plugin.file_path);
                    match crate::plugins::PluginSandbox::call_tool(&p_path, unknown, args, workspace, 30).await {
                        Ok(res) => res,
                        Err(e) => ToolResult::error(format!("插件工具 [{}] 执行失败: {}", unknown, e)),
                    }
                }
            } else {
                ToolResult::error(format!("未知工具: {}", unknown))
            }
        }
    };

        let finished_at = now_ms();
        let duration_ms = (finished_at - started_at).max(0) as u64;
        let status_str = if result.ok { "success" } else { "error" };

        let mut structured = serde_json::Map::new();
        structured.insert("status".to_string(), serde_json::Value::String(status_str.to_string()));
        structured.insert("ok".to_string(), serde_json::Value::Bool(result.ok));
        structured.insert("duration_ms".to_string(), serde_json::json!(duration_ms));
        structured.insert("durationMs".to_string(), serde_json::json!(duration_ms));
        structured.insert("started_at".to_string(), serde_json::json!(started_at));
        structured.insert("startedAt".to_string(), serde_json::json!(started_at));
        structured.insert("finished_at".to_string(), serde_json::json!(finished_at));
        structured.insert("finishedAt".to_string(), serde_json::json!(finished_at));

        if let Ok(parsed_data) = serde_json::from_str::<Value>(&result.output) {
            structured.insert("data".to_string(), parsed_data);
        }
        structured.insert("output".to_string(), serde_json::Value::String(result.output.clone()));

        if let Some(ref det) = result.details {
            structured.insert("details".to_string(), det.clone());
        }

        let structured_content = serde_json::to_string_pretty(&Value::Object(structured))
            .unwrap_or_else(|_| result.output.clone());

        AgentMessage::ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: structured_content,
            is_error: if result.ok { None } else { Some(true) },
            details: result.details,
            patch: result.patch,
            checkpoint_id,
            timestamp: Some(finished_at),
            status: Some(status_str.to_string()),
            duration_ms: Some(duration_ms),
            started_at: Some(started_at),
            finished_at: Some(finished_at),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::PluginManager;

    #[tokio::test]
    async fn test_disabled_plugin_tool_cannot_be_executed() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_executor_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let pm = PluginManager::new();
        // 禁用内置插件 batch-ops
        let _ = pm.toggle_plugin("builtin:batch-ops", false);

        let call = ToolCallInfo {
            id: "call_batch_write_disabled".to_string(),
            name: "batch_write".to_string(),
            args: serde_json::json!({
                "files": [
                    { "path": "test.txt", "content": "hello" }
                ]
            }).to_string(),
        };

        // 执行工具调用
        let msg = execute_tool_call(&temp_dir, "thread_test", &call, None).await;

        // 恢复插件启用状态以防影响其他单测
        let _ = pm.toggle_plugin("builtin:batch-ops", true);
        let _ = std::fs::remove_dir_all(&temp_dir);

        // 验证由于插件被禁用而直接被拦截报错（M2-T5 验收点）
        match msg {
            AgentMessage::ToolResult { is_error, content, .. } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("已被禁用，无法执行工具 [batch_write]"), "实际内容: {}", content);
            }
            _ => panic!("预期返回 ToolResult"),
        }
    }
}

