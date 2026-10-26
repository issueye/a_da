// 任务管理：当前会话的后台命令任务（run_background 族）。
//
// 数据来自宿主注入的 props.data.tasks（task.list 投影）；终止表单提交核心
// POST /tasks/{id}/kill，插件本身不直接操作进程。

export function Page(props: any) {
  const payload = props && props.data && props.data.tasks ? props.data.tasks : null;
  const tasks = payload && Array.isArray(payload.tasks) ? payload.tasks : [];
  if (tasks.length === 0) return null;

  const threadId = props && props.data && props.data.thread && props.data.thread.id
    ? props.data.thread.id
    : "";
  const now = props && props.data && props.data.render && typeof props.data.render.at === "number"
    ? props.data.render.at
    : 0;
  const running = tasks.filter(function (task: any) { return task.running; }).length;

  const rows = tasks.map(function (task: any) {
    const id = String(task.id || "");
    const command = String(task.command || "");
    const elapsed = task.startedAt && now > task.startedAt
      ? Math.max(0, Math.floor((now - task.startedAt) / 1000))
      : 0;
    const elapsedText = elapsed >= 60
      ? Math.floor(elapsed / 60) + "m " + (elapsed % 60) + "s"
      : elapsed + "s";
    const status = task.running
      ? "运行中 · " + elapsedText
      : (typeof task.exitCode === "number" ? "已结束 · 退出码 " + task.exitCode : "已结束");
    const output = String(task.outputPreview || "").trim();
    const pid = typeof task.pid === "number" ? "PID " + task.pid : "PID -";
    return (
      <div class="tm-task" data-task-id={id}>
        <div class="tm-task__head">
          <span class={"tm-task__dot " + (task.running ? "is-running" : "is-done")} />
          <span class="tm-task__command" title={command}>{command}</span>
          <span class="tm-task__meta">{pid} · {status}</span>
        </div>
        {output
          ? <details class="tm-task__output">
              <summary>输出预览</summary>
              <pre>{output}</pre>
            </details>
          : null}
        {task.running
          ? <form method="post" action={"/tasks/" + id + "/kill"} class="tm-task__kill" data-confirm="确定终止这个后台任务（整棵进程树）吗？">
              <input type="hidden" name="thread_id" value={threadId} />
              <button type="submit" class="tm-task__kill-btn">
                <Icon name="i-x-circle" size={12} />终止
              </button>
            </form>
          : null}
      </div>
    );
  });

  return (
    <details class="tm-card" id="task-manager-card">
      <summary class="tm-card__summary" title="当前会话的后台任务">
        <span class="tm-card__title"><Icon name="i-terminal" size={12} />任务</span>
        <span class="tm-card__badge">{running + "/" + tasks.length}</span>
      </summary>
      <div class="tm-card__body">
        {rows}
        {payload && payload.error ? <p class="tm-card__error">{payload.error}</p> : null}
      </div>
    </details>
  );
}
