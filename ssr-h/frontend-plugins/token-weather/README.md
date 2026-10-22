# Token 天气（示例前端插件）

这是一个**示例插件**，不是产品功能。它演示了 ssr-h 动态前端插件能做什么：

- **一个插件、三种投影**：
  - `page.tsx` → 独立页面 `/ext/token-weather/weather`（完整用量明细）；
  - `dashboard.tsx` → 概览页卡片；
  - `slot.tsx` → `composer.above` 插槽里的天气胶囊，点击展开**浮层**（不跳页）。
- **数据来自服务端**：`props.data.tokens`（最近一次模型调用的 `promptTokens ÷ contextWindow`），
  插件本身不发请求、不读文件——Boa 运行时没有 fs/net。
- **实时刷新**：聊天页的 `<details class="ada-popover">` 由 SSE `parts` 每 500ms 重渲染一次；
  `chat.js` 会按 `id` 还原展开状态，点外部 / `Esc` 关闭，所以浮层不会被更新关掉。
- **相对 import**：`slot.tsx` / `page.tsx` 共用 `components/weather.tsx` 里的分档规则，
  演示插件目录内的相对模块加载。

## 文件

| 文件 | 作用 |
|---|---|
| `plugin.json` | 清单：导航、页面、仪表盘、`composer.above` 插槽 |
| `page.tsx` | 完整页面（天气 + 用量明细） |
| `dashboard.tsx` | 概览卡片 |
| `slot.tsx` | 输入框上方的天气胶囊 / 浮层 |
| `components/weather.tsx` | 分档规则（0–30 晴 / 31–60 多云 / 61–85 阴 / >85 暴雨） |

## 启用

插件在 ssr-h 启动时扫描；改完模板后到 `/extensions` 点「重载前端插件」，或重启 ssr-h。

> 直接用 `dist/ssr-h.exe` 运行时，默认读的是 exe 旁的 `dist/frontend-plugins/`。
> 想让本目录生效，启动时加 `--plugins-dir ssr-h/frontend-plugins`，或把本目录同步到 `dist/frontend-plugins/`。

## 口径

百分比 = 最近一次模型调用的 `promptTokens ÷ contextWindow`，与引擎的上下文治理同源；
窗口未配置（`contextWindow = 0`）时显示「无数据」，不拿 0% 冒充。
