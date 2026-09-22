import { readLlmConfig } from '../../config'
import type { AgentTool, AgentToolResult } from '../../core/types'
import { defaultSubagentManager, formatProfilesPrompt } from '../../subagents/manager'
import { defaultSubagentRunner } from '../../subagents/runner'
import type { SendSubagentMessageArgs, SubagentToolArgs } from '../../subagents/types'

export interface CheckSubagentToolArgs {
  subagent_thread_id?: string
  subagent_id?: string
}

export function createSubagentTool(workspace: string): AgentTool<SubagentToolArgs> {
  const profiles = defaultSubagentManager.getSubagentsSync(workspace)
  const dynamicDesc = formatProfilesPrompt(profiles)
  const description = [
    '委派专项任务给独立的子智能体在隔离上下文中自主探索并返回总结。支持异步后台执行（async: true）或同步等待结果。',
    '',
    dynamicDesc,
  ].join('\n')

  return {
    name: 'invoke_subagent',
    label: '委派子智能体',
    description,
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        subagent_id: {
          type: 'string',
          description:
            '目标子智能体 ID。常用选项包括：general_purpose（全能执行专员）、researcher（代码库调研/符号查找/结构分析）、code_reviewer（代码审查/安全隐患与缺陷分析）、tester（自动化测试用例编写与测试运行），或自定义 ID。',
        },
        task: {
          type: 'string',
          description: '清晰明确的委派任务目标与交付要求。',
        },
        additional_context: {
          type: 'string',
          description: '可选的补充背景信息、相关文件路径或前置线索。',
        },
        async: {
          type: 'boolean',
          description:
            '是否以异步后台模式运行。若为 true，立即返回并在独立页签与左侧子会话中后台并发执行，主 Agent 无需阻塞即可继续后续工作；若为 false（默认），等待子智能体产出最终总结后再继续。',
        },
      },
      required: ['subagent_id', 'task'],
    },
    async execute(_callId, args, signal, onUpdate): Promise<AgentToolResult> {
      try {
        const profile = await defaultSubagentManager.getById(args.subagent_id, workspace)
        if (!profile) {
          const available = await defaultSubagentManager.getEnabledSubagents(workspace)
          const listStr = available.map((a) => `\`${a.id}\` (${a.name})`).join(', ')
          return {
            output: `未找到 ID 为 "${args.subagent_id}" 的子智能体。当前可用子智能体包含：${listStr || '无'}`,
            ok: false,
          }
        }

        if (!profile.enabled) {
          return {
            output: `子智能体 "${profile.name}" (${profile.id}) 当前处于禁用状态。`,
            ok: false,
          }
        }

        const config = await readLlmConfig()
        if (!config) {
          return {
            output: '执行失败：当前未配置 LLM 供应商，无法启动子智能体。',
            ok: false,
          }
        }

        onUpdate?.({
          output: `已委派给 [${profile.name}]...\n任务: ${args.task}`,
          ok: true,
        })

        // 优先使用 store.startSubagentThread 以建立独立会话与页签
        let appStore: any = null
        try {
          const mod = await import('../../store')
          appStore = mod.store
        } catch {
          // ignore
        }

        if (appStore && typeof appStore.startSubagentThread === 'function') {
          const { thread, resultPromise } = await appStore.startSubagentThread({
            subagentId: profile.id,
            task: args.task,
            additionalContext: args.additional_context,
            signal,
            onStepUpdate: (update: any) => {
              const stepStr = update.step > 0 ? (update.maxSteps ? `(第 ${update.step}/${update.maxSteps} 步)` : `(第 ${update.step} 步)`) : ''
              const actionStr = update.currentAction ? `\n${update.currentAction}` : ''
              onUpdate?.({
                output: `[${profile.name}] 正在处理 ${stepStr}${actionStr}`,
                ok: true,
              })
            },
          })

          // 异步模式：立即返回启动确认与子会话信息
          if (args.async) {
            return {
              output: `已在后台启动子智能体 [${profile.name}]（会话 ID: ${thread.id}）。已在标签栏与左侧会话树中创建独立子会话并进入后台并发执行。你可以继续处理后续工作，或随时通过 check_subagent 查询进度。`,
              ok: true,
              details: {
                subagent_id: profile.id,
                subagent_name: profile.name,
                subagent_thread_id: thread.id,
                async: true,
              },
            }
          }

          // 同步模式：等待子智能体完成并返回报告
          const result = await resultPromise
          const fileNote = result.outputFile ? `\n\n📄 完整详细报告已保存至：${result.outputFile}` : ''
          return {
            output: `${result.summary}${fileNote}`,
            ok: result.ok,
            details: {
              subagent_id: profile.id,
              subagent_name: profile.name,
              subagent_thread_id: thread.id,
              steps: result.stepsExecuted,
              durationMs: result.durationMs,
              toolCallsCount: result.toolCallsCount,
              outputFile: result.outputFile,
            },
          }
        }

        // 独立运行环境（如单元测试无 store 实例时）的回退路径
        const result = await defaultSubagentRunner.run({
          profile,
          task: args.task,
          additionalContext: args.additional_context,
          workspace,
          parentConfig: config,
          signal,
          onUpdate: (update) => {
            const stepStr = update.step > 0 ? (update.maxSteps ? `(第 ${update.step}/${update.maxSteps} 步)` : `(第 ${update.step} 步)`) : ''
            const actionStr = update.currentAction ? `\n${update.currentAction}` : ''
            onUpdate?.({
              output: `[${profile.name}] 正在处理 ${stepStr}${actionStr}`,
              ok: true,
            })
          },
        })

        const fileNote = result.outputFile ? `\n\n📄 完整详细报告已保存至：${result.outputFile}` : ''
        return {
          output: `${result.summary}${fileNote}`,
          ok: result.ok,
          details: {
            subagent_id: profile.id,
            subagent_name: profile.name,
            steps: result.stepsExecuted,
            durationMs: result.durationMs,
            toolCallsCount: result.toolCallsCount,
            outputFile: result.outputFile,
          },
        }
      } catch (error) {
        return {
          output: `调用子智能体发生未知错误：${(error as Error).message || String(error)}`,
          ok: false,
        }
      }
    },
  }
}

export function createCheckSubagentTool(): AgentTool<CheckSubagentToolArgs> {
  return {
    name: 'check_subagent',
    label: '查询子智能体进度',
    description: '查询先前以异步模式启动的子智能体的执行状态、当前进度或产出报告。',
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        subagent_thread_id: {
          type: 'string',
          description: '子智能体的会话 ID（由 invoke_subagent 返回）。',
        },
        subagent_id: {
          type: 'string',
          description: '可选的子智能体类型 ID（如 researcher、code_reviewer、tester）。',
        },
      },
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      let appStore: any = null
      try {
        const mod = await import('../../store')
        appStore = mod.store
      } catch {
        // ignore
      }

      if (!appStore) {
        return {
          output: '未找到可用会话状态存储。',
          ok: false,
        }
      }

      const thread = appStore.threads.find((t: any) => {
        if (args.subagent_thread_id && t.id === args.subagent_thread_id) return true
        if (args.subagent_id && t.subagentId === args.subagent_id) return true
        return false
      })

      if (!thread) {
        return {
          output: `未找到匹配的子智能体会话（ID: ${args.subagent_thread_id ?? args.subagent_id ?? '未知'}）。`,
          ok: false,
        }
      }

      const isRunning = appStore.isThreadRunning(thread.id)
      if (isRunning) {
        const lastAction = thread.items[thread.items.length - 1]
        let actionDesc = ''
        if (lastAction?.kind === 'tool') {
          actionDesc = `正在执行工具: ${lastAction.name}`
        } else if (lastAction?.kind === 'thinking') {
          actionDesc = '正在深度思考中'
        } else if (lastAction?.kind === 'assistant') {
          actionDesc = '正在生成报告'
        }

        return {
          output: `子智能体「${thread.title}」当前仍在运行中... ${actionDesc ? `(${actionDesc})` : ''}`,
          ok: true,
          details: {
            thread_id: thread.id,
            status: 'running',
            items_count: thread.items.length,
          },
        }
      }

      // 已完成：提取最后的助手回复报告
      const assistantItems = (thread.items as any[]).filter((item: any) => item.kind === 'assistant')
      const finalReport =
        assistantItems.length > 0 ? assistantItems[assistantItems.length - 1]!.text : '执行已结束，未产出正文报告。'

      return {
        output: `子智能体「${thread.title}」已执行完毕。总结报告如下：\n\n${finalReport}`,
        ok: true,
        details: {
          thread_id: thread.id,
          status: 'done',
        },
      }
    },
  }
}

export function createSendSubagentMessageTool(): AgentTool<SendSubagentMessageArgs> {
  return {
    name: 'send_subagent_message',
    label: '向子智能体发送消息',
    description:
      '向正在运行或已完成的子智能体发送补充要求、反馈或实时转向指导。如果子智能体正在运行，指令将在当前工具执行后立即生效纠偏；如果已完成，将唤醒其继续推进任务。',
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        subagent_thread_id: {
          type: 'string',
          description: '目标子智能体的会话 ID（由 invoke_subagent 返回）。',
        },
        message: {
          type: 'string',
          description: '发送给子智能体的具体要求、反馈或补充上下文。',
        },
        summary: {
          type: 'string',
          description: '可选的简要指令摘要（例如："缩小搜索范围至 src/compiler"）。',
        },
      },
      required: ['subagent_thread_id', 'message'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      let appStore: any = null
      try {
        const mod = await import('../../store')
        appStore = mod.store
      } catch {
        // ignore
      }

      if (!appStore) {
        return {
          output: '未找到可用会话状态存储。',
          ok: false,
        }
      }

      const res = await appStore.steerSubagentThread({
        subagentThreadId: args.subagent_thread_id,
        message: args.message,
        summary: args.summary,
      })

      return {
        output: res.text,
        ok: res.status !== 'not_found',
        details: {
          status: res.status,
          subagent_thread_id: args.subagent_thread_id,
        },
      }
    },
  }
}

