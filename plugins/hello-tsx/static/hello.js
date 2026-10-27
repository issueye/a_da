/*
 * hello-tsx 的浏览器增强：演示「插件 JS 调插件自己的 API」。
 *
 * CSP 是 connect-src 'self'——插件 JS 能 fetch 的就是 ssr-h 自己（同源），
 * 插件 API 端点 /ext/{id}/api/* 正好在其中。这里把服务端返回的 JSON 写进
 * 插槽药丸的 title，悬停即可看到一次完整的「浏览器 → 宿主分发 → Boa 沙箱」往返。
 */
(() => {
  'use strict';

  const chip = document.querySelector('[data-hello-api]');
  if (!chip) return;
  fetch('/ext/hello-tsx/api/info?from=browser')
    .then((res) => (res.ok ? res.json() : Promise.reject(new Error('HTTP ' + res.status))))
    .then((data) => {
      chip.title = '插件 API 返回：' + JSON.stringify(data);
    })
    .catch((error) => {
      chip.title = '插件 API 调用失败：' + error.message;
    });
})();
