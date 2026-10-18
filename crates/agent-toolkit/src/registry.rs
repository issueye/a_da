use std::collections::HashMap;
use std::sync::OnceLock;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolDescriptor,
};
use serde_json::json;

/// **合法地长时间运行 / 阻塞等待外部输入**的内置工具（真源）。
///
/// 引擎的策略超时（产品声明 `policies.toolTimeoutSec`，ada-coding 是 120 秒）对**不在此列**
/// 的工具生效；在列的这几个要么在等用户（审批/提问，上限 30 分钟）、要么在等子智能体跑完、
/// 要么自带超时（命令执行最长 10 分钟）——拿 120 秒去掐它们只会制造假失败。
///
/// 这份名单是**唯一真源**：每个工具的 `Tool::long_running()` 必须与它一致，
/// `test_long_running_set_matches_tool_declarations` 机械守门（漂移就红）。
pub const LONG_RUNNING_TOOLS: &[&str] = &[
    // 等用户点"批准/拒绝"（工具名是 `check_gate`，实现文件叫 gate_tool.rs）
    "check_gate",
    // 等用户回答问题
    "ask_user",
    // 等子智能体跑完（分钟级）
    "invoke_subagent",
    // 命令执行自带超时（默认 120s、上限 600s），不该被策略超时二次截断
    "run_command",
    // 决策插件的判定等待（TS 插件执行，最长按插件自己的超时）
    "decide",
];

/// 是否属于上面那份豁免名单。
pub fn is_long_running(name: &str) -> bool {
    LONG_RUNNING_TOOLS.contains(&name)
}

/// 获取所有已知内置工具的基准 ToolDescriptor 清单（真源）
pub fn standard_tool_descriptors() -> &'static [ToolDescriptor] {
    static DESCRIPTORS: OnceLock<Vec<ToolDescriptor>> = OnceLock::new();
    DESCRIPTORS.get_or_init(|| {
        vec![
            ToolDescriptor {
                name: "read_file".to_string(),
                summary: "安全读取工作区指定文件的全部或部分行内容。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "要读取的文件相对路径" },
                        "offset": { "type": "integer", "description": "起始行号（从 1 开始，可选）" },
                        "limit": { "type": "integer", "description": "最大读取行数（可选）" }
                    },
                    "required": ["path"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "write_file".to_string(),
                summary: "在工作区内全量覆写指定文件，若文件不存在则自动创建。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "要写入的目标文件相对路径" },
                        "content": { "type": "string", "description": "要写入的完整文本内容。覆写已存在文件时换行会跟随该文件原有约定" }
                    },
                    "required": ["path", "content"]
                }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "edit_file".to_string(),
                summary: "通过文本匹配替换文件中的指定片段，支持单块与多块替换（换行 CRLF/LF 自动归一化）。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "要编辑的目标文件相对路径" },
                        "old_string": { "type": "string", "description": "被替换的原始内容块（单块替换模式）。换行写 LF 或 CRLF 均可，工具会按文件实际换行归一化匹配" },
                        "new_string": { "type": "string", "description": "替换后的新内容块（单块替换模式）。写回时换行会还原成文件原有约定" },
                        "edits": {
                            "type": "array",
                            "description": "多处批量替换列表（批量模式）",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_string": { "type": "string" },
                                    "new_string": { "type": "string" }
                                },
                                "required": ["old_string", "new_string"]
                            }
                        }
                    },
                    "required": ["path"]
                }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "list_files".to_string(),
                summary: "按层级列出工作区目录树中的文件与文件夹结构。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "要列出的目录相对路径（默认为工作区根目录）" },
                        "depth": { "type": "integer", "description": "遍历最大深度（默认 3 层）" }
                    }
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "search_files".to_string(),
                summary: "在工作区文件内搜索匹配的字符串或正则表达式内容。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "搜索文本或正则表达式模式" },
                        "glob": { "type": "string", "description": "文件通配符模式（例如 **/*.rs，可选）" },
                        "path": { "type": "string", "description": "限制搜索的子目录相对路径（可选）" },
                        "literal": { "type": "boolean", "description": "是否进行纯字面量匹配（默认 false）" },
                        "case_sensitive": { "type": "boolean", "description": "是否区分大小写（默认 false）" },
                        "context": { "type": "integer", "description": "匹配行前后的上下文行数（默认 0）" }
                    },
                    "required": ["pattern"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "batch_write".to_string(),
                summary: "在工作区内批量写入多个文件。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "description": "待写入的文件列表",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "相对路径" },
                                    "content": { "type": "string", "description": "文本内容" }
                                },
                                "required": ["path", "content"]
                            }
                        }
                    },
                    "required": ["files"]
                }),
                access: Access::Mutates { paths: PathSelector::Batch("files") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "batch_replace".to_string(),
                summary: "在多个目标文件中批量进行严格字符串替换。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "description": "待替换的目标文件路径列表",
                            "items": { "type": "string" }
                        },
                        "old_string": { "type": "string", "description": "被替换的原字符串" },
                        "new_string": { "type": "string", "description": "替换后的新字符串" }
                    },
                    "required": ["files", "old_string", "new_string"]
                }),
                access: Access::Mutates { paths: PathSelector::Batch("files") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "run_command".to_string(),
                summary: "在系统底层 shell 中执行指定命令行，并捕获标准输出、错误与退出状态。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的命令行指令字符串" },
                        "cwd": { "type": "string", "description": "子目录路径（相对工作区根目录，可选）" },
                        "timeout": { "type": "integer", "description": "命令超时等待时间（秒，默认 30）" }
                    },
                    "required": ["command"]
                }),
                access: Access::Executes { command_arg: "command" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "run_background".to_string(),
                summary: "后台启动长运行命令（dev server、watcher 等），立即返回任务 id。用 check_task 看输出、kill_task 终止。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要后台执行的命令行指令" },
                        "cwd": { "type": "string", "description": "子目录路径（相对工作区根目录，可选）" }
                    },
                    "required": ["command"]
                }),
                access: Access::Executes { command_arg: "command" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "ask_user".to_string(),
                summary: "在交互界面向用户发起选择（单选/多选）或补充说明提问并阻塞等待答复。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "向用户提出的问题文本。**要一次问多个问题请改用 `questions`**（一次问完，用户逐题作答后一起提交）"
                        },
                        "choices": {
                            "type": "array",
                            "description": "供用户选择的选项列表（可选，不传则仅为文本输入）",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "选项唯一标识（可选，如 c1、c2，回传时使用）" },
                                    "label": { "type": "string", "description": "显示给用户的选项文案" },
                                    "description": { "type": "string", "description": "选项说明或辅助解释" }
                                },
                                "required": ["label"]
                            }
                        },
                        "multi_select": {
                            "type": "boolean",
                            "description": "是否允许多选：需要用户同时勾选多个选项时设为 true（默认 false，单选点一下即答）"
                        },
                        "allow_text": {
                            "type": "boolean",
                            "description": "是否允许用户输入自定义文本（默认为 true）"
                        },
                        "questions": {
                            "type": "array",
                            "description": "一次向用户提出**多个**问题（最多 10 题）。给了它就忽略 question/choices/multi_select/allow_text：界面把每道题分别渲染出来，用户逐题作答后一次提交。适合'发布前一次确认几个约束'这类同批问题。",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "题号（可选，如 q1、q2；不填自动生成）" },
                                    "question": { "type": "string", "description": "该题的题目文本" },
                                    "choices": {
                                        "type": "array",
                                        "description": "该题的选项列表（可选）",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "id": { "type": "string", "description": "选项唯一标识（可选，如 c1、c2）" },
                                                "label": { "type": "string", "description": "显示给用户的选项文案" },
                                                "description": { "type": "string", "description": "选项说明或辅助解释" }
                                            },
                                            "required": ["label"]
                                        }
                                    },
                                    "multi_select": { "type": "boolean", "description": "该题是否多选（默认 false）" },
                                    "allow_text": { "type": "boolean", "description": "该题是否允许文本补充（默认 true）" }
                                },
                                "required": ["question"]
                            }
                        }
                    },
                    "anyOf": [
                        { "required": ["question"] },
                        { "required": ["questions"] }
                    ]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "todo".to_string(),
                summary: "记录或查看当前会话的待办任务与进度清单。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["get", "set", "update"],
                            "description": "操作类型：get (获取待办), set (重置待办), update (更新项)"
                        },
                        "items": {
                            "type": "array",
                            "description": "待办项列表",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "title": { "type": "string" },
                                    "done": { "type": "boolean" }
                                },
                                "required": ["title"]
                            }
                        }
                    }
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "finish".to_string(),
                summary: "声明已完成所有任务并结束当前回合。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string", "description": "本轮已完成的工作总结与最终结果" }
                    },
                    "required": ["summary"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::EndTurn,
            },
            ToolDescriptor {
                name: "project_inspect".to_string(),
                summary: "项目工程与环境诊断报告。".to_string(),
                schema: json!({ "type": "object" }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "inspect_project".to_string(),
                summary: "项目工程与环境诊断报告（别名）。".to_string(),
                schema: json!({ "type": "object" }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "git_status".to_string(),
                summary: "查看工作区 Git 仓库工作区状态。".to_string(),
                schema: json!({ "type": "object" }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "git_diff".to_string(),
                summary: "查看工作区 Git diff 变更。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "查看特定文件的变更（可选，支持 path 别名）" },
                        "path": { "type": "string", "description": "查看特定文件的变更（别名）" }
                    }
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "git_log".to_string(),
                summary: "查看工作区 Git 提交历史。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "返回提交记录上限（默认 10）" }
                    }
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "code_outline".to_string(),
                summary: "解析代码文件结构大纲。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "要提取大纲的代码文件相对路径" }
                    },
                    "required": ["path"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "run_tests".to_string(),
                summary: "自动探测并执行项目单元测试。".to_string(),
                schema: json!({ "type": "object" }),
                access: Access::Executes { command_arg: "command" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "check_task".to_string(),
                summary: "查询后台任务的状态与输出（含退出码）。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": { "task_id": { "type": "string", "description": "run_background 返回的任务 id" } },
                    "required": ["task_id"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "kill_task".to_string(),
                summary: "终止后台任务及其整棵子进程树。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": { "task_id": { "type": "string", "description": "要终止的后台任务 id" } },
                    "required": ["task_id"]
                }),
                access: Access::Executes { command_arg: "task_id" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "decide".to_string(),
                summary: "对给定材料进行结构化判断，支持 choice（多选一）、noul（是否概率）与 score（档位评分）三种类型。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "state": {
                            "description": "被判断的材料（文本或结构化对象），必须自包含",
                            "type": ["string", "object"]
                        },
                        "questions": {
                            "type": "object",
                            "description": "问题映射：id -> 问题定义。包含 type（choice/noul/score）、instructions 与可选的 criteria",
                            "additionalProperties": {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "enum": ["choice", "noul", "score"] },
                                    "instructions": { "type": "string" },
                                    "criteria": { "description": "choice 为对象，score 为数组，noul 无需传递" }
                                },
                                "required": ["type", "instructions"]
                            }
                        },
                        "threshold": {
                            "type": "number",
                            "description": "覆盖默认判定阈值（默认 0.65）"
                        }
                    },
                    "required": ["state", "questions"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "check_gate".to_string(),
                summary: "对当前工作区改动、指定文件或文本依据验收标准执行门禁判定（fail-close 安全默认）。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "criteria": {
                            "type": "string",
                            "description": "具体的验收标准，如「所有新增函数必须包含单元测试」"
                        },
                        "source": {
                            "type": "string",
                            "enum": ["diff", "file", "text"],
                            "description": "门禁材料来源，默认为 diff（当前 git 未提交改动）"
                        },
                        "file": {
                            "type": "string",
                            "description": "当 source 为 file 时指定的工作区文件相对路径"
                        },
                        "text": {
                            "type": "string",
                            "description": "当 source 为 text 时直接传入的文本内容"
                        },
                        "threshold": {
                            "type": "number",
                            "description": "门禁通过阈值（0.0 ~ 1.0，默认 0.7）"
                        },
                        "fail_open": {
                            "type": "boolean",
                            "description": "引擎不可用时是否放行（默认 false，fail-close 安全默认）"
                        }
                    },
                    "required": ["criteria"]
                }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "invoke_subagent".to_string(),
                summary: "把专项任务委派给隔离运行的子智能体，完成后取回结论摘要。".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "subagent_id": {
                            "type": "string",
                            "description": "子智能体配置 id（如 general_purpose / researcher / code_reviewer / tester）"
                        },
                        "task": { "type": "string", "description": "委派的任务描述" },
                        "workspace": { "type": "string", "description": "目标工作区绝对路径（可选，跨工程委派时指定）" },
                        "additional_context": { "type": "string", "description": "额外上下文（可选）" }
                    },
                    "required": ["subagent_id", "task"]
                }),
                // 它不碰文件、不跑命令文本，但**确实有副作用**（会驱动一个能写文件的子智能体）。
                // `Access` 只有 ReadOnly / Mutates(文件) / Executes(命令) 三态，
                // 因此按"驱动一段外部执行"归到 Executes，字段名说明它取的是任务文本。
                // 失败安全（AGENTS.md §2）：绝不能标成 ReadOnly，否则只读档/plan 模式会放行它。
                access: Access::Executes { command_arg: "task" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
        ]
    })
}

/// 根据工具名称查找标准描述符
pub fn find_tool_descriptor(name: &str) -> Option<&'static ToolDescriptor> {
    static MAP: OnceLock<HashMap<&'static str, &'static ToolDescriptor>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for desc in standard_tool_descriptors() {
            m.insert(desc.name.as_str(), desc);
        }
        m
    });
    map.get(name).copied()
}

/// 基于 ToolDescriptor 的权威只读性检查（INV-3：单一真源）
pub fn is_readonly_tool(tool_name: &str) -> bool {
    if let Some(desc) = find_tool_descriptor(tool_name) {
        desc.is_readonly()
    } else {
        // 失败安全原则（AGENTS.md §2）：未知工具一律视为写工具
        false
    }
}

/// 基于 ToolDescriptor 的权威写操作检查（INV-3：单一真源）
pub fn is_write_tool(tool_name: &str) -> bool {
    !is_readonly_tool(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_standard_tool_descriptors_are_valid_and_unique() {
        let list = standard_tool_descriptors();
        assert_eq!(list.len(), 24, "必须声明全部 24 个标准内置工具描述符");

        let mut seen = HashSet::new();
        for desc in list {
            assert!(!desc.name.trim().is_empty(), "工具名称不能为空");
            assert!(
                seen.insert(desc.name.clone()),
                "发现重复声明的工具: {}",
                desc.name
            );
            assert!(
                !desc.summary.trim().is_empty(),
                "工具 `{}` 的 summary 不能为空",
                desc.name
            );
            assert!(
                desc.schema.is_object(),
                "工具 `{}` 的 schema 必须是 JSON Object",
                desc.name
            );
            assert_eq!(
                desc.schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "工具 `{}` 的 schema type 必须为 object",
                desc.name
            );

            // 必须能被 find_tool_descriptor 查到
            let found = find_tool_descriptor(&desc.name);
            assert!(found.is_some(), "必须能查到 `{}`", desc.name);
            assert_eq!(found.unwrap().name, desc.name);
        }
    }

    #[test]
    fn test_is_readonly_and_is_write_tool_fail_safe() {
        // 已知只读工具
        assert!(is_readonly_tool("read_file"));
        assert!(!is_write_tool("read_file"));

        // 已知写工具
        assert!(is_write_tool("write_file"));
        assert!(!is_readonly_tool("write_file"));

        // 失败安全：未知工具一律视为写工具（AGENTS.md §2）
        assert!(!is_readonly_tool("some_unknown_tool"));
        assert!(is_write_tool("some_unknown_tool"));
    }

    /// 契约：**豁免名单是唯一真源**——每个工具自述的 `long_running()` 必须与
    /// [`LONG_RUNNING_TOOLS`] 一致。改了名单不改实现（或反过来）都会在这里红。
    ///
    /// 为什么必须有这条：引擎的策略超时（120 秒）拿"等用户点批准"和"跑长命令"没办法，
    /// 名单漏一个就会把它们掐成假失败；名单多一个则会让真正会挂的工具失去兜底。
    #[test]
    fn test_long_running_set_matches_tool_declarations() {
        let ws = std::env::temp_dir();
        let names: Vec<String> = crate::toolkits::TOOLKIT_NAMES.iter().map(|s| s.to_string()).collect();
        let tools = crate::toolkits::tools_for_toolkits(&names, &ws).expect("工具包应可装配");
        assert!(!tools.is_empty(), "至少要能装配出内置工具");

        let mut provided: HashSet<String> = HashSet::new();
        for t in &tools {
            let n = t.descriptor().name.clone();
            assert_eq!(
                t.long_running(),
                is_long_running(&n),
                "工具 `{n}` 的 long_running() 与 registry::LONG_RUNNING_TOOLS 不一致"
            );
            provided.insert(n);
        }

        // 豁免名单里的名字必须**要么**由工具包提供、**要么**是别的 crate 提供的已登记项
        // （`invoke_subagent` 在 agent-node，按依赖方向不能被 agent-toolkit 实例化）
        for name in LONG_RUNNING_TOOLS {
            assert!(
                provided.contains(*name) || *name == "invoke_subagent",
                "豁免名单里的 `{name}` 既没有实现、也不在已知的跨 crate 项里"
            );
        }
    }
}
