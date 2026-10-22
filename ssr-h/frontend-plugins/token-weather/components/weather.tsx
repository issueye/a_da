//! Token 天气的分档规则（页面与仪表盘卡片共用）。
//!
//! 阈值按需求固定：0-30% 晴天 / 31-60% 多云 / 61-85% 阴天 / >85% 暴雨。
//! `percent` 由服务端算好（`promptTokens ÷ contextWindow`），拿不到时是 `null`。

type Band = {
  max: number;
  key: string;
  label: string;
  icon: string;
  tone: string;
  advice: string;
};

const BANDS: Band[] = [
  {
    max: 30,
    key: "sunny",
    label: "晴天",
    icon: "☀️",
    tone: "success",
    advice: "上下文很宽松，长日志、长文件可以放心贴。",
  },
  {
    max: 60,
    key: "cloudy",
    label: "多云",
    icon: "⛅",
    tone: "brand",
    advice: "用掉一半了：工具输出尽量精简，别整份往上下文里倒。",
  },
  {
    max: 85,
    key: "overcast",
    label: "阴天",
    icon: "☁️",
    tone: "warn",
    advice: "快到顶了：先压缩历史，或者开一个新会话。",
  },
  {
    max: 100,
    key: "storm",
    label: "暴雨",
    icon: "🌧️",
    tone: "danger",
    advice: "随时会撞上下文窗口：立刻压缩或新开会话，否则这一轮会失败。",
  },
];

const UNKNOWN: Band = {
  max: 100,
  key: "unknown",
  label: "未知",
  icon: "🌫️",
  tone: "neutral",
  advice: "还没拿到用量：先在对话里说一句话，或者检查网关是否连上。",
};

/** 取当前会话的 `props.data.tokens`（没有就返回 `null`）。 */
function tokensOf(props: any): any {
  return props && props.data && props.data.tokens ? props.data.tokens : null;
}

/** `percent` 是 0-100 的整数；服务端没算出来时是 `null`。 */
function percentOf(tokens: any): any {
  return tokens && typeof tokens.percent === "number" ? tokens.percent : null;
}

/** 百分比 → 天气档位；`null` 走"未知"。 */
function bandOf(percent: any): Band {
  if (typeof percent !== "number" || percent < 0) {
    return UNKNOWN;
  }
  for (let i = 0; i < BANDS.length; i += 1) {
    if (percent <= BANDS[i].max) {
      return BANDS[i];
    }
  }
  return BANDS[BANDS.length - 1];
}

export { BANDS, UNKNOWN, tokensOf, percentOf, bandOf };
