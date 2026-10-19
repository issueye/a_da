import type { ClientSnapshot, Item } from '../types'
/**
 * 把一张新卡片按 `at`（毫秒时间戳）插到列表里正确的位置；时间戳缺失就追加到末尾。
 *
 * 卡片顺序**只能按时间戳**定，不能按"到达顺序"：增量帧的 `upserted` 是按 id 排序的
 * （宿主刻意让帧内容可复现），收尾那一帧一次带回"用户 + 助手"两条时，
 * 到达顺序会把回答排到提问前面。等时间戳的卡片保持原有相对顺序（稳定插入）。
 *
 * 🔴 这条规则的**前提**是宿主保证"同一个会话里卡片的 `at` 严格递增"
 * （内存侧 `store::next_item_at`、磁盘回填侧 `session::manager::CardStamps`）——
 * 否则同一毫秒里的两张卡（思考 + 正文、正文 + 首段流式）会按 id 字典序落位，
 * 表现为**轮次一收尾思考块就掉到正文下面**。
 * 契约与守门见 `docs/protocol/state-patch.md` 的"顺序"一节。
 */
function insertByItemTime(items: any[], item: any): any[] {
  const at = typeof item?.at === 'number' ? item.at : null
  if (at === null) return [...items, item]
  let i = items.length
  while (i > 0) {
    const prevAt = typeof items[i - 1]?.at === 'number' ? items[i - 1].at : null
    if (prevAt === null || prevAt <= at) break
    i--
  }
  return [...items.slice(0, i), item, ...items.slice(i)]
}

  /**
   * 同步快照 / 增量帧里的配置与元数据字段至客户端状态。
   *
   * 🔴 必须全量快照（`applySnapshot`）与增量帧（`applyPatch`）共用：
   * 服务端快照里是 `config` (ConfigSnapshot) 和 `workspace`，而客户端投影维护的是
   * `providerConfig`、`currentMode`、`approvalMode`、`effort`、`retryCount`、`activeWorkspace` 等展平字段。
   * 早先 `applyPatch` 仅执行 `Object.assign(snapshot, metaRest)`，导致模型/模式/审批策略
   * 等运行时配置在增量更新时从未同步到 `snapshot.providerConfig`，界面模型切换因此失效。
   */
  export function syncMetadata(snapshot: ClientSnapshot, payload: any) {
    if (!payload) return

    if (Array.isArray(payload.queue)) {
      snapshot.queue = payload.queue
    }

    if (Array.isArray(payload.runningThreadIds)) {
      snapshot.runningThreadIds = payload.runningThreadIds
      snapshot.running = payload.runningThreadIds.includes(snapshot.activeThreadId)
    } else if (payload.runningThreadIds !== undefined) {
      snapshot.runningThreadIds = []
      snapshot.running = false
    }

    if (Array.isArray(payload.waitingThreadIds)) {
      snapshot.waitingThreadIds = payload.waitingThreadIds
    }

    if (payload.workspace?.project) {
      snapshot.activeWorkspace = payload.workspace.project
    }

    if (Array.isArray(payload.providers)) {
      snapshot.providers = payload.providers
    }
    if (payload.activeProviderId) {
      snapshot.activeProviderId = payload.activeProviderId
    }

    if (payload.config) {
      if (payload.config.mode) snapshot.currentMode = payload.config.mode
      if (payload.config.approval) snapshot.approvalMode = payload.config.approval
      if (payload.config.effort) snapshot.effort = payload.config.effort
      if (typeof payload.config.retryCount === 'number') {
        snapshot.retryCount = payload.config.retryCount
      }
      if (payload.config.model) {
        snapshot.providerConfig = {
          ...snapshot.providerConfig,
          model: payload.config.model,
          contextWindow: payload.config.contextWindow,
          supportsImages: payload.config.supportsImages,
        }
      }
    }

    // 活跃供应商连接信息同步至 providerConfig
    const activePId = snapshot.activeProviderId || payload.activeProviderId
    const curProv = (snapshot.providers || []).find((p) => p.id === activePId)
    if (curProv) {
      snapshot.providerConfig = {
        ...snapshot.providerConfig,
        baseUrl: curProv.baseUrl,
        apiKey: curProv.apiKey,
        protocol: curProv.protocol,
        name: curProv.name,
        customHeaders: curProv.customHeaders,
        proxyUrl: curProv.proxyUrl,
        model: payload.config?.model || snapshot.providerConfig?.model || curProv.models?.[0]?.id || 'gpt-4o',
        contextWindow: payload.config?.contextWindow ?? snapshot.providerConfig?.contextWindow,
        supportsImages: payload.config?.supportsImages ?? snapshot.providerConfig?.supportsImages,
      }
    }
  }

  /** 这条 item 是不是用户消息（乐观项与"谁算作者"的判定都用它）。 */
  export function isUserItem(it: any): boolean {
    return it?.kind === 'user' || it?.role === 'user'
  }

  /**
   * 把一批 item 增删改应用到某个会话上，**返回新的会话对象（以及新的 `items` 数组）**。
   *
   * 为什么返回值而不是就地改：界面按**数组身份**记忆卡片列表，见 `applyPatch` 里那段说明。
   * 另外这里顺手让本地乐观消息退场：服务端那条用户消息到达时，本地那份
   * `optimistic_*` 的 id 服务端永远不认识（也就永远不会发 `removed` 来删它），
   * 不退场就是同一句话挂着两个气泡。
   */
  export function withItemDelta(thread: any, upserts: any[], dropIds?: Set<string>): any {
    let items: any[] = Array.isArray(thread?.items) ? thread.items : []
    if (dropIds && dropIds.size > 0) items = items.filter((x: any) => !dropIds.has(x.id))
    const incoming = upserts.filter((raw) => raw != null)
    const normalized = incoming.map((raw) => normalizeItem(raw))
    if (normalized.some((it) => isUserItem(it))) {
      items = items.filter(
        (x: any) =>
          !(typeof x?.id === 'string' && x.id.startsWith('optimistic_') && isUserItem(x))
      )
    }
    for (const norm of normalized) {
      const idx = items.findIndex((x: any) => x.id === norm.id)
      // 已在列表里 → 就地换成新版本（流式吐字走这条，位置不动）
      if (idx >= 0) {
        items = items.map((x: any, i: number) => (i === idx ? norm : x))
        continue
      }
      // 新卡片按时间戳插入，**不能一律追加到末尾**：宿主一帧的 `upserted` 是按 id 排序的
      // （帧内容可复现），而轮次收尾那一帧会一次带来"用户 + 助手"两条——直接追加就会把
      // 回答排到提问前面（实测：整轮结束时两行上下颠倒）。
      items = insertByItemTime(items, norm)
    }
    return { ...thread, items }
  }

  export function normalizeItem(it: any): Item {
    if (!it) return it
    const kind = it.kind || (it.role === 'user' ? 'user' : 'assistant')
    const role = it.role || (kind === 'user' ? 'user' : 'assistant')

    let question = it.question || it.details?.question
    if (question && typeof question === 'object') {
      const qCallId = question.callId || it.callId || it.id
      let options = Array.isArray(question.options) ? question.options : []
      if (options.length === 0 && Array.isArray(question.choices)) {
        options = question.choices.map((c: any) => ({
          // `id` 要**单独留着**：界面得把它显示在选项前面（用户照着 c1/c2 回话）
          id: typeof c === 'string' ? undefined : c.id || c.value,
          value: typeof c === 'string' ? c : c.id || c.value || c.label || String(c),
          label: typeof c === 'string' ? c : c.label || c.title || String(c),
          description: typeof c === 'string' ? undefined : c.description,
        }))
      }
      question = {
        ...question,
        callId: qCallId,
        options,
      }
    } else if (it.tool === 'ask_user' || it.name === 'ask_user') {
      const parsedArgs = typeof it.args === 'string'
        ? (() => { try { return JSON.parse(it.args) } catch { return {} } })()
        : (it.args || {})
      if (parsedArgs.question) {
        let options = Array.isArray(parsedArgs.options) ? parsedArgs.options : []
        if (options.length === 0 && Array.isArray(parsedArgs.choices)) {
          options = parsedArgs.choices.map((c: any) => ({
            id: typeof c === 'string' ? undefined : c.id || c.value,
            value: typeof c === 'string' ? c : c.id || c.value || c.label || String(c),
            label: typeof c === 'string' ? c : c.label || c.title || String(c),
            description: typeof c === 'string' ? undefined : c.description,
          }))
        }
        question = {
          callId: it.callId || it.id,
          question: parsedArgs.question,
          options,
        }
      }
    }

    let durationMs = it.durationMs ?? it.duration_ms
    let startedAt = it.startedAt ?? it.started_at
    let finishedAt = it.finishedAt ?? it.finished_at
    if (durationMs === undefined && typeof it.output === 'string' && it.output.trim().startsWith('{')) {
      try {
        const parsed = JSON.parse(it.output)
        if (parsed && typeof parsed === 'object') {
          durationMs = parsed.durationMs ?? parsed.duration_ms
          if (startedAt === undefined) startedAt = parsed.startedAt ?? parsed.started_at
          if (finishedAt === undefined) finishedAt = parsed.finishedAt ?? parsed.finished_at
        }
      } catch {}
    }

    return {
      ...it,
      id: it.id || `item_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      kind,
      role,
      at: it.at || it.createdAt || Date.now(),
      createdAt: it.at || it.createdAt || Date.now(),
      text: it.text || (kind === 'thinking' ? it.text : ''),
      thinking: it.thinking || (kind === 'thinking' ? it.text : undefined),
      question,
      durationMs,
      startedAt,
      finishedAt,
    }
  }
