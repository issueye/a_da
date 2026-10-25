function Page(props: any) {
  return (
    <span
      class="ada-chip ada-chip--brand"
      title="来自 hello-tsx 的 TSX 插槽；插件 JS 会调用 /ext/hello-tsx/api/info 并把结果写进这里"
      data-hello-api
    >
      <span class="ada-dot ada-dot--brand"></span>
      <span class="ada-chip__text">TSX 插槽</span>
    </span>
  );
}
