/**
 * 任务卡片：把 `todo` 工具的当前计划渲染成一张可收起的悬浮卡。
 *
 * 挂载点由 `plugin.json` 的 `slots[].slot` 决定：
 * - `chat.corner`（默认）：会话区右上角，与正文/顶栏都不抢位置；
 * - `composer.above`：输入框上方的悬浮区（贴底浮在对话流之上）。
 *
 * 计划状态直接来自核心 `props.data.todo`（`todo` 工具的参数投影）。
 * 2026-10-27：`beforeTodoUpdate` / `afterTodoUpdate` 已从核心移除，插件不再维护
 * 第二份计划状态；后续若需要拦截/旁注，由统一事件系统提供。
 *
 * 没有计划时返回 `null`：不画 0/0 的空面板。
 */
export function Page(props: any) {
  const todo = props && props.data && props.data.todo ? props.data.todo : null;
  if (!todo || !todo.total) return null;

  const done = todo.done || 0;
  const total = todo.total || 0;
  const allDone = !!todo.allDone;
  const active = todo.active || "";
  const state = allDone ? "is-done" : "is-active";

  return (
    <details
      class={"tp-card " + state}
      data-state="collapsed"
      data-done={done}
      data-total={total}
      id="task-plan-card"
    >
      <summary class="tp-card__summary" title="点击展开任务规划步骤">
        <span class="tp-card__head-left">
          <Icon name="i-list" size={12} />
          <span class="tp-card__title tp-card__title--collapsed">任务规划</span>
          <span class="tp-card__title tp-card__title--expanded">任务规划步骤</span>
          <span class="tp-card__count tp-card__count--collapsed">
            {done + "/" + total}
          </span>
          <span class="tp-card__count tp-card__count--expanded">
            {done + " / " + total + " 已完成"}
          </span>
          {active && !allDone ? <span class="tp-card__active">{active}</span> : null}
        </span>
        <span class="tp-card__head-right">
          <button
            class="tp-card__copy"
            type="button"
            data-copy={todo.markdown || ""}
            data-copy-label="复制"
            title="复制 Markdown 清单"
          >
            <Icon name="i-copy" size={12} />
            <span>复制</span>
          </button>
          <span class="tp-card__collapse">
            <Icon name="i-chevron-up" size={12} />
            收起
          </span>
        </span>
      </summary>
      <div class="tp-card__body">
        <Todo steps={todo.steps} notes={todo.notes} />
      </div>
    </details>
  );
}
