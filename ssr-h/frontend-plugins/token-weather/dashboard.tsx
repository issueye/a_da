import { Chip, Icon } from "ssr-h:components";
import {
  bandOf,
  cacheOf,
  formatTokens,
  percentOf,
  percentText,
  tokensOf,
} from "./components/weather";

/** 概览页卡片：同一套分档，省掉明细，只留一眼能看懂的天气。 */
export function Page(props: any) {
  const tokens = tokensOf(props);
  const percent = percentOf(tokens);
  const band = bandOf(percent);
  const used = tokens && tokens.used ? tokens.used : 0;
  const windowSize = tokens && tokens.window ? tokens.window : 0;
  const cache = cacheOf(tokens);

  return (
    <div class={"tw-dash tw-" + band.key}>
      <span class="tw-dash__orb" aria-hidden="true"><Icon name={band.icon} size={20} /></span>
      <div class="tw-dash__body">
        <div class="tw-dash__head">
          <span class="tw-dash__title">{band.label}</span>
          <Chip text={percent === null ? "无数据" : percent + "%"} tone={band.tone} />
        </div>
        <p class="tw-dash__meta">
          {percent === null
            ? "还没有用量数据"
            : formatTokens(used) + " / " + formatTokens(windowSize) + " tokens · 缓存 " + percentText(cache.percent) + (cache.stale ? "*" : "")}
        </p>
        <progress
          class="tw-progress tw-progress--sm"
          value={percent === null ? 0 : percent}
          max="100"
        ></progress>
      </div>
    </div>
  );
}
