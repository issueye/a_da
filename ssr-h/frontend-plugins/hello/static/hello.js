(() => {
  const button = document.getElementById('hello-action');
  const status = document.getElementById('hello-status');
  if (!button || !status) return;
  let count = 0;
  button.addEventListener('click', () => {
    count += 1;
    status.textContent = `插件脚本已运行 ${count} 次 · ${new Date().toLocaleTimeString()}`;
  });
})();
