@slang 2
@use "scripts.slang"

@# 组合组件：props 用 @props 声明；模板只负责摆结构。
@def StatCard {
  @props {
    label: String,
    value: Any,
    hint: { type: String, default: "" },
  }

  ::div(class: "card-inset") {
    ::p(class: "text-[11px] font-medium text-faint") { {{ label }} }
    ::p(class: "mt-1 text-lg font-semibold text-ink") { {{ value }} }
    @if hint != "" {
      ::p(class: "mt-0.5 text-[11px] text-faint") { {{ hint }} }
    }
  }
}

@# 计算放在模块函数里；模板直接调用。
@def StatusChip {
  @props { count: { type: Int, default: 0 } }

  ::Chip(text: labelFor(count), tone: toneFor(count), icon: "i-plug")
}

@# 作用域插槽：子组件决定每一行的数据，调用点决定怎么画。
@def MetricList {
  @props { items: { type: "list", default: [] } }

  ::ul(class: "mt-3 space-y-2") {
    @for (item, index) in items {
      ::li(class: "flex items-center justify-between gap-3 text-xs") {
        @slot row(item, index) {
          ::span(class: "text-faint") { {{ item.label }} }
          ::span(class: "font-medium text-ink") { {{ item.value }} }
        }
      }
    }
  }
}
