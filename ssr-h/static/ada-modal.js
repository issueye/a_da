/*
 * <ada-modal>：弹窗（Web Component）
 * ---------------------------------------------------------------------------
 * 确认框（`data-modal="confirm"`）与普通表单弹窗（模型编辑/新增）共用同一个元素。
 * 开合、模态栈、焦点圈、Esc、点遮罩、页面滚动锁定都在元素内部实现；
 * 业务侧只用 `data-modal-open="<id>"` / `data-modal-close` 或 openModal/closeModal。
 *
 * 刻意不挂 shadow root：SSR + htmx 的页面里样式与 class 都在同一棵树，要的是行为封装。
 */
(() => {
  'use strict';

  const FOCUSABLE =
    'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

  /* 模态栈：只有栈顶那个响应 Esc / Tab，避免一次关掉两层。 */
  const openModals = [];

  class AdaModal extends HTMLElement {
    connectedCallback() {
      if (this._wired) return;
      this._wired = true;
      this._handleClick = (event) => {
        const target = event.target;
        if (target && target.closest && target.closest('[data-modal-close]')) {
          this.close();
        }
      };
      this.addEventListener('click', this._handleClick);
    }

    disconnectedCallback() {
      if (!this._wired) return;
      this.removeEventListener('click', this._handleClick);
      this._wired = false;
      const index = openModals.indexOf(this);
      if (index >= 0) openModals.splice(index, 1);
    }

    get isOpen() {
      return !this.hasAttribute('hidden');
    }

    get modalId() {
      return this.getAttribute('data-modal') || '';
    }

    open() {
      if (this.isOpen) return;
      openModals.push(this);
      this.removeAttribute('hidden');
      this.classList.add('is-open');
      document.body.classList.add('ada-modal-open');
      const first =
        this.querySelector('[data-modal-autofocus]') || this.querySelector(FOCUSABLE);
      if (first) first.focus();
      document.dispatchEvent(
        new CustomEvent('ada:modal-open', { detail: { id: this.modalId } })
      );
    }

    close() {
      if (!this.isOpen) return;
      const index = openModals.indexOf(this);
      if (index >= 0) openModals.splice(index, 1);
      this.setAttribute('hidden', '');
      this.classList.remove('is-open');
      if (!openModals.length) document.body.classList.remove('ada-modal-open');
      document.dispatchEvent(
        new CustomEvent('ada:modal-close', { detail: { id: this.modalId } })
      );
    }
  }

  function modalById(id) {
    return document.querySelector('ada-modal[data-modal="' + id + '"]');
  }

  function openModal(id) {
    const element = modalById(id);
    if (!element) {
      console.error('[ssr-h] 找不到弹窗组件:', id);
      return;
    }
    element.open();
  }

  function closeModal(id) {
    const element = modalById(id);
    if (element) element.close();
  }

  function modalOpen() {
    return openModals.length > 0;
  }

  function topModal() {
    return openModals.length ? openModals[openModals.length - 1] : null;
  }

  /* `[data-modal-open="<id>"]` 是声明式入口（按钮、链接都能用）。 */
  document.addEventListener('click', (event) => {
    const target = event.target;
    if (!target || !target.closest) return;
    const opener = target.closest('[data-modal-open]');
    if (!opener) return;
    event.preventDefault();
    openModal(opener.getAttribute('data-modal-open'));
  });

  document.addEventListener('keydown', (event) => {
    const modal = topModal();
    if (!modal) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      modal.close();
      return;
    }
    if (event.key !== 'Tab') return;
    const nodes = modal.querySelectorAll(FOCUSABLE);
    if (!nodes.length) return;
    const first = nodes[0];
    const last = nodes[nodes.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });

  if (!customElements.get('ada-modal')) customElements.define('ada-modal', AdaModal);

  window.AdaElements = window.AdaElements || {};
  window.AdaElements.openModal = openModal;
  window.AdaElements.closeModal = closeModal;
  window.AdaElements.modalOpen = modalOpen;
})();
