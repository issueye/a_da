/**
 * Shared shapes for the agent: what a thread holds, what the model is sent,
 * and what the transcript rows render from.
 */

export interface ToolCall {
  id: string
  name: string
  /** Raw JSON as the model produced it, kept for the debug log. */
  args: string
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string
  tool_calls?: { id: string; type: 'function'; function: { name: string; arguments: string } }[]
  tool_call_id?: string
}

export type ToolStatus = 'awaiting' | 'running' | 'done' | 'error' | 'denied'

export type Item =
  | { kind: 'user'; id: string; at: number; text: string; queued?: boolean }
  | { kind: 'assistant'; id: string; at: number; text: string; streaming?: boolean }
  | {
      kind: 'tool'
      id: string
      at: number
      callId: string
      name: string
      args: Record<string, unknown>
      rawArgs: string
      status: ToolStatus
      output?: string
      patch?: string
    }
  | { kind: 'notice'; id: string; at: number; text: string; level: 'info' | 'error' }

export interface Thread {
  id: string
  title: string
  createdAt: number
  /** The project this conversation works in. Every tool call is scoped to it. */
  workspace: string
  items: Item[]
  messages: ChatMessage[]
}

export interface DebugEntry {
  id: number
  at: number
  kind: 'request' | 'delta' | 'tools' | 'tool' | 'error' | 'info'
  text: string
}

export interface ToolOutcome {
  output: string
  ok: boolean
  patch?: string
}

export const TOOL_SPECS = [
  {
    type: 'function' as const,
    function: {
      name: 'list_files',
      description:
        'List files and directories inside the workspace. Use it before reading a file you have not seen.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Directory relative to the workspace root.' },
          depth: { type: 'number', description: 'How many levels to walk. Defaults to 3.' },
        },
      },
    },
  },
  {
    type: 'function' as const,
    function: {
      name: 'read_file',
      description: 'Read a text file from the workspace. Output is capped at 400 lines.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'File path relative to the workspace root.' },
        },
        required: ['path'],
      },
    },
  },
  {
    type: 'function' as const,
    function: {
      name: 'search_files',
      description: 'Search the workspace with a regular expression and return matching lines.',
      parameters: {
        type: 'object',
        properties: {
          pattern: { type: 'string', description: 'JavaScript regular expression source.' },
          glob: { type: 'string', description: 'Optional extension filter, for example "tsx".' },
        },
        required: ['pattern'],
      },
    },
  },
  {
    type: 'function' as const,
    function: {
      name: 'write_file',
      description: 'Create or replace a whole file. Prefer edit_file for a small change.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string' },
          content: { type: 'string' },
        },
        required: ['path', 'content'],
      },
    },
  },
  {
    type: 'function' as const,
    function: {
      name: 'edit_file',
      description:
        'Replace old_string with new_string in a file. old_string must appear exactly once.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string' },
          old_string: { type: 'string' },
          new_string: { type: 'string' },
        },
        required: ['path', 'old_string', 'new_string'],
      },
    },
  },
  {
    type: 'function' as const,
    function: {
      name: 'run_command',
      description:
        'Run a shell command with the workspace as its working directory. Output is capped at 8000 characters.',
      parameters: {
        type: 'object',
        properties: {
          command: { type: 'string' },
          cwd: { type: 'string', description: 'Optional subdirectory inside the workspace.' },
        },
        required: ['command'],
      },
    },
  },
]
