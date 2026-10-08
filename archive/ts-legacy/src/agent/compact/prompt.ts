/**
 * 结构化压缩提示词与格式化引擎
 * 严格对齐 ZCode 的 9 大核心分析板块、双阶段标签约束与无工具纯文本要求
 */

export const NO_TOOLS_PREAMBLE = `CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use list_files, read_file, search_files, write_file, edit_file, run_command, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

`

export const NO_TOOLS_TRAILER = `

REMINDER: Do NOT call any tools. Respond with plain text only — an <analysis> block followed by a <summary> block. Tool calls will be rejected and you will fail the task.`

export const BASE_COMPACT_PROMPT = `Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names and paths
     - key code snippets
     - function / type signatures
     - file edits and diffs
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
   - Note any security-relevant instructions or constraints the user stated (e.g., sensitive files or data to avoid, operations that must not be performed, credential handling rules). These MUST be preserved verbatim in the summary so they continue to apply after compaction.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.

Your summary should be wrapped in <summary>...</summary> tags and MUST include the following 9 sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail.
2. Key Technical Concepts: List all important technical concepts, technologies, architecture and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to recent messages and include code snippets where applicable and include a summary of why each file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to user feedback on errors.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL non-tool-use user messages and instructions in sequence. Preserve any constraints or safety rules verbatim.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary request. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is directly in line with the user's most recent explicit requests and current work.

Here is an example of how your output should be structured:

<example>
<analysis>
[Your thorough analysis and thought process, checking each point chronologically]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description of requests and intents]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File path 1]
     - [Summary of why this file is important and changes made]
     - [Key code snippet]
   - [File path 2]

4. Errors and fixes:
   - [Error description]:
     - [How it was fixed]
     - [User feedback if any]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages:
   - [Original user message 1]
   - [Original user message 2]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]

8. Current Work:
   [Precise description of current work and where execution left off]

9. Optional Next Step:
   [Explicit next step directly continuing the task]
</summary>
</example>

Please provide your summary based on the conversation so far, strictly following this structure and ensuring precision and thoroughness in your response.`

/**
 * 构造用于发起摘要请求的完整提示词
 */
export function buildCompactPrompt(customInstructions?: string): string {
  const customBlock = customInstructions?.trim()
    ? `\n\nAdditional Instructions:\n${customInstructions.trim()}`
    : ''

  return `${NO_TOOLS_PREAMBLE}${BASE_COMPACT_PROMPT}${customBlock}${NO_TOOLS_TRAILER}`
}

/**
 * 从模型输出中清洗提取纯净的结构化摘要
 * 剥离 <analysis> 思考标签，提取 <summary> 中的正文
 */
export function formatCompactSummary(text: string | undefined): string {
  let formatted = text?.trim() ?? ''
  if (!formatted) return ''

  // 移除 <analysis>...</analysis> 块
  formatted = formatted.replace(/<analysis>[\s\S]*?<\/analysis>/gi, '').trim()

  // 提取 <summary>...</summary> 块
  const summaryMatch = formatted.match(/<summary>([\s\S]*?)<\/summary>/i)
  if (summaryMatch && summaryMatch[1]) {
    formatted = summaryMatch[1].trim()
  } else {
    // 若模型未严格包裹 <summary> 标签，去除多余空白后直接使用
    formatted = formatted.replace(/^<summary>/i, '').replace(/<\/summary>$/i, '').trim()
  }

  return formatted.replace(/\n{3,}/g, '\n\n').trim()
}

/**
 * 构造用于替换历史并引导后续轮次继续运行的 Continuation 摘要系统消息
 */
export function buildCompactSummaryMessage(
  summary: string,
  options: {
    recentMessagesPreserved?: boolean
  } = {},
): string {
  let message = `This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n${formatCompactSummary(summary)}`

  if (options.recentMessagesPreserved) {
    message += '\n\nRecent messages are preserved verbatim.'
  }

  return message
}
