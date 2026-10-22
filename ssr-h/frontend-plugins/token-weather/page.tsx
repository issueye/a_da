import { Chip, Notice, Panel } from "ssr-h:components";
import { bandOf, percentOf, tokensOf } from "./components/weather";

/**
 * Token 天气：把"当前会话的上下文用量"渲染成天气。
 *
 * 数据来源是服务端注入的 `props.data.tokens`（`promptTokens ÷ contextWindow`），
 * 插件自己不算 token——Boa 里没有 fs/net，也拿不到网关。
 * 每 5 秒用 HTMX 把本节点整块换一次：`hx-select` 从整页里取回 `#token-weather`。
 */
export function Page(props: any) {
  const tokens = tokensOf(props);
  const percent = percentOf(tokens);
  const band = bandOf(percent);
  const used = tokens && tokens.used ? tokens.used : 0;
  const windowSize = tokens && tokens.window ? tokens.window : 0;
  const threadTitle = tokens && tokens.threadTitle ? tokens.threadTitle : "（没有活动会话）";
  const model = tokens && tokens.model ? tokens.model : "未知";
  const slug = props.page && props.page.slug ? props.page.slug : "weather";
  const href = "/ext/" + props.plugin.id + "/" + slug;

  return (
    <div
      id="token-weather"
      class="space-y-3"
      hx-get={href}
      hx-trigger="every 5s"
      hx-target="#token-weather"
      hx-swap="outerHTML"
      hx-select="#token-weather"
    >
      <div class="flex items-center gap-3">
        <span class="text-2xl" aria-hidden="true">{band.icon}</span>
        <div class="flex-1">
          <div class="text-lg font-semibold text-ink">{band.label}</div>
          <div class="text-xs text-muted">{band.advice}</div>
        </div>
        <Chip text={percent === null ? "无数据" : percent + "%"} tone={band.tone} />
      </div>

      <progress class="w-full" value={percent === null ? 0 : percent} max="100"></progress>

      <Panel title="用量明细">
        <div class="space-y-2 text-xs text-muted">
          <p>已用：{used} tokens</p>
          <p>
            窗口：
            {windowSize > 0 ? windowSize + " tokens" : "未配置（config.contextWindow = 0）"}
          </p>
          <p>会话：{threadTitle}</p>
          <p>模型：{model}</p>
        </div>
      </Panel>

      {band.key === "storm" ? (
        <Notice
          text="上下文已超过 85%：这一轮很可能撞 ContextWindowExceeded，先压缩历史或新开会话。"
          tone="danger"
        />
      ) : null}

      <p class="text-[11px] text-faint">
        口径：最近一次模型调用的 promptTokens ÷ contextWindow（与引擎的上下文治理同源）。
        数据由服务端注入 props.data.tokens，插件只负责显示。
      </p>
    </div>
  );
}
