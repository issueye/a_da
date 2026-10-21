@slang 2
@use "components.slang"

@let count = data.plugin_count ?? 0;

::div(class: "card-inset") {
  ::h3(class: "text-xs font-medium text-ink") { {{ plugin.name }} }
  ::p(class: "mt-1 text-[11px] text-faint") {
    v{{ plugin.version }} · {{ plugin.description }}
  }
  ::div(class: "mt-3") {
    ::StatCard(label: "页面", value: plugin.id, hint: "本插件")
  }
  ::div(class: "mt-2") {
    ::Chip(text: labelFor(count), tone: toneFor(count), icon: "i-plug")
  }
}
