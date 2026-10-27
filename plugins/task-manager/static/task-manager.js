/* 任务管理浮窗的浏览器增强（没有它也能用：开合、终止都靠 SSR 出来的结构）。
 *
 * 为什么需要它：`composer.above` 的内容会被 SSE `parts` 整块换新，
 * 换新后 `<pre>` 的 scrollTop 归零——后台任务正在长输出时，读者每 1 秒被顶回开头。
 * 这里按任务 id 记住"滚到哪 / 是否贴着底部"，换新后按记忆还原：
 * 贴着底部（默认）就继续跟最新输出，手动往上翻过就停在原位置。
 *
 * 全部包在 try/catch 里：拿不到节点就什么都不做，不影响主应用。 */
(() => {
  const DOCK_ID = 'composer-above';
  const KEY = 'data-tm-key';
  // 离底部这么近就算"在跟最新输出"（px）。
  const BOTTOM_GAP = 12;
  const memory = new Map();

  const remember = (pre) => {
    const key = pre.getAttribute(KEY);
    if (!key) return;
    const atBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - BOTTOM_GAP;
    memory.set(key, { top: pre.scrollTop, follow: atBottom });
  };

  const restore = (dock) => {
    dock.querySelectorAll('pre[' + KEY + ']').forEach((pre) => {
      const saved = memory.get(pre.getAttribute(KEY) || '');
      // 新任务还没有记忆：直接跟到最新（读者想看的是"现在发生了什么"）。
      pre.scrollTop = !saved || saved.follow ? pre.scrollHeight : saved.top;
    });
  };

  const boot = () => {
    const dock = document.getElementById(DOCK_ID);
    if (!dock) return;
    // scroll 不冒泡，用捕获阶段挂在容器上即可覆盖所有 `<pre>`。
    dock.addEventListener(
      'scroll',
      (event) => {
        const node = event.target;
        if (node && node.tagName === 'PRE') remember(node);
      },
      true,
    );
    let timer = 0;
    const observer = new MutationObserver(() => {
      // 合帧：`parts` 本身就有 500ms 间隔，这里再防一手高频插入。
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        try {
          restore(dock);
        } catch (error) {
          /* 还原失败只影响滚动位置 */
        }
      }, 60);
    });
    observer.observe(dock, { childList: true, subtree: true });
    try {
      restore(dock);
    } catch (error) {
      /* 首帧失败等下一次换新 */
    }
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
