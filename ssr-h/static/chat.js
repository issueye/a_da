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
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();

/* 侧栏会话搜索：纯前端过滤标题，不改变当前会话。
   live 轮询会重渲染 #thread-list，因此要重新套用过滤并保住滚动位置。 */
(() => {
  const initThreadSearch = () => {
    const input = document.getElementById('chat-thread-search');
    const list = document.getElementById('thread-list');
    if (!input) return;

    let listScroll = 0;
    const apply = () => {
      const query = input.value.trim().toLowerCase();
      document.querySelectorAll('.ada-thread').forEach((item) => {
        item.hidden = Boolean(query) && !item.textContent.toLowerCase().includes(query);
      });
    };

    input.addEventListener('input', apply);

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
