// 任务管理：当前会话的后台命令任务（`run_background` 族）。
//
// 形态与 Token 天气同构：挂在 `composer.above`（输入框上方悬浮区，`.ada-float-dock`），
// 收起时是一枚胶囊，点开是向上弹出的浮窗（`details.ada-popover` + `.ada-popover__panel`）。
// 浮窗不会被 SSE `parts` 的整块替换关掉——`chat.js` 按 `id` 还原 `open`，
// 点外面 / `Esc` 也由它统一收起，插件不需要自带脚本做开合。
//
// 数据来自宿主注入的 `props.data.tasks`（`task.list` 投影，按当前会话过滤）。
// 任务输出的变化由宿主**以事件方式推进来**（SSE `parts`：任务表指纹变了就补一帧），
// 插件只负责"长什么样"，不做轮询、不重演状态。
//
// 终止表单提交核心 `POST /tasks/{id}/kill`，插件不直接操作进程。

/** 耗时：60 秒以内给 `12s`，超过给 `1m 05s`。 */
function elapsedText(task: any, now: number): string {
  const started = typeof task.startedAt === "number" ? task.startedAt : 0;
  if (!started || !now || now <= started) {
    return "0s";
  }
  const seconds = Math.max(0, Math.floor((now - started) / 1000));
  if (seconds < 60) {
    return seconds + "s";
  }
  const rest = seconds % 60;
  return Math.floor(seconds / 60) + "m " + (rest < 10 ? "0" + rest : String(rest)) + "s";
}

/** 一行状态：运行中带真实耗时；结束的带退出码。 */
function statusText(task: any, now: number): string {
  if (task.running) {
    return "运行中 · " + elapsedText(task, now);
  }
  if (typeof task.exitCode === "number") {
    return "已结束 · 退出码 " + task.exitCode;
  }
  return "已结束";
}

/** 输出预览的行数（`split` 在末尾换行时会多出一项，这里按真实行计）。 */
function lineCount(output: string): number {
  if (!output) {
    return 0;
  }
  const lines = output.split("\n");
  return output.endsWith("\n") ? lines.length - 1 : lines.length;
}

/** 前台命令耗时（`run_command` 的 `durationMs`）：`820ms` / `5.1s` / `1m 05s`。 */
function runDurationText(ms: number): string {
  if (!ms || ms < 0) {
    return "";
  }
  if (ms < 1000) {
    return ms + "ms";
  }
  const seconds = ms / 1000;
  if (seconds < 60) {
    return seconds.toFixed(1) + "s";
  }
  const whole = Math.floor(seconds);
  const rest = whole % 60;
  return Math.floor(whole / 60) + "m " + (rest < 10 ? "0" + rest : String(rest)) + "s";
}

/** 前台命令的一行状态：退出码只认结构化字段，不从输出里猜。 */
function runStatusText(run: any): string {
  if (run.running) {
    return "执行中";
  }
  if (typeof run.exitCode === "number") {
    return (run.exitCode === 0 ? "完成" : "失败") + " · 退出码 " + run.exitCode;
  }
  const raw = String(run.status || "");
  if (raw === "timeout") {
    return "已超时";
  }
  if (raw === "aborted") {
    return "已中止";
  }
  if (raw === "error" || raw === "failed") {
    return "失败";
  }
  return "已结束";
}

export function Page(props: any) {
  const payload = props && props.data && props.data.tasks ? props.data.tasks : null;
  const tasks = payload && Array.isArray(payload.tasks) ? payload.tasks : [];
  const failed = !!(payload && payload.error);
  // 前台命令（`run_command`）的投影：只有声明 `runs.read` 的插件才拿得到，
  // 没声明时这里是 `null`（不是"没有命令"）——两种情况都不渲染这一段。
  const runsPayload = props && props.data ? props.data.runs : null;
  const runs = runsPayload && Array.isArray(runsPayload.runs) ? runsPayload.runs : [];
  // 没有后台任务、也没有命令记录时不渲染任何东西：胶囊不该在空闲会话里常驻
  // （与天气胶囊不同，它没有"空"的状态可表达）。读取失败要冒出来——
  // 否则用户只会觉得"任务管理消失了"，无从判断是断了还是没任务。
  if (tasks.length === 0 && runs.length === 0 && !failed) {
    return null;
  }

  const threadId = props && props.data && props.data.thread && props.data.thread.id
    ? props.data.thread.id
    : "";
  const now = props && props.data && props.data.render && typeof props.data.render.at === "number"
    ? props.data.render.at
    : 0;
  const running = tasks.filter(function (task: any) { return task.running; }).length;
  // 有后台任务时按"运行中/总数"报；只剩命令记录时报记录条数（它们不可终止，口径不同）。
  const badge = tasks.length > 0 ? running + "/" + tasks.length : String(runs.length);
  const tone = failed ? "danger" : running > 0 ? "brand" : "neutral";
  const advice = failed
    ? "读取任务表失败：" + payload.error
    : running > 0
      ? running + " 个任务在跑，输出随事件流刷新（完整输出用 check_task）"
      : tasks.length > 0
        ? "全部 " + tasks.length + " 个任务已结束，进程表不会落盘"
        : "最近 " + runs.length + " 条前台命令（已结束，仅作记录，不可终止）";

  const rows = tasks.map(function (task: any) {
    const id = String(task.id || "");
    const command = String(task.command || "");
    const output = String(task.outputPreview || "");
    const pid = typeof task.pid === "number" ? "PID " + task.pid : "PID -";
    const lines = lineCount(output);
    return (
      <div class="tm-task" data-task-id={id} data-running={task.running ? "1" : "0"}>
        <div class="tm-task__head">
          <span class={"tm-task__dot " + (task.running ? "is-running" : "is-done")} />
          <span class="tm-task__command" title={command}>{command}</span>
          <span class="tm-task__pid">{pid}</span>
        </div>
        <div class="tm-task__meta">
          <span class={"tm-task__status " + (task.running ? "is-running" : "is-done")}>
            {statusText(task, now)}
          </span>
          {task.running
            ? <form
                method="post"
                action={"/tasks/" + id + "/kill"}
                class="tm-task__kill"
                data-confirm="确定终止这个后台任务（整棵进程树）吗？"
              >
                <input type="hidden" name="thread_id" value={threadId} />
                <button type="submit" class="tm-task__kill-btn">
                  <Icon name="i-x-circle" size={12} />终止
                </button>
              </form>
            : null}
        </div>
        {output
          ? <details class="tm-task__output" id={"tm-output-" + id}>
              <summary class="tm-task__output-summary">
                <Icon name="i-chevron-right" size={11} class="tm-task__output-caret" />
                输出 · {lines} 行
              </summary>
              <pre class="tm-task__pre" data-tm-key={id}>{output}</pre>
            </details>
          : null}
      </div>
    );
  });

  // 最近命令：**只读**记录（前台命令结束后没有可终止的进程，
  // 所以这里不出现「终止」按钮，也不带 PID）。
  const runRows = runs.map(function (run: any) {
    const id = String(run.id || "");
    const command = String(run.command || "");
    const output = String(run.outputPreview || "");
    const lines = typeof run.outputLines === "number" ? run.outputLines : lineCount(output);
    const duration = runDurationText(typeof run.durationMs === "number" ? run.durationMs : 0);
    const cwd = String(run.cwd || "");
    return (
      <div class="tm-run" data-run-id={id} data-running={run.running ? "1" : "0"}>
        <div class="tm-task__head">
          <span class={"tm-task__dot " + (run.running ? "is-running" : "is-done")} />
          <span class="tm-task__command" title={command}>{command}</span>
          {duration ? <span class="tm-task__pid">{duration}</span> : null}
        </div>
        <div class="tm-task__meta">
          <span class={"tm-task__status " + (run.running ? "is-running" : "is-done")}>
            {runStatusText(run)}
          </span>
          {cwd ? <span class="tm-run__cwd" title={cwd}>{cwd}</span> : null}
        </div>
        {output
          ? <details class="tm-task__output" id={"tm-run-output-" + id}>
              <summary class="tm-task__output-summary">
                <Icon name="i-chevron-right" size={11} class="tm-task__output-caret" />
                输出 · {lines} 行
              </summary>
              <pre class="tm-task__pre" data-tm-key={"run:" + id}>{output}</pre>
            </details>
          : null}
      </div>
    );
  });

  const hasTasks = tasks.length > 0;

  return (
    <details class="ada-popover tm-popover" id="task-manager-popover">
      <summary
        class={"ada-chip ada-chip--" + tone + " tm-chip"}
        title={advice}
        aria-label="后台任务详情"
      >
        <Icon name="i-terminal" size={13} class="tm-chip__icon" />
        <span class="ada-chip__text">任务</span>
        <span class="tm-chip__badge">{badge}</span>
        <Icon name="i-chevron-down" size={11} class="tm-chip__caret" />
      </summary>

      <div class="ada-popover__panel tm-panel">
        <div class="tm-panel__head">
          <span class="tm-panel__orb" aria-hidden="true"><Icon name="i-terminal" size={22} /></span>
          <div class="tm-panel__copy">
            <div class="tm-panel__kicker">{hasTasks ? "BACKGROUND TASKS" : "RECENT COMMANDS"}</div>
            <div class="tm-panel__title">
              {hasTasks ? "后台任务" : "最近命令"}
              <span class="tm-panel__count">
                {hasTasks
                  ? running + " 运行中 · 共 " + tasks.length
                  : runs.length + " 条记录"}
              </span>
            </div>
            <p class="tm-panel__advice">{advice}</p>
          </div>
        </div>

        <div class="tm-scroll">
          {hasTasks
            ? <div class="tm-group">
                <div class="tm-section">后台任务 · {tasks.length}</div>
                <div class="tm-list">{rows}</div>
              </div>
            : null}
          {runs.length > 0
            ? <div class="tm-group">
                <div class="tm-section">最近命令 · {runs.length}</div>
                <div class="tm-list">{runRows}</div>
              </div>
            : null}
        </div>

        <div class="tm-panel__foot">
          <span class="tm-note">任务输出 = 前 4000 字符，随事件流刷新</span>
          <span class="tm-note">命令记录取自会话，只读</span>
        </div>
      </div>
    </details>
  );
}
