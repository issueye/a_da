import { Button, Chip, Panel, Disclosure } from "ssr-h:components";

export function Page(props: any) {
  const items = [
    { label: "前端插件", value: props.data.plugin_count ?? 0 },
    { label: "产品", value: props.app.product },
  ];

  return (
    <div class="max-w-2xl space-y-4">
      <div>
        <span class="badge badge-brand hello-tsx-badge">hello-tsx · TSX</span>
        <h2 class="mt-2 text-lg font-semibold tracking-tight text-ink">
          页面由 TSX 在服务端渲染
        </h2>
        <p class="mt-1 text-xs leading-5 text-muted">
          来自 {props.plugin.id} v{props.plugin.version}：OXC 去类型 + JSX 转译，
          Boa 执行后把 VNode 交给 Rust 的 Button / Chip / Panel。
        </p>
      </div>

      <Panel title="运行时上下文">
        <p class="text-xs text-muted">工作区：{props.app.workspace}</p>
        <p class="text-xs text-muted">页面：{props.page.slug}</p>
      </Panel>

      <div class="flex flex-wrap items-center gap-2">
        <Chip text={"插件 " + (props.data.plugin_count ?? 0)} tone="brand" icon="i-plug" />
        <Chip text="TSX / server" tone="success" icon="i-check-circle" />
        <Button label="打开对话" variant="primary" size="sm" href="/" />
        <Button label="刷新" variant="secondary" size="sm" icon="i-refresh" href="/ext/hello-tsx/hello" />
      </div>

      <Disclosure label="渲染链路" detail="click to expand" badge="TSX" badgeTone="brand">
        <ul class="text-xs text-muted">
          {items.map((item: any, index: number) => (
            <li class="flex justify-between">
              <span>{index + 1}. {item.label}</span>
              <span>{item.value}</span>
            </li>
          ))}
        </ul>
      </Disclosure>
    </div>
  );
}

