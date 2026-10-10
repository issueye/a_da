use std::collections::HashMap;
use std::sync::OnceLock;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolDescriptor,
};
use serde_json::json;

/// 获取所有已知内置工具的基准 ToolDescriptor 清单（真源）
pub fn standard_tool_descriptors() -> &'static [ToolDescriptor] {
    static DESCRIPTORS: OnceLock<Vec<ToolDescriptor>> = OnceLock::new();
    DESCRIPTORS.get_or_init(|| {
        vec![
            ToolDescriptor {
                name: "read_file".to_string(),
                summary: "安全读取工作区指定文件的全部或部分行内容。".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "write_file".to_string(),
                summary: "在工作区内全量覆写指定文件。".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "edit_file".to_string(),
                summary: "通过严格文本匹配替换文件中的指定片段。".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
                access: Access::Mutates { paths: PathSelector::Single("path") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::SingleTarget,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "list_files".to_string(),
                summary: "按层级列出工作区目录树中的文件与文件夹结构。".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "search_files".to_string(),
                summary: "在工作区文件内搜索匹配的字符串或正则表达式内容。".to_string(),
                schema: json!({ "type": "object", "properties": { "pattern": { "type": "string" } }, "required": ["pattern"] }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "batch_write".to_string(),
                summary: "在工作区内批量写入多个文件。".to_string(),
                schema: json!({ "type": "object", "properties": { "files": { "type": "array" } }, "required": ["files"] }),
                access: Access::Mutates { paths: PathSelector::Batch("files") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "batch_replace".to_string(),
                summary: "在多个目标文件中批量进行严格字符串替换。".to_string(),
                schema: json!({ "type": "object", "properties": { "files": { "type": "array" } }, "required": ["files"] }),
                access: Access::Mutates { paths: PathSelector::Batch("files") },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::PerTargetInBatch,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "run_command".to_string(),
                summary: "在系统底层 shell 中执行指定命令行。".to_string(),
                schema: json!({ "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] }),
                access: Access::Executes { command_arg: "command" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "run_background".to_string(),
                summary: "在后台运行长期指令。".to_string(),
                schema: json!({ "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] }),
                access: Access::Executes { command_arg: "command" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "ask_user".to_string(),
                summary: "在交互界面向用户发起单选选择或补充说明提问并阻塞等待答复。".to_string(),
                schema: json!({ "type": "object", "properties": { "question": { "type": "string" } }, "required": ["question"] }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "todo".to_string(),
                summary: "记录或查看当前会话的待办任务与进度清单。".to_string(),
                schema: json!({ "type": "object", "properties": { "action": { "type": "string" } } }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "finish".to_string(),
                summary: "声明已完成所有任务并结束当前回合。".to_string(),
                schema: json!({ "type": "object", "properties": { "summary": { "type": "string" } }, "required": ["summary"] }),
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
                schema: json!({ "type": "object", "properties": { "file": { "type": "string" } } }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "git_log".to_string(),
                summary: "查看工作区 Git 提交历史。".to_string(),
                schema: json!({ "type": "object", "properties": { "limit": { "type": "integer" } } }),
                access: Access::ReadOnly,
                approval: ApprovalPolicy::Never,
                rollback: RollbackPolicy::None,
                execution: Execution::ParallelSafe,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "code_outline".to_string(),
                summary: "解析代码文件结构大纲。".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
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
                // 不碰文件、不跑命令文本，但**确实有副作用**（终结进程树）。
                // `Access` 只有 ReadOnly / Mutates(文件) / Executes(命令) 三态，
                // 因此归到 Executes，字段名说明它取任务 id。
                access: Access::Executes { command_arg: "task_id" },
                approval: ApprovalPolicy::Named("approval-guard"),
                rollback: RollbackPolicy::None,
                execution: Execution::Sequential,
                termination: Termination::ContinueTurn,
            },
            ToolDescriptor {
                name: "decide".to_string(),
                summary: "对给定材料进行结构化类型决策（choice/noul/score）".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "state": { "description": "被判断的材料" },
                        "questions": { "type": "object", "description": "问题映射" }
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
                summary: "对工作区改动、指定文件或文本依据验收标准执行门禁判定（fail-close 安全默认）".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "criteria": { "type": "string", "description": "验收标准" }
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
