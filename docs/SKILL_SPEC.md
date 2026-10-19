> ## Documentation Index
> Fetch the complete documentation index at: https://agentskills.zhcn.dev/llms.txt
> Use this file to discover all available pages before exploring further.

# 规范

> Agent Skills 的完整格式规范。

本文档定义了 Agent Skills 格式。

## 目录结构

一个 Skill 是一个至少包含 `SKILL.md` 文件的目录：

```
skill-name/
└── SKILL.md          # 必需
```

<Tip>
  你可以可选地包含[额外目录](#可选目录)，如 `scripts/`、`references/` 和 `assets/` 来支持你的 Skill。
</Tip>

## SKILL.md 格式

`SKILL.md` 文件必须包含 YAML Frontmatter，后接 Markdown 内容。

### Frontmatter (必需)

```yaml theme={null}
---
name: skill-name
description: A description of what this skill does and when to use it.
---
```

包含可选字段：

```yaml theme={null}
---
name: pdf-processing
description: Extract text and tables from PDF files, fill forms, merge documents.
license: Apache-2.0
metadata:
  author: example-org
  version: "1.0"
---
```

| 字段 | 是否必需 | 约束 |
| - | - | - |
| `name` | 是 | 最多 64 个字符。仅限小写字母、数字和连字符。不能以连字符开头或结尾。 |
| `description` | 是 | 最多 1024 个字符。不能为空。描述 Skill 的功能及使用时机。 |
| `license` | 否 | 许可证名称或对捆绑许可证文件的引用。 |
| `compatibility` | 否 | 最多 500 个字符。指示环境要求（目标产品、系统包、网络访问等）。 |
| `metadata` | 否 | 用于额外元数据的任意键值映射。 |
| `allowed-tools` | 否 | 预先批准的 Skill 可能使用的工具列表（以空格分隔）。（实验性） |

#### `name` 字段

必需的 `name` 字段：

* 必须为 1-64 个字符
* 只能包含 Unicode 小写字母数字字符和连字符（`a-z` 和 `-`）
* 不能以 `-` 开头或结尾
* 不能包含连续的连字符 (`--`)
* 必须与父目录名称匹配

有效示例：

```yaml theme={null}
name: pdf-processing
```

```yaml theme={null}
name: data-analysis
```

```yaml theme={null}
name: code-review
```

无效示例：

```yaml theme={null}
name: PDF-Processing  # 不允许大写字母
```

```yaml theme={null}
name: -pdf  # 不能以连字符开头
```

```yaml theme={null}
name: pdf--processing  # 不允许连续连字符
```

#### `description` 字段

必需的 `description` 字段：

* 必须为 1-1024 个字符
* 应描述 Skill 的功能及使用时机
* 应包含有助于 Agent 识别相关任务的特定关键词

优秀示例：

```yaml theme={null}
description: Extracts text and tables from PDF files, fills PDF forms, and merges multiple PDFs. Use when working with PDF documents or when the user mentions PDFs, forms, or document extraction.
```

不佳示例：

```yaml theme={null}
description: Helps with PDFs.
```

#### `license` 字段

可选的 `license` 字段：

* 指定应用于 Skill 的许可证
* 我们建议保持简短（许可证名称或捆绑许可证文件的名称）

示例：

```yaml theme={null}
license: Proprietary. LICENSE.txt has complete terms
```

#### `compatibility` 字段

可选的 `compatibility` 字段：

* 如果提供，必须为 1-500 个字符
* 仅当你的 Skill 有特定的环境要求时才应包含
* 可以指示目标产品、所需的系统包、网络访问需求等

示例：

```yaml theme={null}
compatibility: Designed for Claude Code (or similar products)
```

```yaml theme={null}
compatibility: Requires git, docker, jq, and access to the internet
```

<Note>
  大多数 Skill 不需要 `compatibility` 字段。
</Note>

#### `metadata` 字段

可选的 `metadata` 字段：

* 从字符串键到字符串值的映射
* 客户端可以使用它来存储 Agent Skills 规范未定义的额外属性
* 我们建议使你的键名尽可能唯一，以避免意外冲突

示例：

```yaml theme={null}
metadata:
  author: example-org
  version: "1.0"
```

#### `allowed-tools` 字段

可选的 `allowed-tools` 字段：

* 预先批准运行的工具列表（以空格分隔）
* 实验性。对该字段的支持可能因 Agent 实现而异

示例：

```yaml theme={null}
allowed-tools: Bash(git:*) Bash(jq:*) Read
```

### 正文内容

Frontmatter 之后的 Markdown 正文包含 Skill 指令。没有格式限制。编写任何有助于 Agent 有效执行任务的内容。

推荐部分：

* 逐步指令
* 输入和输出示例
* 常见边缘情况

请注意，一旦 Agent 决定激活某个 Skill，它将加载整个文件。考虑将较长的 `SKILL.md` 内容拆分为引用的文件。

## 可选目录

### scripts/

包含 Agent 可以运行的可执行代码。脚本应：

* 自包含或清晰记录依赖项
* 包含有用的错误消息
* 优雅地处理边缘情况

支持的语言取决于 Agent 的实现。常见的选择包括 Python、Bash 和 JavaScript。

### references/

包含 Agent 在需要时可以读取的额外文档：

* `REFERENCE.md` - 详细的技术参考
* `FORMS.md` - 表单模板或结构化数据格式
* 领域特定文件（`finance.md`、`legal.md` 等）

保持单个[文件引用](#文件引用)的专注。Agent 按需加载这些文件，因此较小的文件意味着更少的上下文占用。

### assets/

包含静态资源：

* 模板（文档模板、配置模板）
* 图像（图表、示例）
* 数据文件（查找表、模式）

## 渐进式披露

Skills 的结构应利于上下文的高效利用：

1. **元数据** (\~100 tokens)：所有 Skill 在启动时都会加载 `name` 和 `description` 字段
2. **指令** (建议 \< 5000 tokens)：Skill 激活时加载完整的 `SKILL.md` 正文
3. **资源** (按需)：仅在需要时加载文件（例如 `scripts/`、`references/` 或 `assets/` 中的文件）

保持主 `SKILL.md` 在 500 行以内。将详细的参考资料移至单独的文件。

## 文件引用

在 Skill 中引用其他文件时，请使用相对于 Skill 根目录的相对路径：

```markdown theme={null}
有关详情，请参阅[参考指南](references/REFERENCE.md)。

运行提取脚本：
scripts/extract.py
```

保持文件引用距离 `SKILL.md` 一层深度。避免深度嵌套的引用链。

## 验证

使用 [skills-ref](https://github.com/agentskills/agentskills/tree/main/skills-ref) 参考库来验证你的 Skills：

```bash theme={null}
skills-ref validate ./my-skill
```

这将检查你的 `SKILL.md` Frontmatter 是否有效并遵循所有命名规范。


This documentation is built and hosted on [Mintlify](https://mintlify.com), a developer documentation platform.