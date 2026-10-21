/*
 * a_da 组件库 · 客户端行为
 * ---------------------------------------------------------------------------
 * 组件的 DOM 与状态语义在服务端渲染（src/ui/），这里只做四件事：
 *   1. 把"状态"同步到 data-*（折叠开合、聚焦），供样式与插件读取；
 *   2. 渐进增强：没有 JS 时页面依然完整可读；
 *   3. live 轮询替换对话流后，按 **item id** 恢复展开状态（不是按下标）；
 *   4. 插件插槽注册表：把服务端渲染的插槽暴露给插件与调试面板。
 *
 * 刻意不做的事：不接管业务动作（发送/审批/重试都走原生表单提交），
 * 不做虚拟 DOM，不引入框架。
 */
(function () {
  'use strict';

  /* ── 1. 折叠行：整行可点 + 状态同步 ─────────────────────────────────── */
  /* live 轮询会整段替换 #transcript，开合状态必须跨替换保留。
     旧实现按"行类型 + 下标"恢复，打开第 2 个工具行会被恢复到第 1 个；
     现在按服务端渲染的 data-key（item id）记录。 */
  var openKeys = Object.create(null);

  function syncDisclosureState(details) {
    details.setAttribute('data-state', details.open ? 'open' : 'closed');
    var key = details.getAttribute('data-key');
    if (!key) return;
    if (details.open) {
      openKeys[key] = true;
    } else {
      delete openKeys[key];
    }
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
      var key = node.getAttribute('data-key');
      if (key && openKeys[key]) {
        node.open = true;
      }
      syncDisclosureState(node);
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

  /* 只改文字节点，别把按钮里的图标一起抹掉。 */
  function feedback(button, text) {
    var label = button.querySelector('span') || button;
    var original = label.textContent;
    label.textContent = text;
    window.setTimeout(function () {
      label.textContent = original;
    }, 1200);
  }

  function handleCopy(button) {
    var text = button.getAttribute('data-copy') || '';
    if (!text) return;
    copyText(text).then(
      function () {
        feedback(button, '已复制');
      },
      function () {
        feedback(button, '复制失败');
      }
    );
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    var button = target.closest('[data-copy]');
    if (button) {
      handleCopy(button);
    }
  });

  /* 下拉选择与弹窗已升级为 Web Components（见 static/elements.js）：
     `<ada-select>` 与 `<ada-modal>`。这里只保留非元素化的行为（折叠、输入框、复制、插槽）。 */

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

  /* ── 5. 任务规划面板：记住展开/收起偏好 ────────────────────────────── */
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

    // 上次是展开态 → 这次直接展开（live 轮询替换面板后同样适用）。
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
    // 注意**不要** stopPropagation —— 复制动作是 document 上的委托，
    // 截断冒泡会让"复制"永远不执行（历史 bug）。
    var copy = panel.querySelector('.ada-todo__copy');
    if (copy) {
      copy.addEventListener('click', function (event) {
        event.preventDefault();
      });
    }
  }

  /* ── 6. live 轮询后的重建 ────────────────────────────────────────────── */
  function watchTranscript() {
    var transcript = document.getElementById('transcript');
    if (!transcript || !window.MutationObserver) return;
    new MutationObserver(function () {
      bindDisclosures(transcript);
    }).observe(transcript, { childList: true });
  }

  function watchTodoDock() {
    var dock = document.getElementById('ada-todo-dock');
    if (!dock || !window.MutationObserver) return;
    new MutationObserver(function () {
      bindTodoPanel();
    }).observe(dock, { childList: true });
  }

  /* ── 7. 页签（Tabs）────────────────────────────────────────────────────
     服务端已按 `?tab=` 渲染好展开项（无 JS 时页签就是普通链接，点一下整页刷新）。
     这里只做渐进增强：点击/方向键就地切换面板，并用 history.replaceState 把地址栏
     换成该页签自己的 href —— 刷新、收藏、分享都还落在同一个页签上。 */
  function closestTab(node, list) {
    while (node && node !== list) {
      if (node.nodeType === 1 && node.getAttribute('role') === 'tab') return node;
      node = node.parentNode;
    }
    return null;
  }

  function bindTabList(list) {
    if (list.dataset.adaBound === '1') return;
    list.dataset.adaBound = '1';

    var tabs = list.querySelectorAll('[role="tab"][aria-controls]');
    if (!tabs.length) return;

    var indexOf = function (tab) {
      for (var i = 0; i < tabs.length; i += 1) {
        if (tabs[i] === tab) return i;
      }
      return -1;
    };

    var select = function (tab, focus, syncUrl) {
      for (var i = 0; i < tabs.length; i += 1) {
        var current = tabs[i];
        var active = current === tab;
        current.setAttribute('aria-selected', active ? 'true' : 'false');
        current.setAttribute('tabindex', active ? '0' : '-1');
        var panel = document.getElementById(current.getAttribute('aria-controls'));
        if (!panel) continue;
        if (active) {
          panel.removeAttribute('hidden');
        } else {
          panel.setAttribute('hidden', '');
        }
      }
      if (focus) tab.focus();
      if (!syncUrl) return;
      var href = tab.getAttribute('href');
      if (href && window.history && window.history.replaceState) {
        window.history.replaceState(null, '', href);
      }
    };

    list.addEventListener('click', function (event) {
      // 保留"新标签页打开 / 新窗口打开"这类浏览器原生意图，不拦。
      if (event.defaultPrevented || event.button !== 0) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      var tab = closestTab(event.target, list);
      if (!tab) return;
      event.preventDefault();
      select(tab, false, true);
    });

    list.addEventListener('keydown', function (event) {
      var tab = closestTab(event.target, list);
      if (!tab) return;
      var index = indexOf(tab);
      if (index < 0) return;
      var next = -1;
      if (event.key === 'ArrowRight') {
        next = (index + 1) % tabs.length;
      } else if (event.key === 'ArrowLeft') {
        next = (index - 1 + tabs.length) % tabs.length;
      } else if (event.key === 'Home') {
        next = 0;
      } else if (event.key === 'End') {
        next = tabs.length - 1;
      } else if (event.key === 'Enter' || event.key === ' ' || event.key === 'Spacebar') {
        // 链接的 Enter 会自己触发 click；Space 不会，这里补上。
        event.preventDefault();
        select(tab, true, true);
        return;
      } else {
        return;
      }
      event.preventDefault();
      select(tabs[next], true, true);
    });
  }

  function bindTabs(root) {
    var lists = (root || document).querySelectorAll('[data-ada-tabs]');
    for (var i = 0; i < lists.length; i += 1) {
      bindTabList(lists[i]);
    }
  }

  /* ── 启动 ───────────────────────────────────────────────────────────── */
  function boot() {
    bindDisclosures(document);
    bindComposer();
    bindTodoPanel();
    bindTabs(document);
    watchTranscript();
    watchTodoDock();
    loadSlotRegistry();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }

  // 供插件与调试使用。
  // 下拉与弹窗的实现在 elements.js 的 Web Components 里（`<ada-select>` / `<ada-modal>`），
  // 这里只转一层，保持老调用点（app.js 的确认框）不变。
  window.AdaComponents = {
    bindDisclosures: bindDisclosures,
    bindComposer: bindComposer,
    bindTabs: bindTabs,
    copyText: copyText,
    closeSelects: function () {
      if (window.AdaElements) window.AdaElements.closeAllSelects(null);
    },
    openModal: function (id) {
      if (window.AdaElements) window.AdaElements.openModal(id);
    },
    closeModal: function (id) {
      if (window.AdaElements) window.AdaElements.closeModal(id);
    },
    modalOpen: function () {
      return !!(window.AdaElements && window.AdaElements.modalOpen());
    },
  };
})();
