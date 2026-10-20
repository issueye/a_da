# Hello Frontend Plugin

这是 `ssr-h` 的最小动态前端插件示例。

- `plugin.json`：声明 id、导航、页面、仪表盘片段与资源目录；
- `dashboard.html`：注入概览页的卡片；
- `page.html`：独立页面 `/ext/hello/hello`；
- `static/hello.js`：由页面加载的插件脚本。

修改任意文件后，在 ssr-h 的「扩展」页点「重载前端插件」即可生效，不需要重启。
