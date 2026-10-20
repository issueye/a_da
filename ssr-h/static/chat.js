/*
 * 聊天页专属交互。
 *
 * 组件级行为（折叠状态、复制、输入框聚焦与自动增高）在 components.js 里，
 * 这里只保留与"对话流"强相关的部分：滚动跟随、代码块复制、侧栏搜索。
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
    scrollToBottom();
  };

  document.body.addEventListener('htmx:afterSwap', (event) => {
    if (event.target && event.target.id === 'transcript') {
      enhanceCodeBlocks();
      if (stickToBottom) scrollToBottom();
    }
  });

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();

/* 侧栏会话搜索：纯前端过滤标题，不改变当前会话。 */
(() => {
  const initThreadSearch = () => {
    const input = document.getElementById('chat-thread-search');
    if (!input) return;
    const items = [...document.querySelectorAll('.ada-thread')];
    input.addEventListener('input', () => {
      const query = input.value.trim().toLowerCase();
      for (const item of items) {
        item.hidden = Boolean(query) && !item.textContent.toLowerCase().includes(query);
      }
    });
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initThreadSearch);
  } else {
    initThreadSearch();
  }
})();
