/**
 * POST /ext/hello-tsx/api/echo —— 带请求体的插件 API 示例。
 *
 * 请求体必须是 JSON（超 256KB 或非法 JSON 会被宿主拒绝），经 `props.api.body`
 * 进入模板；这里做一点"服务端才算得了"的事：把文本翻转后回传。
 */
export function Page(props: any) {
  const body = (props && props.api && props.api.body) || {};
  const text = typeof body.text === "string" ? body.text : "";
  const flipped = text ? text.split("").reverse().join("") : "";
  return {
    ok: flipped.length > 0,
    received: body,
    flipped: flipped,
    length: text.length,
  };
}
