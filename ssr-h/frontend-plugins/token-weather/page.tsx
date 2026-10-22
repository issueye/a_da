import { Chip, Notice } from "ssr-h:components";
import { bandOf, percentOf, tokensOf } from "./components/weather";

/**
 * Token 天气完整页：把"当前会话的上下文用量"渲染成天气。
 *
 * 数据来源是服务端注入的 `props.data.tokens`（`promptTokens ÷ contextWindow`），
 * 插件自己不算 token；每 5 秒用 HTMX 把本节点整块换一次。
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
  const pct = percent === null ? "—" : percent + "%";

  return (
    <div
      id="token-weather"
      class={"tw-page tw-" + band.key}
      hx-get={href}
      hx-trigger="every 5s"
      hx-target="#token-weather"
      hx-swap="outerHTML"
      hx-select="#token-weather"
    >
      <section class="tw-hero">
        <div class="tw-hero__glow" aria-hidden="true"></div>
        <span class="tw-hero__orb" aria-hidden="true">{band.icon}</span>
        <div class="tw-hero__body">
          <div class="tw-kicker">CONTEXT WEATHER</div>
          <h2 class="tw-hero__title">{band.label}</h2>
          <p class="tw-hero__advice">{band.advice}</p>
        </div>
        <Chip text={percent === null ? "无数据" : pct} tone={band.tone} />
      </section>

      <section class="tw-card">
        <div class="tw-meter">
          <div class="tw-meter__meta">
            <span>上下文用量</span>
            <strong>{percent === null ? "暂无数据" : used + " / " + windowSize + " tokens"}</strong>
          </div>
          <progress class="tw-progress" value={percent === null ? 0 : percent} max="100"></progress>
        </div>

        <div class="tw-stats">
          <div class="tw-stat">
            <span class="tw-stat__label">已用</span>
            <strong class="tw-stat__value">{used}</strong>
            <span class="tw-stat__unit">tokens</span>
          </div>
          <div class="tw-stat">
            <span class="tw-stat__label">窗口</span>
            <strong class="tw-stat__value">{windowSize > 0 ? windowSize : "—"}</strong>
            <span class="tw-stat__unit">tokens</span>
          </div>
          <div class="tw-stat">
            <span class="tw-stat__label">已用比例</span>
            <strong class="tw-stat__value">{pct}</strong>
          </div>
          <div class="tw-stat tw-stat--wide">
            <span class="tw-stat__label">会话</span>
            <span class="tw-stat__text" title={threadTitle}>{threadTitle}</span>
          </div>
          <div class="tw-stat tw-stat--wide">
            <span class="tw-stat__label">模型</span>
            <span class="tw-stat__text" title={model}>{model}</span>
          </div>
        </div>
      </section>

      {band.key === "storm" ? (
        <Notice
          text="上下文已超过 85%：这一轮很可能撞 ContextWindowExceeded，先压缩历史或新开会话。"
          tone="danger"
        />
      ) : null}

      <p class="tw-note tw-note--page">
        口径：最近一次模型调用的 promptTokens ÷ contextWindow（与引擎的上下文治理同源）。
        数据由服务端注入 props.data.tokens，插件只负责显示。
      </p>
    </div>
  );
}

