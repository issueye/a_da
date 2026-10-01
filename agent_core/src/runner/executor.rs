use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::ai::ToolCallInfo;
use crate::checkpoint::CheckpointManager;
use crate::session::AgentMessage;
use crate::tools::{
    check_workspace_sandbox, edit_file, list_files, read_file, run_command, search_files,
    write_file, EditPair, ToolResult,
};

/// 派发并执行单个工具调用
pub async fn execute_tool_call(
    workspace: &Path,
    thread_id: &str,
    call: &ToolCallInfo,
    checkpoint_mgr: Option<&Arc<CheckpointManager>>,
) -> AgentMessage {
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
        unknown => ToolResult::error(format!("未知工具: {}", unknown)),
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
