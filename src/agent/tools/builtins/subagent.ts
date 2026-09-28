import { readLlmConfig } from '../../config'
import type { AgentTool, AgentToolResult } from '../../core/types'
import { defaultSubagentManager, formatProfilesPrompt } from '../../subagents/manager'
import { defaultSubagentRunner } from '../../subagents/runner'
import type { SendSubagentMessageArgs, SubagentToolArgs } from '../../subagents/types'

export interface CheckSubagentToolArgs {
  subagent_thread_id?: string
  subagent_id?: string
}

export function createSubagentTool(workspace: string, defaultParentThreadId?: string): AgentTool<SubagentToolArgs> {
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
    executionMode: 'parallel',
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
            '是否以后台并发模式运行。默认 false：同步等待子智能体产出完整报告后再返回，适合单个必须拿到结论才能继续的任务。置为 true 时立即返回并让子智能体在后台独立页签中继续跑，主对话不被阻塞——有多个互不依赖的子任务时，请在**同一批**里并发发起多个 async 调用，随后用 check_subagent 查询进度与结论。',
        },
      },
      required: ['subagent_id', 'task'],
    },
    async execute(callId, args, signal, onUpdate): Promise<AgentToolResult> {
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
          const card = appStore.cards?.get?.(callId)
          const parentThreadId =
            defaultParentThreadId ??
            card?.threadId ??
            appStore.threads.find((t: any) =>
              t.items?.some((it: any) => it.kind === 'tool' && (it.callId === callId || it.id === callId))
            )?.id ??
            appStore.activeId

          let subagentThreadRef: any = null
          // 后台模式下工具已经返回：再回调 onUpdate 只会把增量堆进一个没人消费的
          // 缓冲区（runOneTool 的 updates 数组），直到子任务结束才释放。进度在子会话
          // 页签里本就实时可见，check_subagent 也能查状态，所以返回后直接静音。
          let detached = false
          const { thread, resultPromise } = await appStore.startSubagentThread({
            parentThreadId,
            subagentId: profile.id,
            task: args.task,
            additionalContext: args.additional_context,
            // 后台模式不继承父轮次的 signal：插队或停止主对话会 abort 该 controller，
            // 继承的话后台子任务会被连带杀掉，那就不是「后台」了。后台会话有自己
            // 的 controller，用户可在它的页签里单独停止。
            signal: args.async ? undefined : signal,
            onStepUpdate: (update: any) => {
              if (detached) return
              const currentThreadId = update?.threadId ?? subagentThreadRef?.id
              const idSuffix = currentThreadId ? `\n(子会话 ID: ${currentThreadId})` : ''
              const stepStr = update.step > 0 ? (update.maxSteps ? `(第 ${update.step}/${update.maxSteps} 步)` : `(第 ${update.step} 步)`) : ''
              const actionStr = update.currentAction ? `\n${update.currentAction}` : ''
              onUpdate?.({
                output: `[${profile.name}] 正在处理 ${stepStr}${actionStr}${idSuffix}`,
                ok: true,
                details: {
                  subagent_id: profile.id,
                  subagent_name: profile.name,
                  subagent_thread_id: currentThreadId,
                },
              })
            },
          })
          subagentThreadRef = thread

          // 立即更新一次卡片，注入明确的 subagent_thread_id
          onUpdate?.({
            output: `已委派给 [${profile.name}]（子会话 ID: ${thread.id}）...\n任务: ${args.task}`,
            ok: true,
            details: {
              subagent_id: profile.id,
              subagent_name: profile.name,
              subagent_thread_id: thread.id,
            },
          })

          // 后台模式：不 await，让子智能体在自己的驱动任务里继续跑。返回时明确给出
          // 会话 ID，主对话据此用 check_subagent 查询结论，而不是在这里干等。
          if (args.async) {
            detached = true
            void resultPromise.catch(() => {
              // 失败已记录在子会话与父会话通知里，这里只是别让 rejection 变成
              // unhandled。成功路径不需要回调：结论可经 check_subagent 取回。
            })
            return {
              output: `已在后台启动子智能体 [${profile.name}]（子会话 ID: ${thread.id}）。它会在独立页签中继续执行，现在可以并行推进其他工作，稍后用 check_subagent（subagent_thread_id: ${thread.id}）查询进度与结论。`,
              ok: true,
              details: {
                subagent_id: profile.id,
                subagent_name: profile.name,
                subagent_thread_id: thread.id,
                status: 'started_async',
              },
            }
          }

          // 等待子智能体完成并返回真实报告（严禁提前虚假结案）
          const result = await resultPromise
          const fileNote = result.outputFile ? `\n\n📄 完整详细报告已保存至：${result.outputFile}` : ''
          return {
            output: `${result.summary}${fileNote}\n\n(子会话 ID: ${thread.id})`,
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

        // 独立运行环境（如无 store 实例时）的回退路径。
        // 这里没有会话存储，子智能体不会挂出页签，check_subagent 也就查不到它——
        // 所以即便请求了 async 也只能同步等待，并在结果里说明这一点，而不是假装
        // 它已经在后台跑（那会让调用方去查一个永远不会存在的会话）。
        const config = await readLlmConfig()
        if (!config) {
          return {
            output: '执行失败：当前未配置 LLM 供应商，无法启动子智能体。',
            ok: false,
          }
        }
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
        const asyncNote = args.async
          ? '\n\n（注意：当前运行环境没有会话存储，无法后台并行，本次已同步执行完毕。）'
          : ''
        return {
          output: `${result.summary}${fileNote}${asyncNote}`,
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

export interface ResumeSubagentToolArgs {
  subagent_thread_id: string
  instruction?: string
  async?: boolean
}

export function createResumeSubagentTool(): AgentTool<ResumeSubagentToolArgs> {
  return {
    name: 'resume_subagent',
    label: '恢复子智能体工作',
    description:
      '当子智能体因网络波动、超时或异常中断停止时，恢复其运行并让其从上次中断的状态与上下文中继续推进未完成的任务，并最终产出总结报告。支持同步等待结果或异步后台运行。',
    executionMode: 'parallel',
    parameters: {
      type: 'object',
      properties: {
        subagent_thread_id: {
          type: 'string',
          description: '需要恢复的子智能体会话 ID（由 invoke_subagent 返回或在子智能体会话中展示）。',
        },
        instruction: {
          type: 'string',
          description:
            '可选的恢复指导说明或重试要求（例如："网络已恢复，请重试上一步并完成剩余分析"）。若不填则默认让子智能体从中断处继续。',
        },
        async: {
          type: 'boolean',
          description: '是否在后台异步运行。默认 false（将等待子智能体恢复执行完毕并返回最终报告）。',
        },
      },
      required: ['subagent_thread_id'],
    },
    async execute(callId, args, signal, onUpdate): Promise<AgentToolResult> {
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

      try {
        onUpdate?.({
          output: `正在恢复子智能体会话 [${args.subagent_thread_id}]...`,
          ok: true,
        })

        let detached = false
        const { thread, resultPromise } = await appStore.resumeSubagentThread({
          subagentThreadId: args.subagent_thread_id,
          instruction: args.instruction,
          // 后台恢复同样不继承父轮次 signal，理由见 invoke_subagent：否则主对话
          // 一旦插队或停止，后台子任务会被一起 abort 掉。
          signal: args.async ? undefined : signal,
          onStepUpdate: (update: any) => {
            if (detached) return
            const currentThreadId = update?.threadId ?? thread.id
            const idSuffix = currentThreadId ? `\n(子会话 ID: ${currentThreadId})` : ''
            const stepStr =
              update.step > 0
                ? update.maxSteps
                  ? `(第 ${update.step}/${update.maxSteps} 步)`
                  : `(第 ${update.step} 步)`
                : ''
            const actionStr = update.currentAction ? `\n${update.currentAction}` : ''
            onUpdate?.({
              output: `[${thread.title}] 正在恢复处理 ${stepStr}${actionStr}${idSuffix}`,
              ok: true,
              details: {
                subagent_id: thread.subagentId,
                subagent_thread_id: thread.id,
              },
            })
          },
        })

        if (args.async) {
          detached = true
          void resultPromise.catch(() => {
            // 失败已落盘到子会话，这里只需要吞掉 rejection
          })
          return {
            output: `已成功唤醒并恢复子智能体「${thread.title}」（会话 ID: ${thread.id}）在后台继续执行。你可以随时通过 check_subagent 工具查询进度或报告。`,
            ok: true,
            details: {
              subagent_id: thread.subagentId,
              subagent_thread_id: thread.id,
              status: 'resumed_async',
            },
          }
        }

        const result = await resultPromise
        const fileNote = result.outputFile ? `\n\n📄 完整详细报告已保存至：${result.outputFile}` : ''
        return {
          output: `${result.summary}${fileNote}\n\n(子会话 ID: ${thread.id})`,
          ok: result.ok,
          details: {
            subagent_id: thread.subagentId,
            subagent_thread_id: thread.id,
            steps: result.stepsExecuted,
            durationMs: result.durationMs,
            toolCallsCount: result.toolCallsCount,
            outputFile: result.outputFile,
          },
        }
      } catch (error) {
        return {
          output: `恢复子智能体失败：${(error as Error).message || String(error)}`,
          ok: false,
        }
      }
    },
  }
}


