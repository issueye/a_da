/**
 * 提问卡片（`thread.tray` 示例插件）：渲染当前会话**待答复**的提问并提供作答表单。
 *
 * 🔴 数据与语义都在服务端：`props.data.questions` 是归一化好的投影
 * （单题/多题已摊平、选项的 value 已是回传口径），插件只负责"长什么样"。
 * 没有 `data.questions`（老宿主 / 渲染点没快照）或为空数组时返回 `null`——
 * 插件输出为空，核心提问托盘自动接管（见 docs/PLAN-question-panel.md D5）。
 *
 * 🔴 表单字段约定与核心托盘共用一套（见 PLAN §4，解析端只有一处）：
 * - 单题：`choice`（radio）/ `choices`（checkbox 多值）/ `text`；
 * - 多题：每题按题号前缀 `q_{id}__choice` / `q_{id}__choices` / `q_{id}__text`；
 * - `hidden thread_id` 决定作答后跳回哪个会话；
 * - 动作是核心路由 `POST /questions/{callId}/answer`——写操作不进插件。
 *
 * 翻页：多题时声明 `data-question-pager` 契约（`data-page-item` 标页、
 * `data-page-bar` 翻页条置 hidden、按钮 `type="button"`），行为由核心 `chat.js`
 * 统一实现（一次一题 + 上一题/下一题，页码按 callId 跨换帧记忆）；无 JS 时
 * 翻页条保持隐藏、全部题目平铺。插件自己不写 JS。
 */
export function Page(props: any) {
  const data = props && props.data ? props.data : null;
  const list = data && Array.isArray(data.questions) ? data.questions : [];
  if (!list.length) return null;
  const threadId = data && data.thread && data.thread.id ? data.thread.id : "";

  return (
    <div class="aq-list">
      {list.map(function (q: any) {
        const items = q.questions || [];
        const multi = items.length > 1;
        const singleNoChoice =
          !!q.single && items.length === 1 &&
          (!items[0].options || !items[0].options.length);
        return (
          <form
            class="aq-card"
            method="post"
            action={"/questions/" + q.callId + "/answer"}
            data-question-pager
            data-page-key={q.callId}
          >
            <input type="hidden" name="thread_id" value={threadId} />
            <div class="aq-card__head">
              <span class="aq-card__title">待答复 · 智能体的提问</span>
              <span class="aq-card__meta">等待回答中</span>
            </div>
            {items.map(function (item: any) {
              return renderQuestion(q, item);
            })}
            {multi ? (
              <div class="aq-card__pager" data-page-bar hidden>
                <button type="button" class="aq-card__pager-btn" data-page-prev>
                  上一题
                </button>
                <span class="aq-card__pager-status" data-page-status>
                  {"第 1 / " + items.length + " 题"}
                </span>
                <button type="button" class="aq-card__pager-btn" data-page-next>
                  下一题
                </button>
              </div>
            ) : null}
            <div class="aq-card__actions">
              {singleNoChoice ? (
                <button
                  class="aq-card__button aq-card__button--ghost"
                  type="submit"
                  name={fieldPrefix(q, items[0]) + "text"}
                  value="用户确认继续"
                >
                  继续
                </button>
              ) : null}
              <button class="aq-card__button" type="submit">
                提交回答
              </button>
            </div>
          </form>
        );
      })}
    </div>
  );
}

/** 字段前缀：单题用裸字段（`choice`/`choices`/`text`），多题加 `q_{id}__` 前缀。 */
function fieldPrefix(q: any, item: any) {
  return q.single ? "" : "q_" + item.id + "__";
}

/** 一道题：题干 + 选项（radio/checkbox）+ 可选文本补充。
 *  `data-page-item` 无条件带上：单题没有翻页条，契约按"条目数 >= 2"才激活，
 *  多出来的标记无副作用；这样也避开运行时对 undefined 属性的处理差异。 */
function renderQuestion(q: any, item: any) {
  const options = item.options || [];
  const multiSelect = !!item.multiSelect;
  const inputType = multiSelect ? "checkbox" : "radio";
  const fieldName = fieldPrefix(q, item) + (multiSelect ? "choices" : "choice");
  const hasOptions = options.length > 0;
  return (
    <div class="aq-card__item" data-page-item>
      <p class="aq-card__prompt">{item.question}</p>
      {hasOptions ? (
        <div class="aq-card__options">
          {options.map(function (option: any) {
            return (
              <label class="aq-card__option">
                <input type={inputType} name={fieldName} value={option.value} />
                <span class="aq-card__option-label">{option.label}</span>
                {option.description ? (
                  <span class="aq-card__option-desc">{option.description}</span>
                ) : null}
              </label>
            );
          })}
        </div>
      ) : null}
      {item.allowText ? (
        <textarea
          class="aq-card__text"
          name={fieldPrefix(q, item) + "text"}
          rows="2"
          placeholder={hasOptions ? "补充说明（可选）" : "输入你的回答"}
        ></textarea>
      ) : null}
    </div>
  );
}
