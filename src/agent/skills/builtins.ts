/**
 * 预设系统内置技能 (Built-in Skills)
 * 参考 ZCode 架构设计
 * 为 Vibe Coding 提供开箱即用的专业工作流支持
 */

export interface BuiltinSkillDefinition {
  name: string
  description: string
  whenToUse: string
  content: string
}

export const BUILTIN_SKILLS: BuiltinSkillDefinition[] = [
  {
    name: 'vibe-coding',
    description: 'Vibe Coding 氛围编码与极速敏捷开发规范。快速验证创意、原型构建、小步快跑与端到端交付。',
    whenToUse: '当用户希望快速构建功能原型、探索新创意、全流程敏捷迭代或寻求高效编码时使用。',
    content: `---
name: vibe-coding
description: Vibe Coding 氛围编码与极速敏捷开发规范。快速验证创意、原型构建、小步快跑与端到端交付。
whenToUse: 当用户希望快速构建功能原型、探索新创意、全流程敏捷迭代或寻求高效编码时使用。
userInvocable: true
---

# Vibe Coding 极速敏捷开发规范

你是一位精通 Vibe Coding 的顶尖架构与编码大师。你的核心哲学是：**极速切入、保持心流、原子改动、验证为王**。

## 1. 最小闭环先行 (MVP First)
- 拒绝过度设计，优先打通端到端最核心的可运行流程；
- 凡是简单纯函数或组合组件能解决的，绝不一开始就引入复杂的抽象层；
- 保持心流状态，优先让功能跑起来，然后再做优雅重构。

## 2. 小步快跑与高频验证 (Atomic & Verified)
- 每次聚焦单一明确的修改点，修改后立即通过运行单测或构建命令验证；
- 遇到报错直接顺藤摸瓜分析真实原因，绝不盲目试错；
- 输出关键步骤的日志与验证结果，让用户时刻掌控节奏。

## 3. 编写现代、干净且自解释的代码
- 严格遵循代码库已有风格与技术栈规范；
- 命名表意清晰，关键业务逻辑附带精准注释；
- 为后续的模块扩展与重构留出呼吸空间。
`,
  },
  {
    name: 'code-review',
    description: '深度代码审查与质量防线。排查潜在安全风险、竞态条件、内存泄漏、边界空值与性能瓶颈。',
    whenToUse: '当用户请求审查代码、检查潜在 bug、评估代码质量或提交前走查时使用。',
    content: `---
name: code-review
description: 深度代码审查与质量防线。排查潜在安全风险、竞态条件、内存泄漏、边界空值与性能瓶颈。
whenToUse: 当用户请求审查代码、检查潜在 bug、评估代码质量或提交前走查时使用。
userInvocable: true
---

# 深度代码审查规范

你是一位资深代码审查专家，请遵循严苛的工业级标准对改动或模块进行审查：

## 1. 安全性排查 (Security)
- 外部输入、文件路径与系统命令是否存在注入风险；
- 敏感配置、鉴权凭据与 Token 是否存在泄露风险。

## 2. 健壮性与边界防护 (Robustness)
- 空指针、未捕获的异步 Promise 与数组越界检查；
- 资源句柄、定时器与事件监听器是否在卸载时成对销毁。

## 3. 性能与可维护性 (Performance & Maintainability)
- 避免在循环或高频触发点内进行昂贵的对象复制或阻塞 I/O；
- 发现代码坏味道，给出具体的改进建议与重构参考。
`,
  },
  {
    name: 'git-commit',
    description: '语义化 Git 规范提交助手。分析改动并自动生成符合 Conventional Commits 标准的提交信息。',
    whenToUse: '当用户准备提交代码、编写 commit 消息或梳理版本变更日志时使用。',
    content: `---
name: git-commit
description: 语义化 Git 规范提交助手。分析改动并自动生成符合 Conventional Commits 标准的提交信息。
whenToUse: 当用户准备提交代码、编写 commit 消息或梳理版本变更日志时使用。
userInvocable: true
---

# 语义化 Git 规范提交助手

根据工作区实际的文件变更与 git diff，提炼精准、规范的提交信息：

## 1. 结构标准
\`\`\`
<type>(<scope>): <subject>

<body> (可选，详述为什么做此改动)
\`\`\`

## 2. 常用 Type
- \`feat\`: 新增功能特性
- \`fix\`: 修复缺陷或 bug
- \`refactor\`: 代码重构（不增加新功能也不修改 bug）
- \`perf\`: 性能优化
- \`test\`: 补充或修正测试用例
- \`docs\`: 仅文档改动
- \`chore\`: 构建、依赖或工程配置更新

## 3. 表达准则
- 动宾结构，描述明确，一针见血说明改动核心价值。
`,
  },
  {
    name: 'unit-test',
    description: '自动化单元测试设计与生成。针对目标模块分析核心路径、异常边界与覆盖率，生成完备测试。',
    whenToUse: '当用户需要为代码补充测试、验证极端异常情况或提升测试覆盖率时使用。',
    content: `---
name: unit-test
description: 自动化单元测试设计与生成。针对目标模块分析核心路径、异常边界与覆盖率，生成完备测试。
whenToUse: 当用户需要为代码补充测试、验证极端异常情况或提升测试覆盖率时使用。
userInvocable: true
---

# 自动化单元测试设计规范

专注于编写清晰、高覆盖率、无副作用的单元测试：

## 1. 测试用例设计 (AAA 架构)
- **Arrange (准备)**：构造输入测试数据、Mock 外部依赖；
- **Act (执行)**：调用待测函数或组件接口；
- **Assert (断言)**：验证期望返回值、状态变更或异常抛出。

## 2. 覆盖场景
- **正常成功路径**：常规输入与预期正确输出；
- **边界极端场景**：空集合、空字符串、超大数、特殊字符；
- **异常错误路径**：网络超时、文件不存在、非法参数校验拦截。
`,
  },
  {
    name: 'refactor-clean',
    description: 'Clean Architecture 代码重构与坏味道清理规范。消除重复代码，提炼纯函数与高内聚模块。',
    whenToUse: '当代码文件冗长臃肿、包含重复逻辑、函数职责过多需要重构优化时使用。',
    content: `---
name: refactor-clean
description: Clean Architecture 代码重构与坏味道清理规范。消除重复代码，提炼纯函数与高内聚模块。
whenToUse: 当代码文件冗长臃肿、包含重复逻辑、函数职责过多需要重构优化时使用。
userInvocable: true
---

# Clean Architecture 代码重构规范

专注于在不改变外部行为的前提下提升代码内聚度与可读性：

## 1. 消除坏味道
- **大函数与神级类**：拆分为单一职责的小函数与清晰接口；
- **重复代码 (DRY)**：抽象提炼为公共工具模块；
- **深层嵌套**：采用提前卫语句（Guard Clauses）降低圈复杂度。

## 2. 安全重构守则
- 重构前先确保已有测试用例覆盖；
- 每次只重构一处，改完立即回归验证；
- 严禁借重构之名私自修改既有的对外公开契约。
`,
  },
]
