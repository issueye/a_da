/**
 * GET /ext/hello-tsx/api/info —— 插件自带 JSON API 的最小示例。
 *
 * 🔴 API 模板的入口函数**返回什么就序列化什么**（不走 JSX runtime 的 `h`）：
 * 在这里写 JSX 得到的是 VNode 对象，不是渲染结果。props 与页面同源
 * （plugin/app/route/data），请求面在 `props.api`（method/path/query/body）。
 * 沙箱预算与 HTML 渲染同一套，仍然没有 fs/net/process。
 */
export function Page(props: any) {
  const query = (props && props.api && props.api.query) || {};
  const data = (props && props.data) || {};
  return {
    ok: true,
    plugin: (props.plugin && props.plugin.name) || "hello-tsx",
    version: (props.plugin && props.plugin.version) || "0.1.0",
    query: query,
    serverAt: (data.render && data.render.at) || null,
    hint: "这是插件在服务端（Boa 沙箱内，无网络/文件）算出来的 JSON",
  };
}
