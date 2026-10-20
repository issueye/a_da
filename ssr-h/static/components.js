/*
 * a_da 组件库 · 客户端行为
 * ---------------------------------------------------------------------------
 * 组件的 DOM 与状态语义在服务端渲染（src/ui/），这里只做三件事：
 *   1. 把"状态"同步到 data-*（折叠开合、聚焦），供样式与插件读取；
 *   2. 渐进增强：没有 JS 时页面依然完整可读（SSR 基线）；
 *   3. 插件插槽注册表：把服务端渲染的插槽暴露给插件与调试面板。
 *
 * 刻意不做的事：不接管业务动作（发送/审批/重试都走原生表单提交），
 * 不做虚拟 DOM，不引入框架。
 */
(function () {
  'use strict';

  /* ── 1. 折叠行：整行可点 + 状态同步 ─────────────────────────────────── */
  function syncDisclosureState(details) {
    details.setAttribute('data-state', details.open ? 'open' : 'closed');
  }

  function bindDisclosures(root) {
    var nodes = (root || document).querySelectorAll('.ada-disclosure');
    for (var i = 0; i < nodes.length; i += 1) {
      var node = nodes[i];
      if (node.dataset.adaBound === '1') continue;
      node.dataset.adaBound = '1';
      node.addEventListener('toggle', function (event) {
        syncDisclosureState(event.currentTarget);
      });
      syncDisclosureState(node);
    }
  }

  /*
   * 对话流每 3 秒被 HTMX 整段替换，展开状态会丢。
   * 这里按"行类型 + 序号"记住哪些行是展开的，替换后恢复。
   */
  var openState = [];

  function snapshotOpenState() {
    var transcript = document.getElementById('transcript');
    if (!transcript) return;
    openState = [];
    var nodes = transcript.querySelectorAll('.ada-disclosure[open]');
    for (var i = 0; i < nodes.length; i += 1) {
      openState.push(nodes[i].dataset.kind || '');
    }
  }

  function restoreOpenState() {
    if (!openState.length) return;
    var transcript = document.getElementById('transcript');
    if (!transcript) return;
    var nodes = transcript.querySelectorAll('.ada-disclosure');
    for (var i = 0; i < nodes.length && i < openState.length; i += 1) {
      if ((nodes[i].dataset.kind || '') === openState[i]) {
        nodes[i].open = true;
        syncDisclosureState(nodes[i]);
      }
    }
  }

  /* ── 2. 复制按钮 ─────────────────────────────────────────────────────── */
  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var area = document.createElement('textarea');
      area.value = text;
      area.setAttribute('readonly', '');
      area.style.position = 'fixed';
      area.style.opacity = '0';
      document.body.appendChild(area);
      area.select();
      try {
        document.execCommand('copy');
        resolve();
      } catch (error) {
        reject(error);
      } finally {
        area.remove();
      }
    });
  }

  function handleCopy(button) {
    var text = button.getAttribute('data-copy') || '';
    if (!text) return;
    var original = button.getAttribute('data-copy-label') || button.textContent;
    copyText(text).then(
      function () {
        button.textContent = '已复制';
        window.setTimeout(function () {
          button.textContent = original;
        }, 1200);
      },
      function () {
        button.textContent = '复制失败';
        window.setTimeout(function () {
          button.textContent = original;
        }, 1200);
      }
    );
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    var button = target.closest('.ada-panel__copy, [data-copy]:not(.ada-panel__copy)');
    if (button) {
      handleCopy(button);
    }
  });

  /* ── 3. 输入框：聚焦态 + 发送可用性 + 自动增高 ───────────────────────── */
  function bindComposer() {
    var card = document.querySelector('.ada-composer__card');
    var input = document.querySelector('.ada-composer__input');
    var send = document.querySelector('.ada-send');
    if (!input) return;

    var syncSend = function () {
      if (send) send.disabled = input.value.trim().length === 0;
    };
    var grow = function () {
      input.style.height = 'auto';
      input.style.height = Math.min(input.scrollHeight, 180) + 'px';
    };

    input.addEventListener('input', function () {
      grow();
      syncSend();
    });
    input.addEventListener('keydown', function (event) {
      // IME 组合输入中不发送；Shift+Enter 换行。
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        var form = input.closest('form');
        if (form && input.value.trim().length > 0) form.requestSubmit();
      }
    });
    input.addEventListener('focus', function () {
      if (card) card.dataset.focused = 'true';
    });
    input.addEventListener('blur', function () {
      if (card) card.dataset.focused = 'false';
    });

    // 发送中：防重复提交。
    var form = input.closest('form');
    if (form && form.dataset.adaBound !== '1') {
      form.dataset.adaBound = '1';
      form.addEventListener('submit', function () {
        if (send) {
          send.disabled = true;
          send.dataset.sending = 'true';
        }
      });
    }

    grow();
    syncSend();
  }

  /* ── 4. 插件插槽注册表 ─────────────────────────────────────────────────
     服务端已经把插槽内容渲染进 HTML（SSR），这里把同一份状态暴露给
     客户端插件与调试面板，保证"服务端看到什么，客户端就能读到什么"。 */
  function loadSlotRegistry() {
    if (!window.fetch) return;
    window
      .fetch('/api/plugins', { headers: { Accept: 'application/json' } })
      .then(function (response) {
        return response.ok ? response.json() : null;
      })
      .then(function (data) {
        if (!data) return;
        window.AdaPlugins = data;
        var slots = document.querySelectorAll('.ada-slot');
        for (var i = 0; i < slots.length; i += 1) {
          var name = slots[i].getAttribute('data-slot');
          for (var j = 0; j < (data.slots || []).length; j += 1) {
            if (data.slots[j].slot === name) {
              slots[i].dataset.contributors = String(data.slots[j].contributors);
            }
          }
        }
      })
      .catch(function () {
        /* 插件注册表不可用不影响主界面 */
      });
  }

  /* ── 5. 任务规划面板：记住展开/收起偏好 ──────────────────────────────
     默认收起（展开态贴着正文会盖住字），但用户的选择要记住——
     与 desktop-ui 的 `a_da_todo_panel_collapsed` 同一个键。
     面板本体是 <details>，没有 JS 也能展开收起；这里只补"记住偏好"。 */
  var TODO_COLLAPSED_KEY = 'a_da_todo_panel_collapsed';

  function bindTodoPanel() {
    var panel = document.querySelector('.ada-todo');
    if (!panel) return;
    if (panel.dataset.adaBound === '1') return;
    panel.dataset.adaBound = '1';

    var readPref = function () {
      try {
        return window.localStorage.getItem(TODO_COLLAPSED_KEY);
      } catch (error) {
        return null;
      }
    };
    var writePref = function (collapsed) {
      try {
        window.localStorage.setItem(TODO_COLLAPSED_KEY, collapsed ? '1' : '0');
      } catch (error) {
        /* 隐私模式下不可用：忽略即可 */
      }
    };

    // 上次是展开态 → 这次直接展开。
    if (readPref() === '0') {
      panel.open = true;
    }
    var sync = function () {
      panel.setAttribute('data-state', panel.open ? 'expanded' : 'collapsed');
      writePref(!panel.open);
    };
    panel.addEventListener('toggle', sync);
    sync();

    // 复制按钮在 summary 里：点它不该顺带把面板开合。
    var copy = panel.querySelector('.ada-todo__copy');
    if (copy) {
      copy.addEventListener('click', function (event) {
        event.preventDefault();
        event.stopPropagation();
      });
    }
  }

  /* ── 启动 ───────────────────────────────────────────────────────────── */
  function boot() {
    bindDisclosures(document);
    bindComposer();
    bindTodoPanel();
    loadSlotRegistry();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }

  document.body.addEventListener('htmx:beforeSwap', function (event) {
    if (event.target && event.target.id === 'transcript') {
      snapshotOpenState();
    }
  });

  document.body.addEventListener('htmx:afterSwap', function (event) {
    if (event.target && event.target.id === 'transcript') {
      restoreOpenState();
      bindDisclosures(event.target);
    }
  });

  // 供插件与调试使用
  window.AdaComponents = {
    bindDisclosures: bindDisclosures,
    bindComposer: bindComposer,
    copyText: copyText,
  };
})();
