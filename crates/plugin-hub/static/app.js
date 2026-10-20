/*
 * plugin-hub 共享交互层（无构建步骤，模板里只挂 data-* 属性）
 *
 * 覆盖四件事：
 *  1. 顶部导航高亮：按 data-nav 前缀匹配，最长前缀获胜；
 *  2. 提示条：可关闭，并在一段时间后自动淡出；
 *  3. 复制按钮：data-copy（候选值）或 data-copy-target（选择器）；
 *  4. 确认对话框：form[data-confirm] 用原生 <dialog> 统一确认，
 *     不再散落 window.confirm / onsubmit 内联脚本。
 * 主题切换在 theme.js（必须阻塞加载以避免闪白）。
 */
(function () {
  'use strict';

  var root = document.documentElement;

  /* ── 1. 导航高亮 ─────────────────────────────────────────────────────── */
  function markActiveNav() {
    var path = window.location.pathname.replace(/\/+$/, '') || '/';
    var links = document.querySelectorAll('[data-nav]');
    var best = null;
    var bestLength = -1;

    for (var i = 0; i < links.length; i += 1) {
      var link = links[i];
      var target = link.getAttribute('data-nav');
      if (!target) continue;
      var targetPath = target.replace(/\/+$/, '') || '/';
      var matches =
        targetPath === '/'
          ? path === '/'
          : path === targetPath || path.indexOf(targetPath + '/') === 0;
      if (matches && targetPath.length > bestLength) {
        best = link;
        bestLength = targetPath.length;
      }
    }

    if (!best) return;
    best.classList.add('nav-link-active');
    best.setAttribute('aria-current', 'page');
  }

  /* ── 2. 提示条 ───────────────────────────────────────────────────────── */
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

  /* ── 3. 复制按钮 ─────────────────────────────────────────────────────── */
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
    var selector = button.getAttribute('data-copy-target');
    if (!text && selector) {
      var source = document.querySelector(selector);
      text = source ? source.textContent || '' : '';
    }
    text = text.trim();
    if (!text) return;

    var original = button.getAttribute('data-copy-label') || button.textContent;
    copyText(text).then(
      function () {
        button.textContent = '已复制';
        window.setTimeout(function () {
          button.textContent = original;
        }, 1500);
      },
      function () {
        button.textContent = '复制失败';
        window.setTimeout(function () {
          button.textContent = original;
        }, 1500);
      }
    );
  }

  /* ── 4. 共享确认对话框 ───────────────────────────────────────────────── */
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

    var copyButton = target.closest('[data-copy], [data-copy-target]');
    if (copyButton) {
      handleCopy(copyButton);
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

  function boot() {
    markActiveNav();
    wireDialog();
    autoDismissFlashes();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }

  // 供调试面板确认静态资源已加载（打包后日志是唯一证据）
  root.setAttribute('data-plugin-hub-ui', 'ready');
})();
