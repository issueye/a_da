/**
 * 工具摘要格式化（卡片头部展示）
 *
 * 纯字符串格式化函数，前后端通用，无外部复杂依赖。
 */
export function describeTool(name: string, args: Record<string, unknown>): string {
  switch (name) {
    case 'run_command':
      return String(args.command ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'run_background':
      return String(args.command ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'check_task': {
      const taskId = String(args.task_id ?? '').trim()
      return taskId || '全部任务'
    }
    case 'kill_task':
      return String(args.task_id ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'search_files':
      return `/${String(args.pattern ?? '').replace(/\r?\n+/g, ' ').trim()}/`
    case 'find_symbol': {
      const query = String(args.query ?? '').replace(/\r?\n+/g, ' ').trim()
      const kind = typeof args.kind === 'string' && args.kind ? `:${args.kind}` : ''
      return `@${query}${kind}`
    }
    case 'list_files':
      return String(args.path || '.').replace(/\r?\n+/g, ' ').trim()
    case 'read_file':
    case 'write_file':
    case 'edit_file':
      return String(args.path ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'read_files': {
      const paths = Array.isArray(args.paths) ? (args.paths as unknown[]) : []
      const files = Array.isArray(args.files) ? (args.files as { path?: unknown }[]) : []
      const count = paths.length + files.length
      const first = String(paths[0] ?? files[0]?.path ?? '').replace(/\r?\n+/g, ' ').trim()
      return count > 0 ? `${first}${count > 1 ? ` 等 ${count} 个文件` : ''}` : ''
    }
    case 'edit_files': {
      const files = Array.isArray(args.files) ? (args.files as { path?: unknown }[]) : []
      const first = String(files[0]?.path ?? '').replace(/\r?\n+/g, ' ').trim()
      return files.length > 0 ? `${first}${files.length > 1 ? ` 等 ${files.length} 个文件` : ''}` : ''
    }
    case 'todo': {
      const todos = Array.isArray(args.todos) ? (args.todos as { title?: string; status?: string }[]) : []
      const active = todos.find((t) => t.status === 'in_progress') ?? todos.find((t) => t.status !== 'completed')
      return (active?.title ?? (todos.length ? `${todos.filter((t) => t.status === 'completed').length}/${todos.length}` : '')).replace(/\r?\n+/g, ' ').trim()
    }
    case 'invoke_subagent': {
      const subagentId = String(args.subagent_id ?? '').trim()
      const task = String(args.task ?? '').split('\n')[0]!.trim()
      return `${subagentId}: ${task}`
    }
    case 'ask_user':
      // 卡片头部的一行摘要：问题本身在问答卡里，这里只取开头
      return String(args.question ?? '').replace(/\r?\n+/g, ' ').trim().slice(0, 60)
    case 'check_subagent': {
      return String(args.subagent_thread_id ?? args.subagent_id ?? '查询子智能体').replace(/\r?\n+/g, ' ').trim()
    }
    case 'await_subagents': {
      const ids = Array.isArray(args.subagent_thread_ids) ? (args.subagent_thread_ids as unknown[]) : []
      if (ids.length === 0) return '全部运行中的子智能体'
      const first = String(ids[0] ?? '').replace(/\r?\n+/g, ' ').trim()
      return ids.length > 1 ? `${first} 等 ${ids.length} 个` : first
    }
    case 'notify_parent':
      return String(args.summary ?? '').replace(/\r?\n+/g, ' ').trim().slice(0, 60)
    case 'decide': {
      const questions = args.questions && typeof args.questions === 'object' ? Object.keys(args.questions) : []
      if (questions.length === 0) return '类型化决策'
      return questions.length > 1 ? `${questions[0]} 等 ${questions.length} 个判定` : String(questions[0])
    }
    case 'design_decision':
      return String(args.prompt ?? '').replace(/\r?\n+/g, ' ').trim().slice(0, 60)
    case 'check_gate': {
      const criteria = String(args.criteria ?? '').replace(/\r?\n+/g, ' ').trim()
      const source = typeof args.source === 'string' && args.source !== 'diff' ? `（${args.source}）` : ''
      return `${criteria.slice(0, 50)}${source}`
    }
    default:
      return ''
  }
}
