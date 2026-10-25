function Page(props: any) {
  return (
    <div class="flex items-start justify-between gap-3">
      <div>
        <p class="text-xs font-medium text-ink">TSX 运行时卡片</p>
        <p class="mt-1 text-[11px] leading-4 text-faint">
          这张卡片由 {props.plugin.name} 的 dashboard.tsx 在服务端渲染。
        </p>
      </div>
      <Chip text="TSX" tone="brand" icon="i-code" />
    </div>
  );
}
