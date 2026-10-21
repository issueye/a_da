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
  let stickToBottom = true;

  const nearBottom = () => {
    const el = scroller();
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  };

  const scrollToBottom = () => {
    const el = scroller();
    if (el) el.scrollTop = el.scrollHeight;
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
    const el = scroller();
    if (el) {
      el.addEventListener('scroll', () => { stickToBottom = nearBottom(); }, { passive: true });
    }
    const transcript = document.getElementById('transcript');
    if (transcript && window.MutationObserver) {
      new MutationObserver(() => {
        enhanceCodeBlocks();
        if (stickToBottom) scrollToBottom();
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
