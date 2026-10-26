/*
 * 主题引导（阻塞式加载，必须早于样式表求值）
 *
 * 规则与 原桌面端 一致：
 *  - localStorage 里有 a_da_theme 就以它为准；
 *  - 没有记录则跟随系统 prefers-color-scheme；
 *  - 类名只切 html.dark / html.light，颜色通过 CSS 变量整体翻转。
 *
 * 单独成文件（而不是内联 <script>）是为了让 Rust 的通用错误页也能复用同一份逻辑，
 * 避免两处各写一遍主题判断而漂移。
 */
(function () {
  'use strict';

  var root = document.documentElement;
  var saved = null;
  try {
    saved = localStorage.getItem('a_da_theme');
  } catch (_) {
    /* 隐私模式下 localStorage 可能抛错，忽略即可 */
  }
  var prefersDark =
    window.matchMedia &&
    window.matchMedia('(prefers-color-scheme: dark)').matches;
  var dark = saved === 'dark' || (saved !== 'light' && !!prefersDark);

  root.classList.toggle('dark', dark);
  root.classList.toggle('light', !dark);

  function label(isDark) {
    return isDark ? '切换到亮色模式' : '切换到暗色模式';
  }

  function syncButtons() {
    var isDark = root.classList.contains('dark');
    var buttons = document.querySelectorAll('[data-theme-toggle]');
    for (var i = 0; i < buttons.length; i += 1) {
      buttons[i].setAttribute('aria-label', label(isDark));
      buttons[i].setAttribute('title', label(isDark));
      buttons[i].setAttribute('aria-pressed', isDark ? 'true' : 'false');
    }
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    var button = target.closest('[data-theme-toggle]');
    if (!button) return;

    var next = !root.classList.contains('dark');
    root.classList.toggle('dark', next);
    root.classList.toggle('light', !next);
    try {
      localStorage.setItem('a_da_theme', next ? 'dark' : 'light');
    } catch (_) {
      /* 忽略持久化失败：本次会话内主题仍然生效 */
    }
    syncButtons();
  });

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', syncButtons);
  } else {
    syncButtons();
  }
})();
