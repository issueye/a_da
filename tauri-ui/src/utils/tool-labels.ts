/**
 * 工具名称 → 界面中文标签（**单一真源**）。
 *
 * 这份表原先私有在 `Transcript.tsx` 里；审批浮窗（`ApprovalFloatingPanel`）也要用同一套
 * 文案点名"到底在批准哪个工具"——两处各抄一份必然漂移（改了一处忘了另一处），
 * 所以上移为共享模块，`Transcript` 与审批浮窗都从这里取。
 */
export const TOOL_LABEL: Record<string, string> = {
  list_files: '列出文件',
  read_file: '读取文件',
  search_files: '搜索代码',
  find_symbol: '查找符号',
  write_file: '写入文件',
  edit_file: '修改文件',
  run_command: '执行命令',
  run_background: '后台命令',
  check_task: '查看后台任务',
  kill_task: '停止后台任务',
  todo: '任务规划',
  invoke_subagent: '委派子智能体',
  check_subagent: '查询子智能体',
  ask_user: '向用户提问',
  decide: '结构化决策',
  check_gate: '验收门禁',
  git_status: 'Git 状态',
  git_diff: 'Git 变更',
  git_log: 'Git 历史',
  code_outline: '代码大纲',
  run_tests: '运行测试',
  batch_write: '批量写入',
  batch_replace: '批量替换',
  project_inspect: '项目诊断',
  read_url_content: '网页抓取',
}
