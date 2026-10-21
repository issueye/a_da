//! 原生组件适配器：把 Slang 属性映射到现有 `ui::components`。
//!
//! 这是“Rust 原子、Slang 编排”的边界：DOM 结构、class、`data-*`、焦点态仍由
//! `src/ui/` 唯一负责；适配器只做属性转换，不手写第二套视觉契约。

use std::collections::BTreeMap;

use crate::ui::{
    self, Button, ButtonKind, ButtonVariant, Chip, Disclosure, EmptyState, IconButton, Notice,
    Panel, Render, Select, Size, TextField, Tone, UserMessage,
};

use super::component::{
    Args, ComponentDef, ComponentRegistry, ComponentSpec, NativeDef, PropSpec, PropType,
    RenderedSlots, SlotSpec,
};
use super::error::{err, SlangResult};
use super::eval::RenderCx;
use super::html;
use super::value::Value;

/// 全部原生组件 + 内置 HTML 元素。
pub fn default_registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();

    for element in html::builtin_elements() {
        registry.register(element.tag, ComponentDef::Element(element));
    }

    register(&mut registry, button_spec(), render_button);
    register(&mut registry, icon_button_spec(), render_icon_button);
    register(&mut registry, chip_spec(), render_chip);
    register(&mut registry, notice_spec(), render_notice);
    register(&mut registry, panel_spec(), render_panel);
    register(&mut registry, disclosure_spec(), render_disclosure);
    register(&mut registry, empty_state_spec(), render_empty_state);
    register(&mut registry, icon_spec(), render_icon);
    register(&mut registry, user_message_spec(), render_user_message);
    register(
        &mut registry,
        assistant_message_spec(),
        render_assistant_message,
    );
    register(&mut registry, text_field_spec(), render_text_field);
    register(&mut registry, select_spec(), render_select);
    register(&mut registry, modal_spec(), render_modal);
    register(&mut registry, form_spec(), render_form);
    register(&mut registry, todo_spec(), render_todo);

    registry
}

fn register(
    registry: &mut ComponentRegistry,
    spec: ComponentSpec,
    render: super::component::NativeRenderFn,
) {
    let name = spec.name.clone();
    let aliases = spec.aliases.clone();
    let alias_template = spec.clone();
    registry.register(&name, ComponentDef::Native(NativeDef { spec, render }));
    // 别名与主名共享同一份契约与渲染函数，只是注册键不同。
    for alias in aliases {
        let mut alias_spec = alias_template.clone();
        alias_spec.name = alias.clone();
        alias_spec.aliases = vec![name.clone()];
        registry.register(
            &alias,
            ComponentDef::Native(NativeDef {
                spec: alias_spec,
                render,
            }),
        );
    }
}

// ── 规格 ─────────────────────────────────────────────────────────────────────

fn req(name: &str, ty: PropType) -> PropSpec {
    PropSpec::required(name, ty)
}

fn opt(name: &str, ty: PropType) -> PropSpec {
    PropSpec::optional(name, ty)
}

fn enum_of(values: &[&str]) -> PropType {
    PropType::Enum(values.iter().map(|value| (*value).to_string()).collect())
}

fn def(name: &str, ty: PropType, value: Value) -> PropSpec {
    PropSpec::default_value(name, ty, value)
}

fn button_spec() -> ComponentSpec {
    ComponentSpec::new("Button", "body")
        .alias("button")
        .prop(req("label", PropType::String))
        .prop(opt(
            "variant",
            enum_of(&["primary", "secondary", "ghost", "danger"]),
        ))
        .prop(opt("size", enum_of(&["sm", "md", "lg"])))
        .prop(opt("kind", enum_of(&["button", "submit"])))
        .prop(opt("icon", PropType::Icon))
        .prop(opt("href", PropType::Url))
        .prop(opt("action", PropType::Url))
        .prop(opt("method", PropType::String))
        .prop(opt("name", PropType::String))
        .prop(opt("value", PropType::String))
        .prop(opt("title", PropType::String))
        .prop(opt("class", PropType::Class))
        .prop(opt("confirm", PropType::String))
        .prop(opt("modal_open", PropType::String))
        .positional(&["label"])
}

fn icon_button_spec() -> ComponentSpec {
    ComponentSpec::new("IconButton", "body")
        .alias("icon-button")
        .prop(req("icon", PropType::Icon))
        .prop(req("label", PropType::String))
        .prop(opt("kind", enum_of(&["button", "submit"])))
        .prop(opt("href", PropType::Url))
        .prop(opt("action", PropType::Url))
        .prop(opt("title", PropType::String))
        .prop(opt("class", PropType::Class))
        .prop(opt("confirm", PropType::String))
        .positional(&["icon", "label"])
}

fn chip_spec() -> ComponentSpec {
    ComponentSpec::new("Chip", "body")
        .prop(req("text", PropType::String))
        .prop(opt("icon", PropType::Icon))
        .prop(def(
            "tone",
            enum_of(&["neutral", "brand", "warn", "danger", "success"]),
            Value::Str("neutral".into()),
        ))
        .prop(def("caret", PropType::Bool, Value::Bool(false)))
        .prop(opt("title", PropType::String))
        .prop(opt("class", PropType::Class))
        .positional(&["text"])
}

fn notice_spec() -> ComponentSpec {
    ComponentSpec::new("Notice", "body")
        .prop(req("text", PropType::String))
        .prop(def(
            "tone",
            enum_of(&["neutral", "brand", "warn", "danger", "success"]),
            Value::Str("brand".into()),
        ))
        .positional(&["text"])
}

fn panel_spec() -> ComponentSpec {
    ComponentSpec::new("Panel", "body")
        .prop(opt("title", PropType::String))
        .prop(opt("copy", PropType::String))
        .prop(def("preformatted", PropType::Bool, Value::Bool(true)))
        .slot(SlotSpec::new("body", false))
}

fn disclosure_spec() -> ComponentSpec {
    ComponentSpec::new("Disclosure", "body")
        .prop(def("kind", PropType::String, Value::Str("tool".into())))
        .prop(def("icon", PropType::Icon, Value::Str("i-circle".into())))
        .prop(req("label", PropType::String))
        .prop(opt("detail", PropType::String))
        .prop(opt("sub", PropType::String))
        .prop(opt("meta", PropType::String))
        .prop(opt("badge", PropType::String))
        .prop(def(
            "badge_tone",
            enum_of(&["neutral", "brand", "warn", "danger", "success"]),
            Value::Str("neutral".into()),
        ))
        .prop(opt("key", PropType::String))
        .prop(def("detail_muted", PropType::Bool, Value::Bool(false)))
        .slot(SlotSpec::new("body", false))
        .positional(&["label"])
}

fn empty_state_spec() -> ComponentSpec {
    ComponentSpec::new("EmptyState", "body")
        .prop(def("icon", PropType::Icon, Value::Str("i-circle".into())))
        .prop(req("title", PropType::String))
        .prop(opt("description", PropType::String))
        .positional(&["title"])
}

fn icon_spec() -> ComponentSpec {
    ComponentSpec::new("Icon", "body")
        .prop(req("name", PropType::Icon))
        .prop(def("size", PropType::Int, Value::Int(14)))
        .positional(&["name"])
}

fn user_message_spec() -> ComponentSpec {
    ComponentSpec::new("UserMessage", "body")
        .prop(req("text", PropType::String))
        .prop(def("images", PropType::Int, Value::Int(0)))
        .positional(&["text"])
}

fn assistant_message_spec() -> ComponentSpec {
    ComponentSpec::new("AssistantMessage", "body").slot(SlotSpec::new("body", true))
}

fn text_field_spec() -> ComponentSpec {
    ComponentSpec::new("TextField", "body")
        .prop(req("name", PropType::String))
        .prop(def("value", PropType::String, Value::Str(String::new())))
        .prop(def(
            "placeholder",
            PropType::String,
            Value::Str(String::new()),
        ))
        .prop(opt("label", PropType::String))
        .prop(opt("hint", PropType::String))
        .prop(opt("icon", PropType::Icon))
        .prop(def("type", PropType::String, Value::Str("text".into())))
        .prop(def("required", PropType::Bool, Value::Bool(false)))
        .prop(def("disabled", PropType::Bool, Value::Bool(false)))
}

fn select_spec() -> ComponentSpec {
    ComponentSpec::new("Select", "body")
        .prop(req("name", PropType::String))
        .prop(opt("value", PropType::String))
        .prop(req("options", PropType::List(Box::new(PropType::Object))))
        .prop(opt("title", PropType::String))
        .prop(opt("class", PropType::Class))
}

fn modal_spec() -> ComponentSpec {
    ComponentSpec::new("Modal", "body")
        .prop(req("id", PropType::String))
        .prop(req("title", PropType::String))
        .prop(def("wide", PropType::Bool, Value::Bool(false)))
        .slot(SlotSpec::new("body", false))
        .slot(SlotSpec::new("actions", false))
}

fn form_spec() -> ComponentSpec {
    ComponentSpec::new("Form", "body")
        .prop(req("action", PropType::Url))
        .prop(def("method", PropType::String, Value::Str("post".into())))
        .prop(opt("confirm", PropType::String))
        .prop(opt("class", PropType::Class))
        .slot(SlotSpec::new("body", false))
}

fn todo_spec() -> ComponentSpec {
    ComponentSpec::new("Todo", "body")
        .prop(req("steps", PropType::List(Box::new(PropType::Object))))
        .prop(opt("notes", PropType::String))
}

// ── 渲染 ─────────────────────────────────────────────────────────────────────

fn tone(raw: &str) -> Tone {
    match raw {
        "brand" => Tone::Brand,
        "warn" => Tone::Warn,
        "danger" => Tone::Danger,
        "success" => Tone::Success,
        _ => Tone::Neutral,
    }
}

fn button_variant(raw: &str) -> ButtonVariant {
    match raw {
        "primary" => ButtonVariant::Primary,
        "ghost" => ButtonVariant::Ghost,
        "danger" => ButtonVariant::Danger,
        _ => ButtonVariant::Secondary,
    }
}

fn size(raw: &str) -> Size {
    match raw {
        "sm" => Size::Sm,
        "lg" => Size::Lg,
        _ => Size::Md,
    }
}

fn render_button(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let mut button = Button::new(args.required_string("label")?);
    if let Some(raw) = args.enum_("variant", &["primary", "secondary", "ghost", "danger"])? {
        button = button.variant(button_variant(&raw));
    }
    if let Some(raw) = args.enum_("size", &["sm", "md", "lg"])? {
        button = button.size(size(&raw));
    }
    if let Some(raw) = args.enum_("kind", &["button", "submit"])? {
        button = button.kind(if raw == "submit" {
            ButtonKind::Submit
        } else {
            ButtonKind::Button
        });
    }
    if let Some(icon) = args.icon("icon")? {
        button = button.icon(icon);
    }
    if let Some(href) = args.url("href")? {
        button = button.href(href);
    }
    if let Some(action) = args.url("action")? {
        button = button.action(action);
    }
    if let Some(method) = args.string("method")? {
        button.method = Some(method);
    }
    if let Some(name) = args.string("name")? {
        button.name = Some(name);
    }
    if let Some(value) = args.string("value")? {
        button.value = Some(value);
    }
    if let Some(title) = args.string("title")? {
        button = button.title(title);
    }
    if let Some(class) = args.class("class")? {
        button = button.class(class);
    }
    let mut extra = String::new();
    if let Some(confirm) = args.string("confirm")? {
        extra.push_str(&format!(
            " data-confirm=\"{}\"",
            ui::theme::escape(&confirm)
        ));
    }
    if let Some(modal) = args.string("modal_open")? {
        extra.push_str(&format!(
            " data-modal-open=\"{}\"",
            ui::theme::escape(&modal)
        ));
    }
    if !extra.is_empty() {
        button = button.attrs(extra.trim().to_string());
    }
    Ok(button.render())
}

fn render_icon_button(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let mut button = IconButton::new(
        args.required_string("icon")?,
        args.required_string("label")?,
    );
    if let Some(raw) = args.enum_("kind", &["button", "submit"])? {
        button = button.kind(if raw == "submit" {
            ButtonKind::Submit
        } else {
            ButtonKind::Button
        });
    }
    if let Some(href) = args.url("href")? {
        button = button.href(href);
    }
    if let Some(action) = args.url("action")? {
        button = button.action(action);
    }
    if let Some(title) = args.string("title")? {
        button.title = Some(title);
    }
    if let Some(class) = args.class("class")? {
        button = button.class(class);
    }
    if let Some(confirm) = args.string("confirm")? {
        button = button.attrs(format!("data-confirm=\"{}\"", ui::theme::escape(&confirm)));
    }
    Ok(button.render())
}

fn render_chip(_cx: &mut RenderCx<'_>, args: &Args, _slots: &RenderedSlots) -> SlangResult<String> {
    let mut chip = Chip::new(
        args.required_string("text")?,
        tone(
            &args
                .enum_("tone", &["neutral", "brand", "warn", "danger", "success"])?
                .unwrap_or_else(|| "neutral".into()),
        ),
    );
    if let Some(icon) = args.icon("icon")? {
        chip = chip.icon(icon);
    }
    if let Some(caret) = args.bool("caret")? {
        chip = chip.caret(caret);
    }
    if let Some(title) = args.string("title")? {
        chip = chip.title(title);
    }
    if let Some(class) = args.class("class")? {
        chip = chip.class(class);
    }
    Ok(chip.render())
}

fn render_notice(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let tone = tone(
        &args
            .enum_("tone", &["neutral", "brand", "warn", "danger", "success"])?
            .unwrap_or_else(|| "brand".into()),
    );
    Ok(Notice::new(args.required_string("text")?, tone).render())
}

fn render_panel(_cx: &mut RenderCx<'_>, args: &Args, slots: &RenderedSlots) -> SlangResult<String> {
    let body = slots.get("body").to_string();
    let mut panel = if args.bool("preformatted")?.unwrap_or(true) {
        Panel::new(body)
    } else {
        Panel::new(String::new()).html(body)
    };
    if let Some(title) = args.string("title")? {
        panel = panel.title(title);
    }
    if let Some(copy) = args.string("copy")? {
        panel = panel.copy(copy);
    }
    Ok(panel.render())
}

fn render_disclosure(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    slots: &RenderedSlots,
) -> SlangResult<String> {
    let mut disclosure = Disclosure::new(
        args.string("kind")?.unwrap_or_else(|| "tool".into()),
        args.icon("icon")?.unwrap_or_else(|| "i-circle".into()),
        args.required_string("label")?,
    );
    if let Some(detail) = args.string("detail")? {
        disclosure = disclosure.detail(detail);
    }
    if let Some(sub) = args.string("sub")? {
        disclosure = disclosure.sub(sub);
    }
    if let Some(meta) = args.string("meta")? {
        disclosure = disclosure.meta(meta);
    }
    if let Some(badge) = args.string("badge")? {
        let badge_tone = tone(
            &args
                .enum_(
                    "badge_tone",
                    &["neutral", "brand", "warn", "danger", "success"],
                )?
                .unwrap_or_else(|| "neutral".into()),
        );
        disclosure = disclosure.badge(badge, badge_tone);
    }
    if let Some(key) = args.string("key")? {
        disclosure = disclosure.key(key);
    }
    if let Some(muted) = args.bool("detail_muted")? {
        disclosure = disclosure.detail_muted(muted);
    }
    Ok(disclosure.body(slots.get("body")).render())
}

fn render_empty_state(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let state = EmptyState {
        icon: args.icon("icon")?.unwrap_or_else(|| "i-circle".into()),
        title: args.required_string("title")?,
        description: args.string("description")?,
        action: None,
    };
    Ok(state.render())
}

fn render_icon(_cx: &mut RenderCx<'_>, args: &Args, _slots: &RenderedSlots) -> SlangResult<String> {
    let name = args.required_string("name")?;
    if !ui::icons::has(&name) {
        return err("E3006", format!("未知图标 `{name}`"), args.span());
    }
    let size = args.int("size")?.unwrap_or(14).clamp(8, 64) as u16;
    Ok(ui::icons::icon(&name, size))
}

fn render_user_message(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    Ok(UserMessage {
        text: args.required_string("text")?,
        images: args.int("images")?.unwrap_or(0).max(0) as usize,
    }
    .render())
}

fn render_assistant_message(
    _cx: &mut RenderCx<'_>,
    _args: &Args,
    slots: &RenderedSlots,
) -> SlangResult<String> {
    Ok(crate::ui::AssistantMessage {
        html: slots.get("body").to_string(),
    }
    .render())
}

fn render_text_field(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let mut field = TextField::new(
        args.required_string("name")?,
        args.string("placeholder")?.unwrap_or_default(),
    );
    field.value = args.string("value")?.unwrap_or_default();
    if let Some(label) = args.string("label")? {
        field.label = Some(label);
    }
    if let Some(hint) = args.string("hint")? {
        field.hint = Some(hint);
    }
    if let Some(icon) = args.icon("icon")? {
        field.icon = Some(icon);
    }
    field.input_type = args.string("type")?;
    let mut extra = String::new();
    if args.bool("required")?.unwrap_or(false) {
        extra.push_str(" required");
    }
    if args.bool("disabled")?.unwrap_or(false) {
        extra.push_str(" disabled");
    }
    if !extra.is_empty() {
        field.extra_attrs = Some(extra.trim().to_string());
    }
    Ok(field.render())
}

fn render_select(
    _cx: &mut RenderCx<'_>,
    args: &Args,
    _slots: &RenderedSlots,
) -> SlangResult<String> {
    let mut select = Select::new(args.required_string("name")?);
    if let Some(value) = args.string("value")? {
        select = select.value(value);
    }
    if let Some(options) = args.list("options")? {
        for option in options {
            let map = option.as_map().ok_or_else(|| {
                super::error::Diagnostic::error(
                    "E3006",
                    "`Select.options` 的每一项都必须是 { value, label } 对象",
                    args.span(),
                )
            })?;
            let value = map.get("value").map(Value::to_text).unwrap_or_default();
            let label = map
                .get("label")
                .map(Value::to_text)
                .unwrap_or_else(|| value.clone());
            select = select.option(value, label);
        }
    }
    if let Some(title) = args.string("title")? {
        select = select.title(title);
    }
    if let Some(class) = args.class("class")? {
        select = select.class(class);
    }
    Ok(select.render())
}

fn render_modal(_cx: &mut RenderCx<'_>, args: &Args, slots: &RenderedSlots) -> SlangResult<String> {
    let mut modal = ui::Modal::new(args.required_string("id")?, args.required_string("title")?);
    modal.body_html = slots.get("body").to_string();
    modal.actions_html = slots.get("actions").to_string();
    if args.bool("wide")?.unwrap_or(false) {
        modal = modal.wide();
    }
    Ok(modal.render())
}

fn render_form(_cx: &mut RenderCx<'_>, args: &Args, slots: &RenderedSlots) -> SlangResult<String> {
    let action = args.url("action")?.unwrap_or_default();
    let method = args.string("method")?.unwrap_or_else(|| "post".into());
    let mut attrs = String::new();
    if let Some(class) = args.class("class")? {
        attrs.push_str(&format!(" class=\"{}\"", ui::theme::escape(&class)));
    }
    if let Some(confirm) = args.string("confirm")? {
        attrs.push_str(&format!(
            " data-confirm=\"{}\"",
            ui::theme::escape(&confirm)
        ));
    }
    Ok(format!(
        r#"<form method="{}" action="{}"{}>{}</form>"#,
        ui::theme::escape(&method),
        ui::theme::escape(&action),
        attrs,
        slots.get("body"),
    ))
}

fn render_todo(_cx: &mut RenderCx<'_>, args: &Args, _slots: &RenderedSlots) -> SlangResult<String> {
    let steps = args.list("steps")?.unwrap_or(&[]);
    let mut parsed = Vec::new();
    for step in steps {
        let map: &BTreeMap<String, Value> = step.as_map().ok_or_else(|| {
            super::error::Diagnostic::error(
                "E3006",
                "`Todo.steps` 的每一项都必须是 { id, title, status } 对象",
                args.span(),
            )
        })?;
        let status = match map.get("status").and_then(Value::as_str) {
            Some("completed") => ui::TodoStatus::Completed,
            Some("in_progress") => ui::TodoStatus::InProgress,
            _ => ui::TodoStatus::Pending,
        };
        parsed.push(ui::TodoStep {
            id: map.get("id").map(Value::to_text).unwrap_or_default(),
            title: map.get("title").map(Value::to_text).unwrap_or_default(),
            status,
        });
    }
    Ok(ui::TodoList {
        steps: parsed,
        notes: args.string("notes")?,
    }
    .render())
}
