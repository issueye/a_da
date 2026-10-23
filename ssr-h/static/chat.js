/*
 * 聊天页专属交互。
 *
 * 组件级行为（折叠状态、复制、输入框聚焦与自动增高）在 components.js 里，
 * 这里只保留与"对话流"强相关的部分：滚动跟随、代码块复制、侧栏搜索。
 *
 * live 轮询用 OOB 方式替换 #transcript（不是 htmx 的普通换 DOM），
 * 所以这里用 MutationObserver 而不是 htmx:afterSwap 钩子。
 */
(() => {
  const scroller = () => document.getElementById('chat-scroll');
  const transcriptOf = () => document.getElementById('transcript');
  let stickToBottom = true;
  let chatTop = 0;

  /* 展开面板内部的滚动位置，键是 `item id#面板序号`。
     live 轮询整段替换 #transcript 后，新旧节点不是同一个 DOM，
     位置只能靠记录还原——不还原就是"展开的工具每 2 秒自己滚回顶部"。 */
  const panelScroll = new Map();

  const nearBottom = () => {
    const el = scroller();
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  };

  const scrollToBottom = () => {
    const el = scroller();
    if (el) el.scrollTop = el.scrollHeight;
  };

  /* "回到最新"按钮：只在离开底部时出现。
   *
   * 插件也可以用同一套行为：任意元素加 `data-ada-scroll-bottom` 即可，
   * 核心脚本负责"点击回到底部 + 恢复自动跟随 + 按滚动位置切换 hidden"。
   * 推荐在 `transcript.after` 插槽里复用 `.ada-scroll-bottom` 类（sticky 居中），
   * 这样插件不需要写任何 JavaScript（TSX 本来也不允许事件回调）。
   */
  const scrollButtons = () =>
    document.querySelectorAll('#scroll-bottom, [data-ada-scroll-bottom]');
  const syncBottomButtons = () => {
    const hidden = nearBottom();
    scrollButtons().forEach((button) => {
      button.hidden = hidden;
    });
  };

  /* 面板的身份：所属折叠行的 data-key + 它是行内第几个面板。
     服务端渲染顺序稳定（参数 → 执行输出），所以序号可跨替换对齐。 */
  const panelId = (node) => {
    const row = node.closest ? node.closest('details.ada-disclosure') : null;
    if (!row) return null;
    const key = row.getAttribute('data-key');
    if (!key) return null;
    const bodies = row.querySelectorAll('.ada-panel__body');
    for (let i = 0; i < bodies.length; i += 1) {
      if (bodies[i] === node) return key + '#' + i;
    }
    return null;
  };

  /* scroll 不冒泡，但捕获阶段能拿到任意元素的滚动，不用逐个绑定。 */
  document.addEventListener('scroll', (event) => {
    const target = event.target;
    if (!target) return;
    if (target === scroller()) {
      chatTop = target.scrollTop;
      stickToBottom = nearBottom();
      syncBottomButtons();
      return;
    }
    if (target.classList && target.classList.contains('ada-panel__body')) {
      const id = panelId(target);
      if (id) panelScroll.set(id, target.scrollTop);
    }
  }, { capture: true, passive: true });

  /* 替换后还原视图位置。顺序不能反：details 折叠着时面板没有高度，
     先设 scrollTop 会被浏览器夹成 0，必须先把展开态放回去再量。 */
  const restoreScroll = () => {
    const box = transcriptOf();
    if (!box) return;
    // 保险：即使 components.js 的观察器还没跑到这一帧，也先把展开态放回去。
    if (window.AdaComponents && window.AdaComponents.bindDisclosures) {
      window.AdaComponents.bindDisclosures(box);
    }
    if (panelScroll.size) {
      void box.offsetHeight; // 强制布局：展开的面板此时才有 scrollHeight
      const rows = box.querySelectorAll('details.ada-disclosure[data-key]');
      for (let i = 0; i < rows.length; i += 1) {
        const row = rows[i];
        if (!row.open) continue;
        const key = row.getAttribute('data-key');
        const bodies = row.querySelectorAll('.ada-panel__body');
        for (let j = 0; j < bodies.length; j += 1) {
          const top = panelScroll.get(key + '#' + j);
          if (top > 0) bodies[j].scrollTop = top;
        }
      }
    }
    const el = scroller();
    if (!el) return;
    if (stickToBottom) {
      el.scrollTop = el.scrollHeight;
      chatTop = el.scrollTop;
    } else if (chatTop > 0) {
      el.scrollTop = chatTop;
    }
    syncBottomButtons();
  };

  /* 代码块悬停显示复制按钮（组件库管工具面板的复制，这里管正文代码块） */
  const enhanceCodeBlocks = () => {
    document.querySelectorAll('.ada-markdown pre').forEach((pre) => {
      if (pre.querySelector('.chat-copy')) return;
      const code = pre.querySelector('code');
      if (!code) return;
      pre.classList.add('has-copy');
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'chat-copy';
      button.setAttribute('aria-label', '复制代码');
      button.textContent = '复制';
      button.addEventListener('click', async () => {
        const text = code.innerText;
        try {
          await navigator.clipboard.writeText(text);
          button.textContent = '已复制';
        } catch {
          button.textContent = '复制失败';
        }
        window.setTimeout(() => { button.textContent = '复制'; }, 1200);
      });
      pre.appendChild(button);
    });
  };

  /* 事件委托：核心按钮与插件按钮（`data-ada-scroll-bottom`）走同一条路径。
     插件是服务端渲染的，动态插入/替换后也不需要重新绑定。 */
  document.addEventListener('click', (event) => {
    const target = event.target;
    const button =
      target && target.closest
        ? target.closest('#scroll-bottom, [data-ada-scroll-bottom]')
        : null;
    if (!button) return;
    event.preventDefault();
    stickToBottom = true;
    scrollToBottom();
    syncBottomButtons();
  });

  /* 插件气泡（`details.ada-popover`）：点外部或 Esc 关闭。
     原生 details 只会在再点 summary 时关闭，对"小窗"来说不够直觉。 */
  document.addEventListener('click', (event) => {
    document.querySelectorAll('details.ada-popover[open]').forEach((popover) => {
      if (!popover.contains(event.target)) popover.open = false;
    });
  });
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    document.querySelectorAll('details.ada-popover[open]').forEach((popover) => {
      popover.open = false;
    });
  });

  const boot = () => {
    enhanceCodeBlocks();
    const transcript = transcriptOf();
    if (transcript && window.MutationObserver) {
      new MutationObserver(() => {
        enhanceCodeBlocks();
        restoreScroll();
      }).observe(transcript, { childList: true });
    }
    scrollToBottom();
    syncBottomButtons();
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();

/* 侧栏会话搜索：纯前端过滤标题，不改变当前会话。
   过滤同时维护三件事：行的 hidden、结果计数（N / 总数）、以及"没有匹配"的空态——
   以前整组 hidden 之后界面就是一片空白，用户分不清"没匹配"和"列表挂了"。
   列表若被重新渲染，滚动位置与过滤结果都会重新套用。 */
(() => {
  const initThreadSearch = () => {
    const input = document.getElementById('chat-thread-search');
    if (!input) return;
    const list = document.getElementById('thread-list');
    const box = input.closest('.ada-search');
    const count = box && box.querySelector('.ada-search__count');
    const empty = document.getElementById('thread-empty');
    const clear = empty && empty.querySelector('.ada-list-empty__clear');

    let listScroll = 0;
    const apply = () => {
      const query = input.value.trim().toLowerCase();
      document.querySelectorAll('.ada-thread').forEach((item) => {
        item.hidden = Boolean(query) && !item.textContent.toLowerCase().includes(query);
      });
      // 工作区树：会话全被过滤掉时把整组藏起来；工作区名命中则整组展开、会话全显示。
      document.querySelectorAll('.ada-ws').forEach((group) => {
        const details = group.querySelector('details');
        const name = (group.querySelector('.ada-ws__name')?.textContent || '').toLowerCase();
        const nameHit = Boolean(query) && name.includes(query);
        const rows = group.querySelectorAll('.ada-thread');
        if (nameHit) {
          rows.forEach((row) => { row.hidden = false; });
        }
        const hasVisible = Array.from(rows).some((row) => !row.hidden);
        group.hidden = Boolean(query) && !nameHit && !hasVisible;
        if (query && (nameHit || hasVisible) && details) details.open = true;
      });

      const rows = document.querySelectorAll('.ada-thread');
      const visible = document.querySelectorAll('.ada-thread:not([hidden])').length;
      if (box) box.classList.toggle('is-searching', Boolean(query));
      if (count) {
        count.hidden = !query;
        count.textContent = query ? `${visible} / ${rows.length}` : '';
      }
      if (empty) empty.hidden = !(query && visible === 0);
    };

    input.addEventListener('input', apply);
    input.addEventListener('keydown', (event) => {
      // Esc 清空而不是"关掉整个侧栏"：焦点在搜索框里时用户想撤掉的是查询。
      if (event.key === 'Escape') {
        event.stopPropagation();
        input.value = '';
        apply();
      }
    });
    if (clear) {
      clear.addEventListener('click', () => {
        input.value = '';
        apply();
        input.focus();
      });
    }

    // 侧栏有两种收起形态：窄屏抽屉（未打开）与宽屏窄轨（is-sidebar-collapsed）。
    // 都得先展开再聚焦——否则焦点落在 display:none 的输入框上，按了 Ctrl K 毫无反应。
    const reveal = () => {
      const side = document.getElementById('chat-sidebar');
      const wrap = document.querySelector('.ada-shell');
      const toggle = document.querySelector('[data-sidebar-toggle]');
      const narrow = window.matchMedia('(max-width: 760px)').matches;
      const drawerClosed = narrow && side && !side.classList.contains('is-open');
      const railCollapsed = !narrow && wrap && wrap.classList.contains('is-sidebar-collapsed');
      if ((drawerClosed || railCollapsed) && toggle) toggle.click();
      input.focus();
      input.select();
    };

    // Ctrl/⌘ + K 直达搜索
    document.addEventListener('keydown', (event) => {
      const key = event.key.toLowerCase();
      if (!(event.ctrlKey || event.metaKey) || key !== 'k') return;
      if (window.AdaComponents && window.AdaComponents.modalOpen && window.AdaComponents.modalOpen()) return;
      event.preventDefault();
      reveal();
    });

    // 窄轨里的搜索按钮
    document.querySelectorAll('[data-rail-search]').forEach((button) => {
      button.addEventListener('click', reveal);
    });

    if (list) {
      list.addEventListener('scroll', () => { listScroll = list.scrollTop; }, { passive: true });
      if (window.MutationObserver) {
        new MutationObserver(() => {
          if (list.scrollTop !== listScroll) list.scrollTop = listScroll;
          apply();
        }).observe(list, { childList: true });
      }
    }
    apply();
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initThreadSearch);
  } else {
    initThreadSearch();
  }
})();

/* ── 服务端推送（SSE）：逐条更新对话流 ─────────────────────────────────────
 *
 * 服务端把网关的 `evt.state.patch` 渲染成 HTML 后推过来：
 *   - `transcript`：结构变化（新增/删除卡片）时整段同步；
 *   - `item`：单条卡片的正文/状态变化（流式回答就是这一种）；
 *   - `parts`：托盘 / 任务规划 / 顶栏 / 输入框辅助区（JSON，按键替换）。
 *
 * 事件流断开时 `window.__adaStream` 置 false，同一段脚本用 2 秒 `fetch` 轮询
 * `/threads/{id}/live` 兜底；EventSource 重连成功后自动停掉兜底。这样既不用写
 * 两套渲染，也不会在断流时让界面冻住。
 */
(() => {
  const transcriptOf = () => document.getElementById('transcript');
  const initial = transcriptOf();
  if (!initial) return;
  const threadId = initial.getAttribute('data-thread-id') || '';
  if (!threadId) return;

  window.__adaStream = false;

  const escapeSelector = (value) => {
    if (window.CSS && window.CSS.escape) return window.CSS.escape(value);
    return String(value).replace(/["\\]/g, '\\$&');
  };

  const replaceTranscript = (html) => {
    const box = transcriptOf();
    if (box) box.innerHTML = html;
  };

  /* 单条卡片：已存在就原位替换（保住阅读位置），不存在就按 `at` 插到正确位置。 */
  const applyItem = (html) => {
    const box = transcriptOf();
    if (!box) return;
    const template = document.createElement('template');
    template.innerHTML = html.trim();
    const node = template.content.firstElementChild;
    if (!node) return;

    const id = node.getAttribute('data-item-id') || '';
    const at = Number(node.getAttribute('data-at') || 0);
    const existing = id
      ? box.querySelector('[data-item-id="' + escapeSelector(id) + '"]')
      : null;
    if (existing) {
      existing.replaceWith(node);
      return;
    }
    const rows = box.children;
    for (let i = 0; i < rows.length; i += 1) {
      const row = rows[i];
      if (!row.hasAttribute || !row.hasAttribute('data-at')) continue;
      if (Number(row.getAttribute('data-at') || 0) > at) {
        box.insertBefore(node, row);
        return;
      }
    }
    box.appendChild(node);
  };

  const PART_TARGETS = {
    trays: 'ada-trays',
    corner: 'chat-corner',
    topbarStatus: 'topbar-status',
    topbarActions: 'topbar-actions',
    aux: 'composer-aux',
    above: 'composer-above',
    approval: 'composer-approval',
    model: 'composer-model',
  };

  /* 替换动态区域时保住展开中的插件气泡：`details[open][id]` 在替换后按 id 还原。
     否则 composer.above 每 500ms 被 parts 换一次，天气小窗会自己关掉。 */
  const replaceKeepingOpen = (target, html) => {
    const openIds = [];
    target.querySelectorAll('details[open][id]').forEach((node) => {
      openIds.push(node.id);
    });
    target.innerHTML = html;
    openIds.forEach((id) => {
      const reopened = target.querySelector('[id="' + escapeSelector(id) + '"]');
      if (reopened && reopened.tagName === 'DETAILS') reopened.open = true;
    });
  };

  const applyParts = (payload) => {
    Object.keys(PART_TARGETS).forEach((key) => {
      if (!(key in payload)) return;
      const target = document.getElementById(PART_TARGETS[key]);
      if (target) replaceKeepingOpen(target, payload[key]);
    });
  };

  /* ── 兜底轮询 ────────────────────────────────────────────────────────────
   * 为什么不交给 `hx-trigger="every 2s [条件]"`：实测这一版 htmx 的 every 过滤器
   * 不会拦住轮询请求（条件恒为 false 时仍每 2 秒打一次 /live），于是 SSE 明明活着，
   * 界面还是被整段重渲染盖掉。这里用同一段脚本显式控制，取代语义不确定的 htmx 写法。
   */
  let pollTimer = null;

  const stopPolling = () => {
    if (pollTimer) {
      window.clearTimeout(pollTimer);
      pollTimer = null;
    }
  };

  const applyOob = (html) => {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    doc.querySelectorAll('[hx-swap-oob]').forEach((node) => {
      const target = node.id ? document.getElementById(node.id) : null;
      if (target) target.innerHTML = node.innerHTML;
    });
  };

  const pollOnce = async () => {
    pollTimer = null;
    if (window.__adaStream) return;
    try {
      const response = await fetch('/threads/' + encodeURIComponent(threadId) + '/live', {
        headers: { 'HX-Request': 'true' },
      });
      if (response.ok) applyOob(await response.text());
    } catch (error) {
      /* 网关还没回来：下一轮继续探测 */
    }
    if (!window.__adaStream) schedulePoll();
  };

  function schedulePoll() {
    stopPolling();
    pollTimer = window.setTimeout(pollOnce, 2000);
  }

  /* ── 模型选择器：点外面 / Esc 关闭；同一层只展开一个分组 ────────────────
     外层是 `<details>`（面板），内层两个 `<details>`（模型 / 推理等级）。
     没有这段脚本也能用：summary 自己可开合、菜单项是真表单。 */
  const bindModelPicker = () => {
    const picker = document.querySelector('.ada-model-picker');
    if (!picker || picker.dataset.adaBound === '1') return;
    picker.dataset.adaBound = '1';
    const groups = picker.querySelectorAll('.ada-model-picker__group');
    groups.forEach((group) => {
      group.addEventListener('toggle', () => {
        if (!group.open) return;
        groups.forEach((other) => {
          if (other !== group) other.open = false;
        });
      });
    });
    picker.addEventListener('toggle', () => {
      if (picker.open) return;
      groups.forEach((group) => {
        group.open = false;
      });
    });
  };
  bindModelPicker();
  document.addEventListener('click', (event) => {
    document.querySelectorAll('details.ada-model-picker[open]').forEach((picker) => {
      if (!picker.contains(event.target)) picker.open = false;
    });
  });
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    document.querySelectorAll('details.ada-model-picker[open]').forEach((picker) => {
      picker.open = false;
    });
  });

  let source = null;
  /* 只有**真的收到事件**才算流可用。
     不能在 `open` 里置 true：SSE 的 200 头在服务端连上网关之前就发出去了，
     网关挂着时 `open` 也会触发，于是"假在线"会把兜底轮询关掉（实测断网时正是这样）。 */
  const markLive = () => {
    window.__adaStream = true;
    stopPolling();
  };
  const connect = () => {
    if (source) source.close();
    source = new EventSource('/threads/' + encodeURIComponent(threadId) + '/stream');
    source.addEventListener('transcript', (event) => {
      markLive();
      replaceTranscript(event.data);
    });
    source.addEventListener('item', (event) => {
      markLive();
      applyItem(event.data);
    });
    source.addEventListener('parts', (event) => {
      markLive();
      try {
        applyParts(JSON.parse(event.data));
      } catch (error) {
        /* 坏帧不拖垮页面：下一次 parts（500ms 内）会覆盖 */
      }
    });
    source.addEventListener('error', () => {
      // EventSource 自己会重连；期间用 2 秒轮询兜底。
      // 只在没有计时器时排下一轮：反复的 error 不该把轮询一直往后推。
      window.__adaStream = false;
      if (!pollTimer) schedulePoll();
    });
  };

  if (window.EventSource) {
    connect();
  }
  // 首屏先排一次兜底：SSE 在几百毫秒内连上时，`open` 会把它取消。
  schedulePoll();
})();
