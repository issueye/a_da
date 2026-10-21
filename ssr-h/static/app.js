/*
 * 应用外壳交互层（无构建步骤，模板里只挂 data-* 属性）
 *
 * 覆盖三件事：
 *  1. 提示条：可关闭，并在一段时间后自动淡出；
 *  2. 确认对话框：form[data-confirm] 用原生 <dialog> 统一确认，
 *     不再散落 window.confirm / onsubmit 内联脚本；
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

  /* ── 2. 共享确认对话框 ───────────────────────────────────────────────── */
  var dialog = document.querySelector('[data-confirm-dialog]');
  var pendingConfirm = null;

  function openConfirm(message, onConfirm) {
    if (!dialog || typeof dialog.showModal !== 'function') {
      if (window.confirm(message)) onConfirm();
      return;
    }
    var messageNode = dialog.querySelector('[data-confirm-message]');
    if (messageNode) messageNode.textContent = message;
    pendingConfirm = onConfirm;
    dialog.showModal();
    var ok = dialog.querySelector('[data-confirm-ok]');
    if (ok) ok.focus();
  }

  function closeConfirm() {
    pendingConfirm = null;
    if (dialog && dialog.open) dialog.close();
  }

  function wireDialog() {
    if (!dialog) return;
    dialog.addEventListener('cancel', function (event) {
      event.preventDefault();
      closeConfirm();
    });
    var cancel = dialog.querySelector('[data-confirm-cancel]');
    if (cancel) cancel.addEventListener('click', closeConfirm);
    var ok = dialog.querySelector('[data-confirm-ok]');
    if (ok) {
      ok.addEventListener('click', function () {
        var action = pendingConfirm;
        closeConfirm();
        if (action) action();
      });
    }
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

  /* ── 3. 移动端侧栏抽屉（≤760px 由 CSS 启用） ─────────────────────────── */
  function sidebar() {
    return document.getElementById('chat-sidebar');
  }

  function backdrop() {
    return document.getElementById('chat-backdrop');
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
  }

  document.addEventListener('click', function (event) {
    var target = event.target;
    if (!target || !target.closest) return;
    if (target.closest('[data-sidebar-toggle]')) {
      setSidebar(true);
      return;
    }
    if (target.closest('#chat-backdrop')) {
      setSidebar(false);
    }
  });

  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    var side = sidebar();
    if (side && side.classList.contains('is-open')) {
      setSidebar(false);
    }
  });

  function boot() {
    wireDialog();
    autoDismissFlashes();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
