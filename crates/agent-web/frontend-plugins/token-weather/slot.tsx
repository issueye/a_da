import { Button, Icon } from "ssr-h:components";
import {
  bandOf,
  cacheOf,
  formatTokens,
  percentOf,
  percentText,
  tokensOf,
} from "./components/weather";

/**
 * 输入框上方的天气胶囊（`composer.above` 插槽）。
 *
 * 点击展开浮动小窗（`<details>` + 绝对定位面板）：天气分档、百分比、
 * 已用/窗口、会话与模型都在这里看；完整页面仍保留入口。
 * 面板在 SSE `parts` 刷新时会被整块替换，`chat.js` 按 `id` 还原 `open`。
 */
export function Page(props: any) {
  const tokens = tokensOf(props);
  const percent = percentOf(tokens);
  const band = bandOf(percent);
  const used = tokens && tokens.used ? tokens.used : 0;
  const windowSize = tokens && tokens.window ? tokens.window : 0;
  const completion = tokens && tokens.completion ? tokens.completion : 0;
  const cache = cacheOf(tokens);
  const usedText = formatTokens(used);
  const windowText = windowSize > 0 ? formatTokens(windowSize) : "—";
  const threadTitle = tokens && tokens.threadTitle ? tokens.threadTitle : "（没有活动会话）";
  const model = tokens && tokens.model ? tokens.model : "未知";
  const href = "/ext/token-weather/weather";
  const label = percent === null ? "Token 用量" : band.label + " · " + percent + "%";
  const pct = percentText(percent);
  const detail =
    percent === null
      ? "还没有用量数据：先在对话里说一句，或检查网关是否连上"
      : usedText + " / " + windowText + " tokens · 缓存 " + percentText(cache.percent);

  return (
    <details class={"ada-popover tw-popover tw-" + band.key} id="token-weather-popover">
      <summary class={"ada-chip ada-chip--" + band.tone + " tw-chip"} title={detail} aria-label="Token 天气详情">
        <Icon name={band.icon} size={13} class="tw-chip__icon" />
        <span class="ada-chip__text">{label}</span>
        <Icon name="i-chevron-down" size={11} class="tw-chip__caret" />
      </summary>

      <div class="ada-popover__panel tw-panel">
          <div class="tw-panel__head">
          <span class="tw-panel__orb" aria-hidden="true"><Icon name={band.icon} size={22} /></span>
          <div class="tw-panel__copy">
            <div class="tw-panel__kicker">CONTEXT WEATHER</div>
            <div class="tw-panel__title">
              {band.label}
              <span class="tw-panel__pct">{pct}</span>
            </div>
            <p class="tw-panel__advice">{band.advice}</p>
          </div>
        </div>

        <div class="tw-meter">
          <div class="tw-meter__meta">
            <span>上下文用量</span>
            <span>{percent === null ? "暂无数据" : usedText + " / " + windowText}</span>
          </div>
          <progress class="tw-progress" value={percent === null ? 0 : percent} max="100"></progress>
        </div>

        <div class="tw-stats">
          <div class="tw-stat">
            <span class="tw-stat__label">已用</span>
            <strong class="tw-stat__value" title={used + " tokens"}>{usedText}</strong>
            <span class="tw-stat__unit">tokens</span>
          </div>
          <div class="tw-stat">
            <span class="tw-stat__label">窗口</span>
            <strong class="tw-stat__value" title={windowSize + " tokens"}>{windowText}</strong>
            <span class="tw-stat__unit">tokens</span>
          </div>
          <div class="tw-stat">
            <span class="tw-stat__label">缓存命中</span>
            <strong
              class="tw-stat__value"
              title={
                cache.percent === null
                  ? "模型没有返回 cachedTokens，无法计算命中率"
                  : cache.stale
                    ? "本轮模型未报缓存，数字来自上一次上报缓存的调用（" + cache.cached + " cached tokens）"
                    : cache.cached + " cached tokens"
              }
            >
              {percentText(cache.percent) + (cache.stale ? "*" : "")}
            </strong>
            <span class="tw-stat__unit">{cache.cached > 0 ? formatTokens(cache.cached) : "—"}</span>
          </div>
          <div class="tw-stat">
            <span class="tw-stat__label">输出</span>
            <strong class="tw-stat__value">{formatTokens(completion)}</strong>
            <span class="tw-stat__unit">tokens</span>
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

        <div class="tw-panel__foot">
          <span class="tw-note">缓存 = cached ÷ prompt · 上下文 = prompt ÷ window</span>
          <Button label="完整页面" href={href} variant="ghost" size="sm" icon="i-external" />
        </div>
      </div>
    </details>
  );
}

