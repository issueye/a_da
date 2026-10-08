use std::path::Path;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::approval::{global_question_manager, QuestionAnswer};
use crate::checkpoint::CheckpointManager;
use crate::runner::AgentLoopEvent;
use crate::tools::{read_file, run_command, write_file, edit_file, ToolResult};

/// 处理 ask_user 交互向用户提问工具
pub async fn execute_ask_user(
    call_id: &str,
    args: &Value,
    event_tx: Option<&mpsc::Sender<AgentLoopEvent>>,
    abort_rx: Option<&watch::Receiver<bool>>,
) -> ToolResult {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if question.is_empty() {
        return ToolResult::error("缺少 question 参数：要问用户什么？");
    }

    if question.chars().count() > 600 {
        return ToolResult::error("问题太长（上限 600 字）。请把背景压缩成几句后再问。");
    }

    let mut choices_list = Vec::new();
    let mut options_json = Vec::new();

    if let Some(arr) = args.get("choices").and_then(|v| v.as_array()) {
        for (idx, item) in arr.iter().enumerate().take(6) {
            if let Some(obj) = item.as_object() {
                let label = obj.get("label").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !label.is_empty() {
                    let id = obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("c{}", idx + 1));
                    let desc = obj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

                    options_json.push(serde_json::json!({
                        "value": id.clone(),
                        "label": label.to_string(),
                    }));

                    choices_list.push(crate::protocol::ChoiceOption {
                        id,
                        label: label.to_string(),
                        description: desc,
                    });
                }
            }
        }
    }

    let allow_text = if choices_list.is_empty() {
        true
    } else {
        args.get("allow_text").and_then(|v| v.as_bool()).unwrap_or(true)
    };

    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let question_val = serde_json::json!({
        "callId": call_id,
        "question": question,
        "choices": if choices_list.is_empty() { serde_json::Value::Null } else { serde_json::to_value(&choices_list).unwrap_or(serde_json::Value::Null) },
        "options": options_json,
        "allowText": allow_text,
        "status": "pending",
        "askedAt": now_ts,
    });

    // 1. 注册通道并等待答复
    let rx = global_question_manager().register_waiter(call_id);

    // 2. 发送 ToolAwaitingQuestion 事件，让 store 更新状态并即时广播到前端 UI
    if let Some(tx) = event_tx {
        let _ = tx
            .send(AgentLoopEvent::ToolAwaitingQuestion {
                id: call_id.to_string(),
                question: question_val,
            })
            .await;
    }

    // 3. 挂起等待前端用户的回答或会话中止信号
    let mut rx_abort = abort_rx.cloned();
    let answer = tokio::select! {
        ans = rx => {
            match ans {
                Ok(a) => a,
                Err(_) => QuestionAnswer { choice: None, text: None, answered_by: "aborted".to_string() },
            }
        }
        _ = async {
            if let Some(ref mut abr) = rx_abort {
                if *abr.borrow() {
                    return;
                }
                while abr.changed().await.is_ok() {
                    if *abr.borrow() {
                        break;
                    }
                }
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            global_question_manager().cancel(call_id);
            QuestionAnswer { choice: None, text: None, answered_by: "aborted".to_string() }
        }
    };

    // 4. 组装给模型上下文的自然语言答复
    let output = if answer.answered_by == "aborted" {
        "用户中止了这次运行，问题没有得到回答。不要再假设一个答案继续做——把需要确认的点说清楚，等用户回来再问一次。".to_string()
    } else {
        let picked = answer.choice.as_ref().and_then(|c_id| {
            choices_list.iter().find(|c| &c.id == c_id)
        });
        match (picked, answer.text.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty())) {
            (Some(c), Some(t)) => format!("用户选择了「{}」，并补充说明：{}", c.label, t),
            (Some(c), None) => format!("用户选择了「{}」", c.label),
            (None, Some(t)) => format!("用户回答：{}", t),
            (None, None) => "用户提交了空白确认。请结合已知信息继续。".to_string(),
        }
    };

    let ok = answer.answered_by != "aborted";
    ToolResult {
        output,
        ok,
        details: None,
        patch: None,
        terminate: None,
    }
}

/// 执行内置插件工具（彻底规避虚拟路径 (builtin):xxx 造成沙箱读盘失败）
pub async fn execute_builtin_plugin_tool(
    workspace: &Path,
    thread_id: &str,
    name: &str,
    args: &Value,
    checkpoint_mgr: Option<&std::sync::Arc<CheckpointManager>>,
) -> Option<ToolResult> {
    match name {
        "project_inspect" | "inspect_project" => {
            Some(execute_project_inspect(workspace).await)
        }
        "git_status" => {
            let res = run_command(workspace, "git status --porcelain=v1 -b", None, Some(10), None).await;
            if !res.ok && res.output.contains("not a git repository") {
                Some(ToolResult::error("当前目录不是一个有效的 Git 仓库。"))
            } else {
                Some(res)
            }
        }
        "git_diff" => {
            let file_opt = args.get("file").or_else(|| args.get("path")).and_then(|v| v.as_str());
            let cmd = match file_opt {
                Some(f) if !f.trim().is_empty() => format!("git diff -- {}", f.trim()),
                _ => "git diff".to_string(),
            };
            Some(run_command(workspace, &cmd, None, Some(15), None).await)
        }
        "git_log" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            let cmd = format!("git log -n {} --oneline", limit);
            Some(run_command(workspace, &cmd, None, Some(10), None).await)
        }
        "code_outline" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            Some(execute_code_outline(workspace, path).await)
        }
        "run_tests" => {
            let cmd = if workspace.join("Cargo.toml").exists() {
                "cargo test"
            } else if workspace.join("bun.lockb").exists() || workspace.join("bun.lock").exists() {
                "bun test"
            } else if workspace.join("package.json").exists() {
                "npm test"
            } else if workspace.join("pytest.ini").exists() || workspace.join("tests").exists() {
                "pytest"
            } else {
                "cargo test"
            };
            Some(run_command(workspace, cmd, None, Some(180), None).await)
        }
        "batch_write" => {
            let files = args.get("files").and_then(|v| v.as_array());
            if let Some(list) = files {
                // 拍摄批量写前检查点（M2-T3：批量写每个目标都有回滚记录）
                if let Some(mgr) = checkpoint_mgr {
                    let mut abs_paths = Vec::new();
                    for f in list {
                        if let Some(p) = f.get("path").and_then(|v| v.as_str()) {
                            if let Ok(abs) = crate::tools::check_workspace_sandbox(workspace, p) {
                                abs_paths.push((p, abs));
                            }
                        }
                    }
                    let targets: Vec<(&str, &Path)> = abs_paths.iter().map(|(p, abs)| (*p, abs.as_path())).collect();
                    if !targets.is_empty() {
                        let _ = mgr.capture(thread_id, "batch_write", &targets);
                    }
                }

                let mut written = 0;
                for f in list {
                    if let (Some(p), Some(c)) = (
                        f.get("path").and_then(|v| v.as_str()),
                        f.get("content").and_then(|v| v.as_str()),
                    ) {
                        let res = write_file(workspace, p, c);
                        if !res.ok {
                            return Some(ToolResult::error(format!("写入 [{}] 失败: {}", p, res.output)));
                        }
                        written += 1;
                    }
                }
                Some(ToolResult::success(format!("成功原子写入 {} 个文件。", written)))
            } else {
                Some(ToolResult::error("缺少 files 参数"))
            }
        }
        "batch_replace" => {
            let files = args.get("files").and_then(|v| v.as_array());
            let old_str = args.get("old_string").and_then(|v| v.as_str());
            let new_str = args.get("new_string").and_then(|v| v.as_str());
            if let (Some(list), Some(old_s), Some(new_s)) = (files, old_str, new_str) {
                // 拍摄批量替换前检查点（M2-T3：批量写每个目标都有回滚记录）
                if let Some(mgr) = checkpoint_mgr {
                    let mut abs_paths = Vec::new();
                    for f in list {
                        if let Some(p) = f.as_str() {
                            if let Ok(abs) = crate::tools::check_workspace_sandbox(workspace, p) {
                                abs_paths.push((p, abs));
                            }
                        }
                    }
                    let targets: Vec<(&str, &Path)> = abs_paths.iter().map(|(p, abs)| (*p, abs.as_path())).collect();
                    if !targets.is_empty() {
                        let _ = mgr.capture(thread_id, "batch_replace", &targets);
                    }
                }

                let mut replaced = 0;
                for f in list {
                    if let Some(p) = f.as_str() {
                        let res = edit_file(workspace, p, Some(old_s), Some(new_s), None);
                        if res.ok {
                            replaced += 1;
                        }
                    }
                }
                Some(ToolResult::success(format!("批量替换完成，在 {} 个文件中生效。", replaced)))
            } else {
                Some(ToolResult::error("缺少 files / old_string / new_string 参数"))
            }
        }
        "decide" => {
            let tool = crate::tools::decision::DecideTool::new();
            let receipt = tool.run(args).await;
            Some(ToolResult {
                output: receipt.output,
                ok: receipt.status == agent_base::domain::ToolStatus::Success,
                details: receipt.data.or(receipt.details),
                patch: None,
                terminate: None,
            })
        }
        "check_gate" => {
            let tool = crate::tools::decision::CheckGateTool::new(workspace);
            let receipt = tool.run(args).await;
            Some(ToolResult {
                output: receipt.output,
                ok: receipt.status == agent_base::domain::ToolStatus::Success,
                details: receipt.data.or(receipt.details),
                patch: None,
                terminate: None,
            })
        }
        // 空壳工具 evaluate_diff / manage_ponytail 已按计划 §1.3 下线
        _ => None,
    }
}

async fn execute_project_inspect(workspace: &Path) -> ToolResult {
    let mut sections = vec![format!("# 项目工程与环境诊断报告: {}", workspace.display())];

    let pkg_path = workspace.join("package.json");
    if pkg_path.exists() {
        if let Ok(raw) = tokio::fs::read_to_string(&pkg_path).await {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&raw) {
                sections.push("## Node / TypeScript 生态配置".to_string());
                let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
                let ver = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
                sections.push(format!("- **包名**: {} (v{})", name, ver));

                if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
                    sections.push("- **可用 Scripts 指令**:".to_string());
                    for (k, v) in scripts {
                        sections.push(format!("  - `{}`: {}", k, v.as_str().unwrap_or("")));
                    }
                }
                if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                    let keys: Vec<&str> = deps.keys().map(|s| s.as_str()).take(15).collect();
                    sections.push(format!("- **生产依赖**: {} 个 ({})", deps.len(), keys.join(", ")));
                }
                if let Some(dev) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
                    let keys: Vec<&str> = dev.keys().map(|s| s.as_str()).take(15).collect();
                    sections.push(format!("- **开发依赖**: {} 个 ({})", dev.len(), keys.join(", ")));
                }
            }
        }
    }

    let cargo_path = workspace.join("Cargo.toml");
    if cargo_path.exists() {
        sections.push("## Rust / Cargo 生态配置".to_string());
        if let Ok(raw) = tokio::fs::read_to_string(&cargo_path).await {
            if raw.contains("[workspace]") {
                sections.push("- **工程模式**: Cargo Workspace 多包工作区".to_string());
            }
            if let Some(line) = raw.lines().find(|l| l.trim().starts_with("name =")) {
                sections.push(format!("- **Crate 包名**: {}", line.trim()));
            }
        }
    }

    if workspace.join("pyproject.toml").exists() || workspace.join("requirements.txt").exists() {
        sections.push("## Python 生态配置".to_string());
        if workspace.join("pyproject.toml").exists() {
            sections.push("- 发现 pyproject.toml".to_string());
        }
        if workspace.join("requirements.txt").exists() {
            sections.push("- 发现 requirements.txt".to_string());
        }
    }

    if workspace.join("go.mod").exists() {
        sections.push("## Go 生态配置".to_string());
        sections.push("- 发现 go.mod".to_string());
    }

    ToolResult::success(sections.join("\n\n"))
}

async fn execute_code_outline(workspace: &Path, file_path: &str) -> ToolResult {
    let res = read_file(workspace, file_path, None, Some(2000));
    if !res.ok {
        return res;
    }

    let mut outlines = Vec::new();
    for (line_no, line) in res.output.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
            continue;
        }
        if trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("pub enum ")
            || trimmed.starts_with("enum ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("export const ")
        {
            outlines.push(format!("L{:03}: {}", line_no + 1, trimmed));
        }
    }

    if outlines.is_empty() {
        ToolResult::success(format!("文件 [{}] 未提取到显著类、函数或接口大纲。", file_path))
    } else {
        ToolResult::success(format!("## 代码大纲结构: {}\n```\n{}\n```", file_path, outlines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_project_inspect() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let res = execute_project_inspect(manifest_dir).await;
        assert!(res.ok);
        assert!(res.output.contains("项目工程与环境诊断报告"));
        assert!(res.output.contains("Cargo"));
    }

    #[tokio::test]
    async fn test_execute_ask_user_flow() {
        let (tx, mut rx) = mpsc::channel(16);
        let args = serde_json::json!({
            "question": "请确认重构计划？",
            "choices": [
                { "id": "opt1", "label": "方案 A：就地修改" },
                { "id": "opt2", "label": "方案 B：新建模块" }
            ],
            "allow_text": true
        });

        let call_id = "test_ask_001";
        let ask_task = tokio::spawn(async move {
            execute_ask_user(call_id, &args, Some(&tx), None).await
        });

        // 验证收到了 ToolAwaitingQuestion 事件
        let event = rx.recv().await.expect("应该收到 ToolAwaitingQuestion 事件");
        match event {
            AgentLoopEvent::ToolAwaitingQuestion { id, question } => {
                assert_eq!(id, "test_ask_001");
                assert_eq!(question.get("callId").and_then(|v| v.as_str()), Some("test_ask_001"));
                assert_eq!(question.get("question").and_then(|v| v.as_str()), Some("请确认重构计划？"));
            }
            _ => panic!("收到非预期的事件"),
        }

        // 模拟用户作答
        let resolved = global_question_manager().resolve_answer(
            call_id,
            QuestionAnswer {
                choice: Some("opt1".to_string()),
                text: Some("请保留旧接口兼容".to_string()),
                answered_by: "user".to_string(),
            },
        );
        assert!(resolved);

        let res = ask_task.await.expect("任务应该正常结束");
        assert!(res.ok);
        assert!(res.output.contains("用户选择了「方案 A：就地修改」"));
        assert!(res.output.contains("请保留旧接口兼容"));
    }

    #[tokio::test]
    async fn test_execute_ask_user_aborted() {
        let (abort_tx, abort_rx) = watch::channel(false);
        let args = serde_json::json!({
            "question": "测试中止问题？"
        });

        let call_id = "test_ask_abort";
        let ask_task = tokio::spawn(async move {
            execute_ask_user(call_id, &args, None, Some(&abort_rx)).await
        });

        // 稍等让其挂起并注册 waiter
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(global_question_manager().has_pending(call_id));

        // 发送中止信号
        let _ = abort_tx.send(true);

        let res = ask_task.await.expect("任务应该被正常唤醒退出");
        assert!(!res.ok);
        assert!(res.output.contains("用户中止了这次运行"));
        assert!(!global_question_manager().has_pending(call_id));
    }

    #[tokio::test]
    async fn test_batch_write_captures_checkpoint_and_empty_tools_offline() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_batch_cp_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let cp_dir = temp_dir.join("cp_storage");
        let cp_mgr = std::sync::Arc::new(CheckpointManager::new(Some(cp_dir)));
        let thread_id = "thread_batch_test";

        // 先预先创建一个文件以备覆盖拍摄
        let file_a = temp_dir.join("a.txt");
        std::fs::write(&file_a, "initial A").unwrap();

        let args = serde_json::json!({
            "files": [
                { "path": "a.txt", "content": "modified A" },
                { "path": "b.txt", "content": "created B" }
            ]
        });

        // 1. 执行 batch_write
        let res = execute_builtin_plugin_tool(&temp_dir, thread_id, "batch_write", &args, Some(&cp_mgr)).await;
        assert!(res.is_some());
        let tool_res = res.unwrap();
        assert!(tool_res.ok);
        assert!(tool_res.output.contains("成功原子写入 2 个文件"));

        // 2. 断言检查点已被成功拍摄且能被读取（M2-T3 核心验收）
        let loaded = cp_mgr.load(thread_id, true).expect("读取检查点成功");
        assert!(loaded.is_some());
        let entries = loaded.unwrap();
        assert_eq!(entries.len(), 1);
        if let crate::checkpoint::CheckpointEntry::Checkpoint(ref record) = entries[0] {
            assert_eq!(record.files.len(), 2);
            assert!(record.files.iter().any(|f| f.path == "a.txt" && f.existed));
            assert!(record.files.iter().any(|f| f.path == "b.txt" && !f.existed));
        } else {
            panic!("预期是 CheckpointEntry::Checkpoint");
        }

        // 3. 断言空壳工具下线（M2-T4：evaluate_diff / manage_ponytail 仍按下线处置）
        let dummy_args = serde_json::json!({});
        let diff_res = execute_builtin_plugin_tool(&temp_dir, thread_id, "evaluate_diff", &dummy_args, None).await;
        assert!(diff_res.is_none(), "evaluate_diff 空壳工具必须已下线返回 None");
        let ponytail_res = execute_builtin_plugin_tool(&temp_dir, thread_id, "manage_ponytail", &dummy_args, None).await;
        assert!(ponytail_res.is_none(), "manage_ponytail 空壳工具必须已下线返回 None");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_execute_decide_tool() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_decide_{}", uuid::Uuid::new_v4()));
        let args = serde_json::json!({
            "state": "本项目使用纯 Rust 原生开发，代码整洁，所有单元测试均已通过。",
            "questions": {
                "is_rust": {
                    "type": "noul",
                    "instructions": "该项目是否基于 Rust 语言实现？"
                },
                "stack": {
                    "type": "choice",
                    "instructions": "识别项目技术栈",
                    "criteria": {
                        "rust": "Rust 语言项目",
                        "python": "Python 语言项目"
                    }
                }
            }
        });

        let res = execute_builtin_plugin_tool(&temp_dir, "thread_decide", "decide", &args, None).await;
        assert!(res.is_some(), "decide 工具必须成功派发并执行");
        let tool_res = res.unwrap();
        assert!(tool_res.ok, "decide 评估应当执行成功");
        assert!(tool_res.output.contains("决策结果"));
        assert!(tool_res.output.contains("is_rust"));
        assert!(tool_res.output.contains("stack"));

        let details = tool_res.details.expect("必须返回 details 结构化结果");
        assert_eq!(details.get("engine").and_then(|v| v.as_str()), Some("heuristic"));
        // 断言 calibrated 绝不捏造确定性（AGENTS.md §9）
        let answers = details.get("answers").and_then(|v| v.as_object()).expect("包含 answers 映射");
        for (_k, ans) in answers {
            assert_eq!(ans.get("calibrated").and_then(|v| v.as_bool()), Some(false));
        }
    }

    #[tokio::test]
    async fn test_execute_check_gate_tool() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_gate_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);

        // 1. 测试成功通过的文本准入
        let pass_args = serde_json::json!({
            "criteria": "必须包含所有单元测试通过的字样",
            "source": "text",
            "text": "test result: ok. 15 passed; 0 failed; all tests green",
            "threshold": 0.6
        });
        let pass_res = execute_builtin_plugin_tool(&temp_dir, "thread_gate", "check_gate", &pass_args, None).await;
        assert!(pass_res.is_some(), "check_gate 工具必须成功派发");
        let pass_tool_res = pass_res.unwrap();
        assert!(pass_tool_res.ok);
        assert!(pass_tool_res.output.contains("通过 [PASS]"));
        let pass_details = pass_tool_res.details.expect("包含 details");
        assert_eq!(pass_details.get("passed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(pass_details.get("calibrated").and_then(|v| v.as_bool()), Some(false));

        // 2. 测试材料缺失下的 fail-close 默认拦截
        let fail_args = serde_json::json!({
            "criteria": "必须包含所有单元测试通过的字样",
            "source": "file",
            "file": "non_existent_result_file.txt",
            "threshold": 0.6
        });
        let fail_res = execute_builtin_plugin_tool(&temp_dir, "thread_gate", "check_gate", &fail_args, None).await;
        assert!(fail_res.is_some());
        let fail_tool_res = fail_res.unwrap();
        assert!(fail_tool_res.ok, "工具自身完成判定回执输出");
        assert!(fail_tool_res.output.contains("未通过 [BLOCKED]"));
        let fail_details = fail_tool_res.details.expect("包含 details");
        assert_eq!(fail_details.get("passed").and_then(|v| v.as_bool()), Some(false));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

