import { Button } from "ssr-h:components";
import { bandOf, percentOf, tokensOf } from "./components/weather";

/**
 * 输入框上方的天气胶囊（`composer.above` 插槽）。
 *
 * 点击不再跳页，而是展开一个浮动小窗（`<details>` + 绝对定位面板）：
 * 天气分档、百分比、已用/窗口、会话与模型都在这里看；完整页面仍保留入口。
 *
 * 面板在每次 `parts` 刷新时会被整块替换，`chat.js` 会按 `id` 还原 `open`，
 * 所以展开状态不会被 500ms 一次的更新关掉。
 */
export function Page(props: any) {
  const tokens = tokensOf(props);
  const percent = percentOf(tokens);
  const band = bandOf(percent);
  const used = tokens && tokens.used ? tokens.used : 0;
  const windowSize = tokens && tokens.window ? tokens.window : 0;
  const threadTitle = tokens && tokens.threadTitle ? tokens.threadTitle : "（没有活动会话）";
  const model = tokens && tokens.model ? tokens.model : "未知";
  const href = "/ext/token-weather/weather";
  const label = percent === null ? "Token 用量" : band.label + " · " + percent + "%";
  const detail =
    percent === null
      ? "还没有用量数据：先在对话里说一句，或检查网关是否连上"
      : used + " / " + windowSize + " tokens · " + band.advice;

  return (
    <details class="ada-popover" id="token-weather-popover">
      <summary class={"ada-chip ada-chip--" + band.tone} title={detail} aria-label="Token 天气详情">
        <span aria-hidden="true">{band.icon}</span>
        <span class="ada-chip__text">{label}</span>
      </summary>

      <div class="ada-popover__panel">
        <div class="flex items-center gap-3">
          <span class="text-2xl" aria-hidden="true">{band.icon}</span>
          <div class="flex-1">
            <div class="text-sm font-semibold text-ink">{label}</div>
            <div class="text-xs text-muted">{band.advice}</div>
          </div>
        </div>

        <progress class="mt-2 w-full" value={percent === null ? 0 : percent} max="100"></progress>

        <div class="mt-2 space-y-2 text-xs text-muted">
          <p>已用：{used} tokens</p>
          <p>窗口：{windowSize > 0 ? windowSize + " tokens" : "未配置（contextWindow = 0）"}</p>
          <p>会话：{threadTitle}</p>
          <p>模型：{model}</p>
        </div>

        <p class="mt-2 text-[11px] text-faint">
          口径：最近一次模型调用的 promptTokens ÷ contextWindow；随对话实时刷新。
        </p>

        <div class="mt-2 flex justify-end">
          <Button label="完整页面" href={href} variant="ghost" size="sm" icon="i-external" />
        </div>
      </div>
    </details>
  );
}
