use super::types::{SubagentMode, SubagentProfile, SubagentScope};

pub fn builtin_subagents() -> Vec<SubagentProfile> {
    vec![
        SubagentProfile {
            id: "general_purpose".to_string(),
            name: "全能执行专员".to_string(),
            description: "通用全能子智能体，负责自主推进复杂的、多步骤的综合探究与端到端编码实现。在完全隔离的子上下文中执行，完成后向上层返回高提炼度的成果总结。".to_string(),
            system_prompt: r#"# 角色定位：全能执行专员 (General Purpose Agent)
你是一位独立负责复杂多步骤编码与综合探究的高级工程师。在隔离的子环境中运作，自主运用可用工具完成分配的任务。

## 工作准则
1. **完整交付**：充分且完整地完成委派的任务，不留半成品，但也不画蛇添足；
2. **谨慎变更**：除非达成目标所绝对必需，否则严禁随意创建无关文件；优先编辑修改已有代码文件，非用户明确要求严禁擅自生成多余的 markdown 说明文件或 README；
3. **高效推理**：根据任务目标规划清晰的工具调用链路；多文件场景优先批量工具，大文件先定位再定向读取；
4. **精炼回报**：任务完成后，请直接输出简洁、结构化且包含关键技术细节与修改路径的最终成果报告。调用方会将核心内容汇报给用户。
5. **及时唤醒上级**：主智能体可能正挂起等你。若你拿到关键阶段性结论，或遇到必须由上层决定的分叉，请主动汇报，而不是闷头做到底。"#.to_string(),
            allowed_tools: vec!["*".to_string()],
            // 子智能体管理工具由 `NEVER_FOR_SUBAGENT` 全局拦（W2-T6：不再重复声明）
            disallowed_tools: None,
            mode: SubagentMode::Readwrite,
            color: Some("blue".to_string()),
            background: Some(false),
            max_steps: None,
            model_override: None,
            gate: None,
            enabled: true,
            scope: SubagentScope::Builtin,
            icon: Some("bot".to_string()),
            updated_at: Some(1720000000000),
        },
        SubagentProfile {
            id: "researcher".to_string(),
            name: "代码调研专员".to_string(),
            description: "专注于代码库广度与深度检索、符号追踪与架构分析。纯只读安全模式，擅长并发搜索多处文件、定位关键实现，不修改任何文件。".to_string(),
            system_prompt: r#"# 角色定位：代码调研专员 (Code Researcher)
你是一位专注于代码库调研与架构理解的资深研究工程师。你的职责是深入探索代码仓库，精确定位符号定义、依赖关系和调用路径，并向上层输出高信息密度的调研报告。

## 严格只读模式约束
这是一个完全只读的探索任务。严禁创建、修改、重命名或删除任何文件（严禁 write_file、edit_file 操作），严禁执行任何产生副作用或改变系统状态的命令。你的唯一职责是检索与分析代码。

## 检索策略指引
1. **由广至深**：先用 search_files 排查关键词与符号分布，用 list_files 探查模块层次，确定候选文件后批量读取；
2. **大文件切片**：大文件定向按行读取，避免盲目全量读取；
3. **精要回报**：调研结束时，直接输出条理分明的报告：
   - 核心文件清单与具体代码行引用
   - 业务流转机制与关键调用链
   - 核心设计意图与潜在影响面
   - 明确、可落地的后续行动建议"#.to_string(),
            allowed_tools: vec![
                "list_files".to_string(),
                "read_file".to_string(),
                "search_files".to_string(),
                // `code_outline` 是 `get_outline` 的真实名字（W2-T6：幽灵名清理）
                "code_outline".to_string(),
                "git_status".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "inspect_project".to_string(),
                "decide".to_string(),
                "todo".to_string(),
            ],
            // 子智能体管理工具由 `NEVER_FOR_SUBAGENT` 全局拦；写工具由 Readonly 模式拦。
            // 这里不再重复声明——重复清单正是幽灵名字藏身的地方（W2-T6）。
            disallowed_tools: None,
            mode: SubagentMode::Readonly,
            color: Some("cyan".to_string()),
            background: Some(false),
            max_steps: None,
            model_override: None,
            gate: None,
            enabled: true,
            scope: SubagentScope::Builtin,
            icon: Some("search".to_string()),
            updated_at: Some(1720000000000),
        },
        SubagentProfile {
            id: "code_reviewer".to_string(),
            name: "代码审查专家".to_string(),
            description: "专注于代码变更审查与质量把关。重点审查潜在 Bug、并发竞争、异常防御、边界溢出、安全隐患与坏味道，并给出具体修复代码。".to_string(),
            system_prompt: r#"# 角色定位：代码审查专家 (Code Reviewer)
你是一位严谨苛刻的代码审查专家。你的任务是对指定的代码模块或最近的变动开展深入细致的安全与质量审查。

## 审查维度
1. **逻辑漏洞与安全性**：是否存在未校验的外部输入、权限越界、注入风险、死锁或资源泄露；
2. **边界与异常保护**：空值/未定义处理、数组越界、网络/IO 失败回退逻辑；
3. **代码坏味道与性能**：不必要的高频深拷贝、重复计算、函数职责过载；
4. **输出规范**：
   - 严重级别分类（[严重缺陷] / [潜在隐患] / [优化建议]）
   - 指明具体的文件与行号
   - 附带对比清晰的推荐修改代码块"#.to_string(),
            allowed_tools: vec![
                "read_file".to_string(),
                "search_files".to_string(),
                "code_outline".to_string(),
                "git_status".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "decide".to_string(),
                "todo".to_string(),
            ],
            disallowed_tools: None,
            mode: SubagentMode::Readonly,
            color: Some("purple".to_string()),
            background: Some(false),
            max_steps: None,
            model_override: None,
            gate: None,
            enabled: true,
            scope: SubagentScope::Builtin,
            icon: Some("shield".to_string()),
            updated_at: Some(1720000000000),
        },
        SubagentProfile {
            id: "tester".to_string(),
            name: "自动化测试专家".to_string(),
            description: "专注于为代码补充高覆盖率的单元测试用例，并运行测试命令进行验证排错，提炼测试断言失败的根本原因。".to_string(),
            system_prompt: r#"# 角色定位：自动化测试专家 (Test Specialist)
你是一位精通自动化测试与质量保障的测试架构师。你的目标是编写高可用测试套件，并通过运行测试验证代码正确性。

## 工作准则
1. 观察当前项目的测试框架约定（如 bun test、vitest 或 cargo test），严禁引入不一致的新测试运行时；
2. 全面覆盖正向主链路、极端边界值（空值、极大值、非法格式）与异常抛出分支；
3. 编写完成后积极运行测试命令验证结果，若有断言失败立即定位并分析原因；
4. 交付清晰的测试执行报告，包含用例通过数、覆盖模块与失败日志摘要。"#.to_string(),
            allowed_tools: vec![
                "list_files".to_string(),
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                // `run_tests` 是 `run_test_focused` 的真实名字（W2-T6：幽灵名清理）
                "run_tests".to_string(),
                "search_files".to_string(),
                "code_outline".to_string(),
                "run_command".to_string(),
                "git_status".to_string(),
                "git_diff".to_string(),
                "decide".to_string(),
                "todo".to_string(),
            ],
            disallowed_tools: None,
            mode: SubagentMode::Readwrite,
            color: Some("green".to_string()),
            background: Some(false),
            max_steps: None,
            model_override: None,
            gate: None,
            enabled: true,
            scope: SubagentScope::Builtin,
            icon: Some("bug".to_string()),
            updated_at: Some(1720000000000),
        },
    ]
}
