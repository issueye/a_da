@slang 2
@use "components.slang"

@# 服务端只做一次性数据准备；交互交给浏览器里的 Alpine。
@let plugin_count = data.plugin_count ?? 0;
@let rows = [
  { label: "前端插件", value: plugin_count },
  { label: "产品", value: app.product },
];

::div(class: "max-w-2xl space-y-4") {
  ::div {
    ::span(class: "badge badge-brand") { hello-slang · 独立页面 }
    ::h2(class: "mt-2 text-lg font-semibold tracking-tight text-ink") {
      前端插件页面（Slang）
    }
    ::p(class: "mt-1 text-xs leading-5 text-muted") {
      这个页面由 {{ plugin.name }} v{{ plugin.version }} 在服务端解析 .slang 得到。
      没有新增 Rust 组件，也没有重新编译。
    }
  }

  ::div(class: "card-inset") {
    ::p(class: "text-xs font-medium text-ink") { 运行时上下文 }
    ::p(class: "mt-1 text-[11px] text-faint") {
      插件：{{ plugin.id }} · 页面：{{ page.slug }} · 工作区：{{ app.workspace }}
    }
    ::div(class: "mt-3 flex items-center gap-2") {
      ::Chip(text: "Slang / 2", tone: "brand", icon: "i-code")
      ::Chip(text: "SSR", tone: "success", icon: "i-check-circle")
      ::StatusChip(count: plugin_count)
    }
  }

  ::div(class: "grid gap-3 sm:grid-cols-2") {
    ::StatCard(label: "前端插件", value: plugin_count, hint: "运行时扫描")
    ::StatCard(label: "产品", value: app.product, hint: "服务端上下文")
  }

  @# 作用域插槽：MetricList 把 item / index 交给这里的 @fill。
  ::MetricList(items: rows) {
    @fill row(item, index) {
      ::span(class: "text-faint") { {{ index + 1 }}. {{ item.label }} }
      ::span(class: "font-medium text-ink") { {{ item.value }} }
    }
  }

  @if plugin_count > 0 {
    ::Notice(text: "插件目录已扫描；以上数据来自服务端上下文。", tone: "success")
  } @else {
    ::Notice(text: "插件目录为空；这张卡片仍然由服务端渲染。", tone: "warn")
  }

  ::div(class: "flex items-center gap-2") {
    ::Button("刷新页面", variant: "secondary", size: "sm", icon: "i-refresh",
             href: "/ext/hello-slang/hello")
    ::Button("打开对话", variant: "primary", size: "sm", href: "/")
  }
}
