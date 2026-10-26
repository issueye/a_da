/* 任务卡片（task-plan 插件）的浏览器端增强。
 *
 * 只做两件事，服务端渲染的卡片在没有本脚本时也能开合：
 * 1. 记住展开/收起偏好（localStorage，键与 原桌面端 同口径）；
 * 2. live 轮询 / SSE 会整块替换插槽内容，替换后把偏好重新应用回去。
 *
 * 复制按钮本身交给 components.js 的 `[data-copy]` 委托；这里只拦住
 * "点复制顺带把 summary 开合"的默认行为（不要 stopPropagation——那会让复制不执行）。
 */
(function () {
  'use strict';

  var PREF_KEY = 'a_da_todo_panel_collapsed';

  function readPref() {
    try {
      return window.localStorage.getItem(PREF_KEY);
    } catch (error) {
      return null;
    }
  }

  function writePref(collapsed) {
    try {
      window.localStorage.setItem(PREF_KEY, collapsed ? '1' : '0');
    } catch (error) {
      /* 隐私模式下不可用：忽略即可 */
    }
  }

  function bindCard(card) {
    if (!card || card.dataset.tpBound === '1') return;
    card.dataset.tpBound = '1';
    // 上次是展开态 → 这次直接展开（插槽被轮询替换后同样适用）。
    if (readPref() === '0') card.open = true;
    card.setAttribute('data-state', card.open ? 'expanded' : 'collapsed');
    card.addEventListener('toggle', function () {
      card.setAttribute('data-state', card.open ? 'expanded' : 'collapsed');
      writePref(!card.open);
    });
  }

  function bindAll() {
    var cards = document.querySelectorAll('.tp-card');
    for (var i = 0; i < cards.length; i += 1) bindCard(cards[i]);
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    if (target.closest('.tp-card__copy')) event.preventDefault();
  });

  // 两个可能的挂载点都观察：插件可以在 chat.corner，也可以在 composer.above。
  function watch() {
    bindAll();
    if (!window.MutationObserver) return;
    var docks = ['chat-corner', 'composer-above'];
    for (var i = 0; i < docks.length; i += 1) {
      var dock = document.getElementById(docks[i]);
      if (!dock || dock.dataset.tpWatched === '1') continue;
      dock.dataset.tpWatched = '1';
      new MutationObserver(bindAll).observe(dock, { childList: true, subtree: true });
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', watch);
  } else {
    watch();
  }
})();
