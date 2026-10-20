/*
 * 插件自带的增强脚本。
 *
 * 约定：
 * - 插件的 JS 失败绝不能影响主应用 —— 所有逻辑都包在 try/catch 里，
 *   读不到数据就保持占位文案，不抛错、不阻断其它脚本。
 * - 数据从服务端已有的 `/api/plugins` 取，服务端与客户端看到同一份事实。
 */
(function () {
  'use strict';

  function render() {
    var node = document.querySelector('[data-float-status]');
    if (!node) return;
    if (!window.fetch) {
      node.textContent = '当前环境不支持读取插件状态。';
      return;
    }
    window
      .fetch('/api/plugins', { headers: { Accept: 'application/json' } })
      .then(function (response) {
        return response.ok ? response.json() : null;
      })
      .then(function (data) {
        if (!data) {
          node.textContent = '暂时读不到插件状态。';
          return;
        }
        var plugins = data.plugins || [];
        var active = plugins.filter(function (plugin) {
          return plugin.enabled && plugin.status === 'ready';
        }).length;
        var slots = (data.slots || []).filter(function (slot) {
          return slot.contributors > 0;
        }).length;
        node.innerHTML =
          '当前 <b>' + active + '</b> 个前端插件已挂载，正在 <b>' + slots + '</b> 个插槽上贡献内容。';
      })
      .catch(function () {
        node.textContent = '插件状态读取失败（不影响主界面）。';
      });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', render);
  } else {
    render();
  }
})();
