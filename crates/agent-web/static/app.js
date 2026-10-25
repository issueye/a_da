/*
 * 应用外壳交互层（无构建步骤，模板里只挂 data-* 属性）
 *
 * 覆盖三件事：
 *  1. 提示条：可关闭，并在一段时间后自动淡出；
 *  2. 确认弹窗：`form[data-confirm]` / `a[data-confirm]` 统一走**组件库**的纯 DOM 弹窗
 *     （`ui::ConfirmDialog`；不用原生 `<dialog>`，更不用 `window.confirm`），
 *     不再散落内联 onsubmit 脚本；
 *  3. 移动端侧栏抽屉。
 *
 * 复制按钮（data-copy）与组件行为在 components.js，这里不重复绑定——
 * 两份 document 级委托会让每次点击写两次剪贴板。
 * 主题切换在 theme.js（必须阻塞加载以避免闪白）。
 */
(function () {
  'use strict';

  /* ── 1. 提示条 ───────────────────────────────────────────────────────── */
  function dismiss(target) {
    if (!target) return;
    target.style.transition = 'opacity 160ms ease, transform 160ms ease';
    target.style.opacity = '0';
    target.style.transform = 'translateY(-2px)';
    window.setTimeout(function () {
      target.remove();
    }, 180);
  }

  function autoDismissFlashes() {
    var flashes = document.querySelectorAll('[data-flash]');
    for (var i = 0; i < flashes.length; i += 1) {
      (function (flash) {
        var delay = flash.getAttribute('data-flash') === 'error' ? 9000 : 6000;
        window.setTimeout(function () {
          if (flash.isConnected) dismiss(flash);
        }, delay);
      })(flashes[i]);
    }
  }

  /* ── 2. 共享确认弹窗 ─────────────────────────────────────────────────
     弹窗本体是组件库的 `ui::ConfirmDialog`（`data-modal="confirm"`）；开合、焦点圈、
     Esc、遮罩与滚动锁定由 `components.js` 的 `[data-modal]` 控制器统一负责。
     这里只做三件事：记下待确认动作、写入文案、确认后执行。
     🔴 **不要退回 window.confirm**：原生对话框不可样式化、会阻塞整页脚本，而且同一个
     "删除"在聊天页与二级页会变成两种观感（历史 bug）。组件缺失＝模板漏渲染，直接报错。 */
  var dialog = document.querySelector('[data-confirm-dialog]');
  var pendingConfirm = null;

  function openConfirm(message, onConfirm) {
    if (!dialog || !window.AdaComponents || typeof window.AdaComponents.openModal !== 'function') {
      console.error('[ssr-h] 页面缺少 [data-confirm-dialog] 组件，确认动作已取消');
      return;
    }
    var messageNode = dialog.querySelector('[data-confirm-message]');
    if (messageNode) messageNode.textContent = message;
    pendingConfirm = onConfirm;
    window.AdaComponents.openModal('confirm');
  }

  function closeConfirm() {
    pendingConfirm = null;
    if (window.AdaComponents) window.AdaComponents.closeModal('confirm');
  }

  function wireDialog() {
    if (!dialog) return;
    dialog.addEventListener('click', function (event) {
      var target = event.target;
      if (!target || !target.closest) return;
      // 取消/遮罩：关闭交给 data-modal-close，这里只清掉待确认动作。
      if (target.closest('[data-confirm-cancel]')) {
        pendingConfirm = null;
        return;
      }
      if (target.closest('[data-confirm-ok]')) {
        var action = pendingConfirm;
        closeConfirm();
        if (action) action();
      }
    });
    // Esc / 点遮罩关闭时控制器会派发关闭事件，用来清掉悬着的动作。
    document.addEventListener('ada:modal-close', function (event) {
      if (event.detail && event.detail.id === 'confirm') pendingConfirm = null;
    });
  }

  document.addEventListener(
    'submit',
    function (event) {
      var form = event.target;
      if (!form || !form.matches || !form.matches('form[data-confirm]')) return;
      if (form.dataset.confirmed === '1') {
        delete form.dataset.confirmed;
        return;
      }
      event.preventDefault();
      var message = form.getAttribute('data-confirm') || '确认执行这个操作？';
      openConfirm(message, function () {
        form.dataset.confirmed = '1';
        form.requestSubmit();
      });
    },
    true
  );

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;

    var dismissButton = target.closest('[data-dismiss]');
    if (dismissButton) {
      dismiss(dismissButton.closest('[data-flash]'));
      return;
    }

    var confirmedLink = target.closest('a[data-confirm]');
    if (confirmedLink) {
      event.preventDefault();
      openConfirm(confirmedLink.getAttribute('data-confirm'), function () {
        window.location.href = confirmedLink.href;
      });
    }
  });

  /* 排序下拉框等：改选即提交 */
  document.addEventListener('change', function (event) {
    var field = event.target;
    if (!field || !field.matches || !field.matches('[data-auto-submit]')) {
      return;
    }
    if (field.form) field.form.submit();
  });

  /* ── 3. 侧栏：≤760px 是抽屉，>760px 是 56px 窄轨 ───────────────────────
     两套形态共用一个按钮（data-sidebar-toggle），但状态不同：
       · 窄屏：.ada-sidebar.is-open + 遮罩，点遮罩/Esc 关闭；
       · 宽屏：.ada-shell.is-sidebar-collapsed（CSS 里收成窄轨，不是隐藏），
         偏好存 localStorage。窄轨的目的是"收起不等于失去导航"，
         所以别再改回 display:none —— 那会让折叠 = 自断一臂。 */
  var COLLAPSE_KEY = 'a_da_sidebar_collapsed';
  var narrowQuery = window.matchMedia('(max-width: 760px)');

  function sidebar() {
    return document.getElementById('chat-sidebar');
  }

  function backdrop() {
    return document.getElementById('chat-backdrop');
  }

  function shell() {
    return document.querySelector('.ada-shell');
  }

  function readCollapsed() {
    try {
      return window.localStorage.getItem(COLLAPSE_KEY) === '1';
    } catch (error) {
      return false;
    }
  }

  function writeCollapsed(collapsed) {
    try {
      window.localStorage.setItem(COLLAPSE_KEY, collapsed ? '1' : '0');
    } catch (error) {
      /* 隐私模式下不可用：忽略即可 */
    }
  }

  function isCollapsed() {
    var wrap = shell();
    return !!(wrap && wrap.classList.contains('is-sidebar-collapsed'));
  }

  /* 按钮的无障碍状态要跟真实形态一致，否则读屏用户听到的是"打开侧栏"却按不开 */
  function syncToggle() {
    var button = document.querySelector('[data-sidebar-toggle]');
    if (!button) return;
    var narrow = narrowQuery.matches;
    var open = narrow
      ? !!(sidebar() && sidebar().classList.contains('is-open'))
      : !isCollapsed();
    var label = narrow
      ? (open ? '关闭侧栏' : '打开侧栏')
      : (open ? '收起侧栏' : '展开侧栏');
    button.setAttribute('aria-expanded', open ? 'true' : 'false');
    button.setAttribute('aria-label', label);
    button.setAttribute('title', label);
  }

  function setSidebar(open) {
    var side = sidebar();
    var veil = backdrop();
    if (!side || !veil) return;
    side.classList.toggle('is-open', open);
    veil.classList.toggle('is-open', open);
    if (open) {
      var search = side.querySelector('.ada-search__input');
      if (search) search.focus();
    }
    syncToggle();
  }

  function setCollapsed(collapsed) {
    var wrap = shell();
    if (!wrap) return;
    wrap.classList.toggle('is-sidebar-collapsed', collapsed);
    writeCollapsed(collapsed);
    syncToggle();
  }

  /* 跨断点时清理另一套形态的状态，避免"抽屉开着 + 折叠着"同时成立 */
  function onBreakpointChange() {
    var side = sidebar();
    var veil = backdrop();
    if (narrowQuery.matches) {
      var wrap = shell();
      if (wrap) wrap.classList.remove('is-sidebar-collapsed');
    } else {
      if (side) side.classList.remove('is-open');
      if (veil) veil.classList.remove('is-open');
      setCollapsed(readCollapsed());
    }
    syncToggle();
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    if (target.closest('[data-sidebar-toggle]')) {
      if (narrowQuery.matches) {
        setSidebar(!(sidebar() && sidebar().classList.contains('is-open')));
      } else {
        setCollapsed(!isCollapsed());
      }
      return;
    }
    if (target.closest('#chat-backdrop')) {
      setSidebar(false);
    }
  });

  if (typeof narrowQuery.addEventListener === 'function') {
    narrowQuery.addEventListener('change', onBreakpointChange);
  } else if (typeof narrowQuery.addListener === 'function') {
    narrowQuery.addListener(onBreakpointChange);
  }

  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    // 弹窗开着时 Esc 归弹窗控制器管，别顺手把侧栏也收起来。
    if (window.AdaComponents && window.AdaComponents.modalOpen && window.AdaComponents.modalOpen()) {
      return;
    }
    var side = sidebar();
    if (side && side.classList.contains('is-open')) {
      setSidebar(false);
    }
  });

  function boot() {
    wireDialog();
    autoDismissFlashes();
    // 恢复上次的折叠偏好（宽屏）并让按钮文案与真实状态一致
    onBreakpointChange();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
