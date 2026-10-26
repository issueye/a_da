// 统一插件示例的前端插槽。
//
// props.data.backend.<pluginId>.<binding> 是宿主预取的只读动作结果；
// 写入动作**不会**预取，必须由下面的表单 POST 触发。

export function Page(props: any) {
  const backend = props.data && props.data.backend;
  const group = backend && backend["unified-demo"];
  const state = group && group["status"];
  const data = state && state.ok ? state.data : null;
  const error = state && state.error ? state.error : "后端动作不可用";
  const badge = data ? data.count + " 次刷新" : "未连接";
  let text = error;
  if (data) {
    text = "后端状态：" + data.count + " 次刷新";
    if (data.lastRefresh) {
      text = text + "，最近一次 " + data.lastRefresh;
    }
  }

  return (
    <details class="ud-panel">
      <summary class="ud-panel__summary">
        <span>统一插件</span>
        <span class="ud-panel__badge">{badge}</span>
      </summary>
      <div class="ud-panel__body">
        <p class="ud-panel__text">{text}</p>
        <form method="post" action="/ext/unified-demo/backend/refresh">
          <button class="ud-panel__button" type="submit">刷新后端</button>
        </form>
        <p class="ud-panel__text">
          <a href="/ext/unified-demo/route/hello/ada?q=demo" target="_blank" rel="noreferrer">
            动态路由示例
          </a>
        </p>
      </div>
    </details>
  );
}
