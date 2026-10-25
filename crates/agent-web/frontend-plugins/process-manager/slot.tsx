import { Icon } from "ssr-h:components";

/**
 * 顶栏进程管理浮窗（`topbar.right` 插槽）。
 *
 * 数据来自宿主的 `props.data.processes`（只有声明 `processes.read` 的插件才会拿到）：
 * `{ status, agents, count, error }`。进程列表由网关的 `gateway.status` /
 * `gateway.listAgents` 投影而来，端点已经剥掉 token。
 *
 * 没有进程数据（插件刚装好、网关未连接）时返回 null：不画一个空面板。
 * 「刷新」是普通链接，整页重载后宿主会重新取一次进程数据——插件没有网络能力，
 * 不需要也不应该自己发请求。
 */
function workspaceName(path: any): string {
  const parts = String(path || "")
    .split(/[\\/]/)
    .filter(function (part: string) {
      return !!part;
    });
  return parts.length ? parts[parts.length - 1] : String(path || "—");
}

function ageLabel(ms: any, now: any): string {
  const value = Number(ms || 0);
  const current = Number(now || 0);
  if (!value || !current) return "—";
  const delta = Math.max(0, current - value);
  if (delta < 60000) return "刚刚";
  if (delta < 3600000) return Math.floor(delta / 60000) + " 分钟前";
  if (delta < 86400000) return Math.floor(delta / 3600000) + " 小时前";
  return Math.floor(delta / 86400000) + " 天前";
}

function statusTone(status: string): string {
  if (status === "ready") return "success";
  if (status === "starting") return "warn";
  if (status === "unhealthy") return "danger";
  return "";
}

function statusLabel(status: string): string {
  if (status === "ready") return "就绪";
  if (status === "starting") return "启动中";
  if (status === "unhealthy") return "异常";
  return "已停止";
}

export function Page(props: any) {
  const processes =
    props && props.data && props.data.processes ? props.data.processes : null;
  if (!processes) return null;

  const status = processes.status || null;
  const agents = Array.isArray(processes.agents) ? processes.agents : [];
  const now = props.data && props.data.render ? props.data.render.at : 0;
  const error = processes.error || "";
  const gatewayWs = status ? String(status.defaultWorkspace || "") : "";

  return (
    <details class="pm-popover" id="process-manager">
      <summary
        class="ada-icon-btn pm-trigger"
        aria-label="进程管理"
        title="进程管理（网关与 agent 实例）"
      >
        <Icon name="i-server" size={15} />
      </summary>

      <div class="pm-panel" role="dialog" aria-label="进程管理">
        <div class="pm-head">
          <span class="pm-title">
            <Icon name="i-server" size={14} />
            <span>进程管理</span>
          </span>
          <a class="pm-refresh" href="" title="重新读取进程列表">
            <Icon name="i-refresh" size={12} />
            <span>刷新</span>
          </a>
        </div>

        {error ? <div class="pm-error">读取进程失败：{error}</div> : null}

        {status ? (
          <div class="pm-gateway">
            <div class="pm-gateway-line">
              <strong>ada-gateway</strong>
              <span class="ada-chip ada-chip--success">
                <span class="ada-dot ada-dot--success"></span>运行中
              </span>
            </div>
            <div class="pm-facts">
              <span>PID {status.pid || "—"}</span>
              <span>实例 {status.agentCount || 0}</span>
              <span>可路由 {status.routableCount || 0}</span>
            </div>
            <div class="pm-workspace" title={gatewayWs}>
              {workspaceName(gatewayWs)}
            </div>
          </div>
        ) : null}

        <div class="pm-section-title">AGENT 实例</div>
        {agents.length === 0 ? (
          <div class="pm-empty">当前没有 agent 实例；发送消息时会按工作区自动拉起。</div>
        ) : (
          agents.map(function (agent: any) {
            const tone = statusTone(String(agent.status || ""));
            const label = statusLabel(String(agent.status || ""));
            const workspace = String(agent.workspace || "");
            return (
              <div class="pm-row" key={agent.id || agent.pid || workspace}>
                <span class="pm-row-icon">
                  <Icon name="i-bot" size={15} />
                </span>
                <div class="pm-row-main">
                  <div class="pm-row-line">
                    <span class="pm-name">{agent.product || "ada-coding"}</span>
                    {tone ? (
                      <span class={"ada-chip ada-chip--" + tone}>{label}</span>
                    ) : (
                      <span class="ada-chip">{label}</span>
                    )}
                  </div>
                  <div class="pm-meta" title={workspace}>
                    <span class="pm-ws">{workspaceName(workspace)}</span>
                    <span>· PID {agent.pid || "—"}</span>
                    <span>· {ageLabel(agent.lastActiveMs, now)}</span>
                  </div>
                  <div class="pm-endpoint" title={agent.endpoint || ""}>
                    {agent.endpoint || "—"}
                  </div>
                </div>
              </div>
            );
          })
        )}

        <div class="pm-hint">
          只读视图：实例由网关按工作区按需拉起，空闲会自动回收。
        </div>
      </div>
    </details>
  );
}
