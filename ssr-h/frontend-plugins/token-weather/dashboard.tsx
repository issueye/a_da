import { Chip } from "ssr-h:components";
import { bandOf, percentOf, tokensOf } from "./components/weather";

/** 概览页卡片：同一套分档，省掉明细，只留一眼能看懂的天气。 */
export function Page(props: any) {
  const tokens = tokensOf(props);
  const percent = percentOf(tokens);
  const band = bandOf(percent);
  const used = tokens && tokens.used ? tokens.used : 0;
  const windowSize = tokens && tokens.window ? tokens.window : 0;

  return (
    <div class="flex items-center gap-3">
      <span class="text-2xl" aria-hidden="true">{band.icon}</span>
      <div class="flex-1">
        <div class="flex items-center gap-2">
          <span class="text-sm font-medium text-ink">{band.label}</span>
          <Chip text={percent === null ? "无数据" : percent + "%"} tone={band.tone} />
        </div>
        <p class="mt-1 text-[11px] text-muted">
          {percent === null
            ? "还没有用量数据"
            : used + " / " + windowSize + " tokens"}
        </p>
      </div>
    </div>
  );
}
