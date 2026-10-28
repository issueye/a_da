/**
 * 提问卡片（`thread.tray` 插槽）：渲染插件后端 `list` 动作返回的待答提问。
 *
 * 整个提问功能由本插件提供：后端 `ask_user` 工具写状态文件并等待，前端通过
 * `/ext/ask-user/backend/answer` 提交作答。核心不再有提问托盘或 `question.answer`。
 *
 * 表单字段（后端 `answer` 动作解析）：
 * - `question_id` / `thread_id`；
 * - 每题：`choice__{id}`（单选）/ `choices__{id}`（多选取值数组）/ `text__{id}`。
 *
 * 多题翻页沿用核心 `chat.js` 的 `data-question-pager` 契约（无 JS 时全部平铺）。
 */
export function Page(props: any) {
  const data = props && props.data ? props.data : null;
  const backend = data && data.backend ? data.backend : null;
  const group = backend && backend["ask-user"] ? backend["ask-user"] : null;
  const list = group && group["list"] && group["list"].ok ? group["list"].data : null;
  const questions = list && Array.isArray(list.questions) ? list.questions : [];
  if (!questions.length) return null;
  const threadId = data && data.thread && data.thread.id ? data.thread.id : "";

  return (
    <div class="aq-list">
      {questions.map(function (question: any) {
        const items = Array.isArray(question.items) ? question.items : [];
        const multi = items.length > 1;
        return (
          <form
            class="aq-card"
            method="post"
            action="/ext/ask-user/backend/answer"
            data-question-pager
            data-page-key={question.id}
          >
            <input type="hidden" name="question_id" value={question.id} />
            <input type="hidden" name="thread_id" value={threadId} />
            <div class="aq-card__head">
              <span class="aq-card__title">待答复 · 智能体的提问</span>
              <span class="aq-card__meta">等待回答中</span>
            </div>
            {items.map(function (item: any) {
              return renderQuestion(item);
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

function renderQuestion(item: any) {
  const choices = Array.isArray(item.choices) ? item.choices : [];
  const multiSelect = item.multiSelect === true;
  const inputType = multiSelect ? "checkbox" : "radio";
  const fieldName = (multiSelect ? "choices__" : "choice__") + item.id;
  const hasChoices = choices.length > 0;
  return (
    <div class="aq-card__item" data-page-item>
      <p class="aq-card__prompt">{item.question}</p>
      {hasChoices ? (
        <div class="aq-card__options">
          {choices.map(function (choice: any) {
            return (
              <label class="aq-card__option">
                <input type={inputType} name={fieldName} value={choice.id} />
                <span class="aq-card__option-label">{choice.label}</span>
                {choice.description ? (
                  <span class="aq-card__option-desc">{choice.description}</span>
                ) : null}
              </label>
            );
          })}
        </div>
      ) : null}
      {item.allowText ? (
        <textarea
          class="aq-card__text"
          name={"text__" + item.id}
          rows="2"
          placeholder={hasChoices ? "补充说明（可选）" : "输入你的回答"}
        ></textarea>
      ) : null}
    </div>
  );
}
