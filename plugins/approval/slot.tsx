// 审批面板（thread.approval 插槽）。
//
// 待批准列表优先来自同包后端插件（beforeApproval/afterApproval 维护的状态）；
// 后端停用/未就绪时回落到核心投影 props.data.approvals。
// 批准/拒绝仍然提交核心路由 /approvals/{callId}/decision——插件只负责呈现。

export function Page(props: any) {
  const coreApprovals = props && props.data && Array.isArray(props.data.approvals)
    ? props.data.approvals
    : [];
  const backendGroup = props && props.data && props.data.backend
    ? props.data.backend["approval"]
    : null;
  const backendPending = backendGroup ? backendGroup.pending : null;
  const backendData = backendPending && backendPending.ok ? backendPending.data : null;
  const backendItems = backendData && Array.isArray(backendData.items) ? backendData.items : [];

  // 核心 `data.approvals` 决定“现在到底有哪些待批准项”（避免 abort/timeout 后
  // 后端状态残留）；后端 action 只用来补 args/waited 与统计。
  const byCallId: any = {};
  for (let i = 0; i < backendItems.length; i += 1) {
    const item = backendItems[i];
    if (item && item.callId) byCallId[String(item.callId)] = item;
  }
  const items = coreApprovals.map(function (item: any) {
    const key = String(item.callId || "");
    return byCallId[key] || item;
  });
  if (!items || items.length === 0) return null;

  const threadId = props && props.data && props.data.thread && props.data.thread.id
    ? props.data.thread.id
    : "";
  const approved = backendData && typeof backendData.approved === "number" ? backendData.approved : 0;
  const rejected = backendData && typeof backendData.rejected === "number" ? backendData.rejected : 0;

  const cards = items.map(function (item: any) {
    const callId = String(item.callId || "");
    const tool = String(item.tool || item.toolName || "工具");
    const rawArgs = item.args === undefined || item.args === null
      ? ""
      : (typeof item.args === "string" ? item.args : JSON.stringify(item.args, null, 2));
    const waited = String(item.waited || "");
    return (
      <div class="ada-tray ada-tray--approval" data-plugin-approval-call={callId}>
        <div class="ada-tray__head">
          <span class="ada-tray__icon"><Icon name="i-shield-check" size={14} /></span>
          <span class="ada-tray__title">需要批准 · {tool}</span>
          {waited ? <span class="ada-tray__meta">{waited}</span> : null}
        </div>
        {rawArgs
          ? <pre class="ada-tray__args">{rawArgs}</pre>
          : <span class="ada-tray__empty">（这个工具没有提供参数预览）</span>}
        <div class="ada-tray__actions">
          <form method="post" action={"/approvals/" + callId + "/decision"} class="ada-tray__form" data-partial>
            <input type="hidden" name="thread_id" value={threadId} />
            <button class="ada-btn ada-btn--primary ada-btn--sm" type="submit" name="approved" value="true">
              <Icon name="i-check" size={12} />批准
            </button>
            <button class="ada-btn ada-btn--secondary ada-btn--sm" type="submit" name="approved" value="false">
              <Icon name="i-x" size={12} />拒绝
            </button>
          </form>
        </div>
      </div>
    );
  });

  return (
    <div class="ada-approval-plugin">
      {items.length > 1 ? <div class="ada-trays__summary">等待批准 ×{items.length}</div> : null}
      {cards}
      {approved + rejected > 0
        ? <div class="ada-approval-plugin__stats">本会话已批准 {approved} · 已拒绝 {rejected}</div>
        : null}
    </div>
  );
}
