@slang 2

@# 模块脚本：纯计算、无 IO；同一次渲染里被所有组件共享。
@script {
  function toneFor(count) {
    if (count > 0) { return "success"; }
    return "warn";
  }

  function labelFor(count) {
    if (count <= 0) { return "还没有插件"; }
    return `共 {{ count }} 个插件`;
  }
}
