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
     现在按服务端渲染的 data-key（item id）记录。
     🔴 还要跨**整页跳转/刷新**保留（建议 02）：按键存 sessionStorage，
     按 threadId 命名空间隔离；存不进去（隐私模式）只影响恢复，不拦交互。 */
  var openKeys = Object.create(null);
  var OPEN_PREFIX = 'a_da_open:';
  var openPersistTimer = null;

  function openStateKey() {
    var transcript = document.getElementById('transcript');
    return (transcript && transcript.getAttribute('data-thread-id')) || '';
  }

  function restoreOpenKeys() {
    var thread = openStateKey();
    if (!thread) return;
    try {
      var saved = JSON.parse(sessionStorage.getItem(OPEN_PREFIX + thread) || '[]');
      if (!Array.isArray(saved)) return;
      for (var i = 0; i < saved.length; i += 1) {
        if (typeof saved[i] === 'string') openKeys[saved[i]] = true;
      }
    } catch (error) {
      /* 坏数据按全折叠处理 */
    }
  }

  function persistOpenKeys() {
    if (openPersistTimer) return;
    openPersistTimer = window.setTimeout(function () {
      openPersistTimer = null;
      var thread = openStateKey();
      if (!thread) return;
      try {
        sessionStorage.setItem(
          OPEN_PREFIX + thread,
          JSON.stringify(Object.keys(openKeys))
        );
      } catch (error) {
        /* 存不进去只影响刷新恢复 */
      }
    }, 200);
  }

  function syncDisclosureState(details) {
    details.setAttribute('data-state', details.open ? 'open' : 'closed');
    var key = details.getAttribute('data-key');
    if (!key) return;
    if (details.open) {
      openKeys[key] = true;
    } else {
      delete openKeys[key];
    }
    persistOpenKeys();
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
      // 有图片时允许"纯图片消息"：发送键可用、textarea 不再必填。
      var owner = input.closest('form');
      var hasImages = !!(owner && owner.querySelector('input[name="images"]'));
      if (send) send.disabled = input.value.trim().length === 0 && !hasImages;
      input.required = !hasImages;
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
        var hasImages = !!(form && form.querySelector('input[name="images"]'));
        if (form && (input.value.trim().length > 0 || hasImages)) form.requestSubmit();
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
      .fetch('/web-ui/frontend-plugins', { headers: { Accept: 'application/json' } })
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

  /* ── 工具输出：单层「展开全部」────────────────────────────────────────── */
  /* 全文服务端已经渲染进 DOM，这里只切 data-expanded：
     不重新请求，也不改布局宽度以外的东西。按钮文案自己记原始值。 */
  var toolOutputBound = false;
  function bindToolOutputs() {
    if (toolOutputBound) return;
    toolOutputBound = true;
    document.addEventListener('click', function (event) {
      var button = event.target.closest && event.target.closest('[data-ada-expand]');
      if (!button) return;
      var block = button.closest('.ada-tool-output');
      if (!block) return;
      event.preventDefault();
      if (!button.dataset.adaLabel) button.dataset.adaLabel = button.textContent;
      var expanded = block.getAttribute('data-expanded') !== 'true';
      block.setAttribute('data-expanded', expanded ? 'true' : 'false');
      button.setAttribute('aria-expanded', expanded ? 'true' : 'false');
      button.textContent = expanded ? '收起输出' : button.dataset.adaLabel;
    });
  }

  /* ── 运行中工具：每秒走一次耗时 ──────────────────────────────────────── */
  /* 每次轮询都会整段替换对话流，所以节点不能缓存——每轮重新查询，
     否则计时会停在上一批已经被换掉的节点上。 */
  function tickRunningElapsed() {
    var rows = document.querySelectorAll(
      '.ada-disclosure[data-phase="running"][data-started-at]'
    );
    for (var i = 0; i < rows.length; i += 1) {
      var started = parseInt(rows[i].getAttribute('data-started-at'), 10);
      if (!started) continue;
      var meta = rows[i].querySelector('.ada-disclosure__meta');
      if (!meta) continue;
      var diff = Date.now() - started;
      // 起点不合理（时钟漂移、单位写错）就保持服务端给的文案，不往外抛天文数字。
      if (!(diff >= 0 && diff < 86400000)) continue;
      var seconds = Math.floor(diff / 1000);
      meta.textContent =
        seconds >= 60
          ? '已 ' + Math.floor(seconds / 60) + ' 分 ' + (seconds % 60) + ' 秒'
          : '已 ' + seconds + 's';
    }
  }

  /* ── 「＋」菜单：点外面或按 Esc 收起 ────────────────────────────────── */
  /* details 自己只管开合：不处理"点别处"，菜单会一直挂在输入框上方。 */
  var plusBound = false;
  function bindPlusMenu() {
    if (plusBound) return;
    plusBound = true;
    document.addEventListener('click', function (event) {
      var menus = document.querySelectorAll('details.ada-plus[open]');
      for (var i = 0; i < menus.length; i += 1) {
        if (!menus[i].contains(event.target)) menus[i].open = false;
      }
    });
    document.addEventListener('keydown', function (event) {
      if (event.key !== 'Escape') return;
      var menus = document.querySelectorAll('details.ada-plus[open]');
      for (var i = 0; i < menus.length; i += 1) menus[i].open = false;
    });
  }

  /* ── 远程模型弹窗：按需加载 + 就地过滤 ───────────────────────────────── */
  /* 弹窗内容不走首屏：远端可能返回上百个模型，只有真的点开才值得去要。
     `ada:modal-open` 由 <ada-modal> 派发，这里把它转成 loader 的 htmx 事件。 */
  function bindModalSources() {
    document.addEventListener('ada:modal-open', function (event) {
      var id = event.detail && event.detail.id;
      if (!id || !window.htmx) return;
      var source = document.querySelector('[data-modal-src="' + id + '"]');
      if (source) window.htmx.trigger(source, 'ada-modal-load');
    });
  }

  /* `?dialog=<id>`：从弹窗里提交表单后回跳，弹窗要自己站起来。 */
  function openDialogFromQuery() {
    var match = /[?&]dialog=([^&]+)/.exec(window.location.search || '');
    if (!match) return;
    var id = decodeURIComponent(match[1]);
    if (!id) return;
    window.setTimeout(function () {
      if (window.AdaElements) window.AdaElements.openModal(id);
    }, 0);
  }

  /* 搜索只在本地过滤已渲染的行，不再请求一次：
     94 行 DOM 过滤是微秒级，往返一次要几百毫秒。 */
  function applyRemoteFilter(input) {
    var root = input.closest('.ada-remote');
    if (!root) return;
    var rows = root.querySelectorAll('[data-remote-row]');
    var needle = input.value.trim().toLowerCase();
    var visible = 0;
    for (var i = 0; i < rows.length; i += 1) {
      var name = rows[i].getAttribute('data-name') || '';
      var hit = !needle || name.indexOf(needle) !== -1;
      rows[i].hidden = !hit;
      if (hit) visible += 1;
    }
    var groups = root.querySelectorAll('.ada-remote__group');
    for (var g = 0; g < groups.length; g += 1) {
      groups[g].hidden = !groups[g].querySelector('[data-remote-row]:not([hidden])');
    }
    var count = root.querySelector('[data-remote-count]');
    if (count) {
      count.textContent = needle ? visible + ' / ' + rows.length : rows.length + ' 个模型';
    }
  }

  function syncRemoteCount() {
    var root = document.querySelector('.ada-remote');
    if (!root) return;
    var rows = root.querySelectorAll('[data-remote-row]');
    var count = root.querySelector('[data-remote-count]');
    if (count && rows.length) count.textContent = rows.length + ' 个模型';
  }

  var remoteFilterBound = false;
  function bindRemoteFilter() {
    if (remoteFilterBound) return;
    remoteFilterBound = true;
    document.addEventListener('input', function (event) {
      var input = event.target;
      if (!input || !input.matches || !input.matches('[data-remote-search]')) return;
      applyRemoteFilter(input);
    });
    // htmx 换完列表：搜索框里已经有词就重新套一次，否则只更新计数。
    document.body.addEventListener('htmx:afterSwap', function (event) {
      var target = event.target;
      if (!target || target.id !== 'remote-models-body') return;
      var input = document.querySelector('[data-remote-search]');
      if (input && input.value.trim()) applyRemoteFilter(input);
      else syncRemoteCount();
    });
  }

  /* ── 8. live 轮询后的重建 ────────────────────────────────────────────── */
  function watchTranscript() {
    var transcript = document.getElementById('transcript');
    if (!transcript || !window.MutationObserver) return;
    new MutationObserver(function () {
      bindDisclosures(transcript);
    }).observe(transcript, { childList: true });
  }

  /* ── 9. 页签（Tabs）────────────────────────────────────────────────────
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

  /* ── 行级操作菜单（表格里的 SVG 省略号）────────────────────────────── */
  function closeRowMenus(except) {
    var menus = document.querySelectorAll('details.ada-row-menu[open]');
    for (var i = 0; i < menus.length; i += 1) {
      if (menus[i] !== except) menus[i].open = false;
    }
  }

  function positionRowMenu(menu) {
    var panel = menu.querySelector('.ada-row-menu__panel');
    if (!panel) return;
    var rect = menu.getBoundingClientRect();
    var width = panel.offsetWidth || 156;
    panel.style.position = 'fixed';
    panel.style.right = 'auto';
    panel.style.left =
      Math.max(8, Math.min(window.innerWidth - width - 8, rect.right - width)) + 'px';
    panel.style.top = rect.bottom + 6 + 'px';
    var panelRect = panel.getBoundingClientRect();
    if (panelRect.bottom > window.innerHeight - 8) {
      panel.style.top = Math.max(8, rect.top - panelRect.height - 6) + 'px';
    }
  }

  function bindRowMenus(root) {
    var menus = (root || document).querySelectorAll('details.ada-row-menu');
    for (var i = 0; i < menus.length; i += 1) {
      var menu = menus[i];
      if (menu._adaRowMenuBound) continue;
      menu._adaRowMenuBound = true;
      menu.addEventListener('toggle', function () {
        if (!this.open) return;
        closeRowMenus(this);
        var current = this;
        window.requestAnimationFrame(function () {
          positionRowMenu(current);
        });
      });
    }
  }

  var rowMenuDocumentBound = false;
  function bindRowMenuDocument() {
    if (rowMenuDocumentBound) return;
    rowMenuDocumentBound = true;
    document.addEventListener('click', function (event) {
      var menu = event.target.closest && event.target.closest('details.ada-row-menu');
      if (!menu) {
        closeRowMenus();
        return;
      }
      // 点面板里的动作后收起；编辑按钮先由 modal 控制器打开，延迟关闭不会影响它。
      if (event.target.closest('.ada-row-menu__panel')) {
        var current = menu;
        window.setTimeout(function () {
          current.open = false;
        }, 0);
      }
    });
    document.addEventListener('keydown', function (event) {
      if (event.key === 'Escape') closeRowMenus();
    });
    window.addEventListener('resize', function () {
      closeRowMenus();
    });
    window.addEventListener(
      'scroll',
      function () {
        closeRowMenus();
      },
      true
    );
  }

  /* ── 启动 ───────────────────────────────────────────────────────────── */
  function boot() {
    restoreOpenKeys();
    bindDisclosures(document);
    bindComposer();
    bindTabs(document);
    bindRowMenus(document);
    bindRowMenuDocument();
    bindToolOutputs();
    bindPlusMenu();
    bindModalSources();
    bindRemoteFilter();
    openDialogFromQuery();
    // 运行中的工具每秒刷新一次耗时：1s 粒度足够，也能让"它还在跑"看得见。
    window.setInterval(tickRunningElapsed, 1000);
    watchTranscript();
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
    bindRowMenus: bindRowMenus,
    bindToolOutputs: bindToolOutputs,
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
