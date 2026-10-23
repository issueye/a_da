//! TSX 组件库（内置）：插件通过 import { Button } from "ssr-h:components"
//! 或直接使用全局同名组件。所有视觉契约仍然落在 ada-* class 上；
//! 唯一的原生桥是 Icon：它复用 Rust 侧的 SVG 图标集。

function Icon(props: any) {
  return h("Icon", { name: props.name, size: props.size || 16, class: props.class });
}

function __classes(base: string, extra?: any) {
  return extra ? base + " " + extra : base;
}

function __check(name: string, props: any, allowed: string[]) {
  Object.keys(props || {}).forEach((key) => {
    // `key` 与 `children` 一样是通用 prop：h() 会把它传给组件（HTML 标签上才丢弃），
    // 组件靠它落稳定身份（Disclosure 的 data-key 供 live 轮询还原展开态）。
    if (key !== "children" && key !== "key" && allowed.indexOf(key) < 0) {
      throw new Error("组件 <" + name + "> 不支持属性 `" + key + "`");
    }
  });
}

function Button(props: any) {
  __check("Button", props, ["label", "variant", "size", "icon", "href", "action", "method", "name", "value", "kind", "title", "class", "confirm", "disabled"]);
  const variant = props.variant || "secondary";
  const size = props.size || "md";
  const kind = props.kind || "submit";
  const classes = __classes(
    "ada-btn ada-btn--" + variant + " ada-btn--" + size,
    props.class
  );
  const body = (
    <>
      {props.icon ? <Icon name={props.icon} size={14} /> : null}
      <span class="ada-btn__label">{props.label}</span>
    </>
  );
  const title = props.title ? { title: props.title } : {};
  if (props.href) {
    return (
      <a class={classes} href={props.href} {...title} rel="noreferrer">
        {body}
      </a>
    );
  }
  if (props.action) {
    return (
      <form
        class="ada-action-form"
        method={props.method || "post"}
        action={props.action}
        data-confirm={props.confirm || undefined}
      >
        <button class={classes} type="submit" {...title}>
          {body}
        </button>
      </form>
    );
  }
  return (
    <button
      class={classes}
      type={kind}
      name={props.name || undefined}
      value={props.value || undefined}
      disabled={props.disabled || undefined}
      {...title}
    >
      {body}
    </button>
  );
}

function IconButton(props: any) {
  __check("IconButton", props, ["icon", "label", "kind", "href", "action", "title", "class", "disabled"]);
  const classes = __classes("ada-icon-btn", props.class);
  const label = props.label || "";
  const title = props.title || label;
  const body = <Icon name={props.icon} size={15} />;
  if (props.href) {
    return (
      <a
        class={classes}
        href={props.href}
        aria-label={label}
        title={title}
        rel="noreferrer"
      >
        {body}
      </a>
    );
  }
  if (props.action) {
    return (
      <form class="ada-action-form" method="post" action={props.action}>
        <button class={classes} type="submit" aria-label={label} title={title}>
          {body}
        </button>
      </form>
    );
  }
  return (
    <button
      class={classes}
      type={props.kind || "button"}
      aria-label={label}
      title={title}
      disabled={props.disabled || undefined}
    >
      {body}
    </button>
  );
}

function Chip(props: any) {
  __check("Chip", props, ["text", "tone", "icon", "caret", "title", "class"]);
  const tone = props.tone || "neutral";
  return (
    <span class={__classes("ada-chip ada-chip--" + tone, props.class)} title={props.title || undefined}>
      {props.icon ? <Icon name={props.icon} size={13} /> : null}
      <span class="ada-chip__text">{props.text}</span>
      {props.caret ? <Icon name="i-chevron-down" size={12} /> : null}
    </span>
  );
}

function Notice(props: any) {
  __check("Notice", props, ["text", "tone"]);
  const tone = props.tone || "neutral";
  return (
    <div class={"ada-notice ada-notice--" + tone}>
      <div class="ada-notice__row">
        <Icon name="i-alert" size={14} />
        <span>{props.text}</span>
      </div>
      {props.children ? <div class="ada-notice__action">{props.children}</div> : null}
    </div>
  );
}

function Panel(props: any) {
  __check("Panel", props, ["title", "copy"]);
  return (
    <div class="ada-panel">
      {props.title ? (
        <div class="ada-panel__head">
          <span class="ada-panel__title">{props.title}</span>
          {props.copy ? (
            <button
              class="ada-panel__copy"
              type="button"
              data-copy={props.copy}
              data-copy-label="复制"
            >
              复制
            </button>
          ) : null}
        </div>
      ) : null}
      <div class="ada-panel__body ada-panel__body--html">{props.children}</div>
    </div>
  );
}

function Disclosure(props: any) {
  __check("Disclosure", props, ["label", "kind", "icon", "detail", "sub", "meta", "badge", "badgeTone", "key"]);
  const kind = props.kind || "tool";
  const icon = props.icon || "i-terminal";
  const badgeTone = props.badgeTone || "neutral";
  return (
    <details
      class="ada-disclosure"
      data-kind={kind}
      data-state="closed"
      data-key={props.key || undefined}
    >
      <summary class="ada-disclosure__summary">
        <span class="ada-disclosure__icon"><Icon name={icon} size={13} /></span>
        <span class="ada-disclosure__caret"><Icon name="i-chevron-right" size={12} /></span>
        <span class="ada-disclosure__label">{props.label}</span>
        {props.detail ? <span class="ada-disclosure__detail">{props.detail}</span> : null}
        {props.sub ? <span class="ada-disclosure__sub">{props.sub}</span> : null}
        {props.meta ? <span class="ada-disclosure__meta">{props.meta}</span> : null}
        {props.badge ? <span class={"ada-badge ada-badge--" + badgeTone}>{props.badge}</span> : null}
      </summary>
      <div class="ada-disclosure__rail">{props.children}</div>
    </details>
  );
}

function EmptyState(props: any) {
  __check("EmptyState", props, ["icon", "title", "description"]);
  return (
    <div class="ada-empty">
      <span class="ada-empty__mark"><Icon name={props.icon || "i-chat"} size={20} /></span>
      <h2 class="ada-empty__title">{props.title}</h2>
      {props.description ? <p class="ada-empty__desc">{props.description}</p> : null}
      {props.children ? <div class="ada-empty__action">{props.children}</div> : null}
    </div>
  );
}

function TextField(props: any) {
  __check("TextField", props, ["name", "placeholder", "value", "label", "hint", "icon", "type", "disabled"]);
  return (
    <label class="ada-field">
      {props.icon ? <span class="ada-field__icon"><Icon name={props.icon} size={14} /></span> : null}
      {props.label ? <span class="ada-field__label">{props.label}</span> : null}
      <input
        class="ada-input ada-focusable"
        type={props.type || "text"}
        name={props.name}
        value={props.value || ""}
        placeholder={props.placeholder || ""}
        disabled={props.disabled || undefined}
      />
      {props.hint ? <span class="ada-field__hint">{props.hint}</span> : null}
    </label>
  );
}

function Select(props: any) {
  __check("Select", props, ["name", "value", "title", "class", "options"]);
  const options = props.options || [];
  const current = options.some((option: any) => option.value === props.value)
    ? props.value
    : (options[0] && options[0].value) || "";
  const currentOption = options.find((option: any) => option.value === current);
  return (
    <ada-select class={__classes("ada-select", props.class)} data-select>
      <input type="hidden" name={props.name} value={current} data-select-input />
      <button
        type="button"
        class="ada-select__trigger"
        aria-haspopup="listbox"
        aria-expanded="false"
        title={props.title || undefined}
        data-select-trigger
      >
        <span class="ada-select__label" data-select-label>
          {currentOption ? currentOption.label : ""}
        </span>
        <Icon name="i-chevron-down" size={12} class="ada-select__caret" />
      </button>
      <ul class="ada-select__menu" role="listbox" hidden data-select-menu>
        {options.map((option: any) => (
          <li
            class={"ada-select__option" + (option.value === current ? " is-selected" : "")}
            role="option"
            data-value={option.value}
            aria-selected={option.value === current ? "true" : "false"}
            tabindex="-1"
          >
            {option.label}
          </li>
        ))}
      </ul>
    </ada-select>
  );
}

function Modal(props: any) {
  __check("Modal", props, ["id", "title", "wide"]);
  const titleId = "ada-modal-title-" + props.id;
  return (
    <ada-modal class="ada-modal" data-modal={props.id} hidden>
      <div class="ada-modal__backdrop" data-modal-close></div>
      <div
        class={"ada-modal__panel" + (props.wide ? " ada-modal__panel--wide" : "")}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <h2 class="ada-modal__title" id={titleId}>{props.title}</h2>
        <div class="ada-modal__body">{props.children}</div>
      </div>
    </ada-modal>
  );
}

function Form(props: any) {
  __check("Form", props, ["action", "method", "class", "confirm"]);
  return (
    <form
      class={props.class || undefined}
      method={props.method || "post"}
      action={props.action || ""}
      data-confirm={props.confirm || undefined}
    >
      {props.children}
    </form>
  );
}

/** 任务状态 → 样式类：`completed|done` → done，`in_progress|running` → running。 */
function todoState(status: any) {
  if (status === "completed" || status === "done") return "done";
  if (status === "in_progress" || status === "running") return "running";
  return "pending";
}

function Todo(props: any) {
  __check("Todo", props, ["steps", "notes"]);
  const steps = props.steps || [];
  if (steps.length === 0) return null;
  return (
    <div class="ada-todo__list-wrap">
      {props.notes ? <p class="ada-todo__notes"><span>备注：</span>{props.notes}</p> : null}
      <ol class="ada-todo__list">
        {steps.map((step: any) => {
          const state = todoState(step.status);
          return (
            <li class={"ada-todo__step is-" + state}>
              <Icon
                name={
                  state === "done"
                    ? "i-check-circle"
                    : state === "running"
                      ? "i-arrow-right"
                      : "i-circle"
                }
                size={13}
              />
              <span>{step.title}</span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function UserMessage(props: any) {
  __check("UserMessage", props, ["text", "images"]);
  return (
    <div class="ada-msg ada-msg--user">
      <div class="ada-bubble">
        {props.text}
        {props.images > 0 ? (
          <span class="ada-msg__images" title="ssr-h 暂不显示图片内容">
            含 {props.images} 张图片
          </span>
        ) : null}
      </div>
    </div>
  );
}

function AssistantMessage(props: any) {
  __check("AssistantMessage", props, []);
  return (
    <div class="ada-msg ada-msg--assistant">
      <span class="ada-avatar" title="助手"><Icon name="i-bot" size={15} /></span>
      <div class="ada-msg__body markdown-body ada-markdown">{props.children}</div>
    </div>
  );
}

const __SSR_COMPONENTS = {
  Button,
  IconButton,
  Chip,
  Notice,
  Panel,
  Disclosure,
  EmptyState,
  Icon,
  TextField,
  Select,
  Modal,
  Form,
  Todo,
  UserMessage,
  AssistantMessage,
};

globalThis.__SSR_COMPONENTS__ = __SSR_COMPONENTS;
globalThis.Button = Button;
globalThis.IconButton = IconButton;
globalThis.Chip = Chip;
globalThis.Notice = Notice;
globalThis.Panel = Panel;
globalThis.Disclosure = Disclosure;
globalThis.EmptyState = EmptyState;
globalThis.Icon = Icon;
globalThis.TextField = TextField;
globalThis.Select = Select;
globalThis.Modal = Modal;
globalThis.Form = Form;
globalThis.Todo = Todo;
globalThis.UserMessage = UserMessage;
globalThis.AssistantMessage = AssistantMessage;
