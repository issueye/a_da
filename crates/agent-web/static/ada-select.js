/*
 * <ada-select>：下拉选择（Web Component）
 * ---------------------------------------------------------------------------
 * 原生 `<select>` 的弹层由浏览器/系统渲染——样式不受控、各家键盘行为不同、深色主题下
 * 直接一块白色方块。这里用「触发按钮 + listbox + 隐藏 input」自己实现：表单照常提交
 * `name=value`，交互（点击、方向键、Home/End、Enter/Space、Esc、点外部收起）都封装在
 * 元素内部，不再依赖 document 级的 data-* 委托。
 *
 * 刻意不挂 shadow root：页面是 SSR + htmx 的，样式与 class 都在同一棵树上，这里要的是
 * 行为封装而不是样式隔离。
 */
(() => {
  'use strict';

  class AdaSelect extends HTMLElement {
    connectedCallback() {
      if (this._wired) return;
      this._wired = true;
      this._handleClick = (event) => this.onClick(event);
      this._handleKeydown = (event) => this.onKeydown(event);
      this.addEventListener('click', this._handleClick);
      this.addEventListener('keydown', this._handleKeydown);
    }

    disconnectedCallback() {
      if (!this._wired) return;
      this.removeEventListener('click', this._handleClick);
      this.removeEventListener('keydown', this._handleKeydown);
      this._wired = false;
    }

    get trigger() {
      return this.querySelector('[data-select-trigger]');
    }

    get menu() {
      return this.querySelector('[data-select-menu]');
    }

    get input() {
      return this.querySelector('[data-select-input]');
    }

    get options() {
      return Array.from(this.querySelectorAll('[data-select-menu] [data-value]'));
    }

    get activeOption() {
      return this.querySelector('[data-select-menu] .is-active');
    }

    get isOpen() {
      return this.classList.contains('is-open');
    }

    get value() {
      return this.input ? this.input.value : '';
    }

    set value(next) {
      const option = this.options.find((item) => item.getAttribute('data-value') === next);
      if (option) this.select(option, false);
    }

    openMenu() {
      closeAllSelects(this);
      if (!this.menu || !this.trigger) return;
      this.classList.add('is-open');
      this.menu.hidden = false;
      this.trigger.setAttribute('aria-expanded', 'true');
      const selected = this.menu.querySelector('.is-selected') || this.options[0];
      this.highlight(selected);
      if (selected && selected.scrollIntoView) {
        selected.scrollIntoView({ block: 'nearest' });
      }
    }

    closeMenu() {
      this.classList.remove('is-open');
      if (this.menu) this.menu.hidden = true;
      if (this.trigger) this.trigger.setAttribute('aria-expanded', 'false');
    }

    highlight(option) {
      this.options.forEach((item) => item.classList.toggle('is-active', item === option));
    }

    select(option, focusTrigger = true) {
      if (!option) return;
      const value = option.getAttribute('data-value') || '';
      this.options.forEach((item) => {
        const selected = item === option;
        item.classList.toggle('is-selected', selected);
        item.setAttribute('aria-selected', selected ? 'true' : 'false');
      });
      if (this.input) this.input.value = value;
      const label = this.querySelector('[data-select-label]');
      if (label) label.textContent = option.textContent.trim();
      this.closeMenu();
      this.highlight(option);
      if (focusTrigger && this.trigger) this.trigger.focus();
      this.dispatchEvent(
        new CustomEvent('ada:select', {
          bubbles: true,
          detail: { name: this.input ? this.input.name : '', value: value },
        })
      );
    }

    onClick(event) {
      const target = event.target;
      if (!target || !target.closest) return;
      if (target.closest('[data-select-trigger]')) {
        event.preventDefault();
        if (this.isOpen) this.closeMenu();
        else this.openMenu();
        return;
      }
      const option = target.closest('[data-select-menu] [data-value]');
      if (option) {
        event.preventDefault();
        this.select(option);
      }
    }

    onKeydown(event) {
      const options = this.options;
      const current = this.activeOption;
      const index = options.indexOf(current);

      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        if (!options.length) return;
        event.preventDefault();
        if (!this.isOpen) {
          this.openMenu();
          return;
        }
        const step = event.key === 'ArrowDown' ? 1 : -1;
        let next = index < 0 ? (step > 0 ? 0 : options.length - 1) : index + step;
        if (next < 0) next = 0;
        if (next >= options.length) next = options.length - 1;
        this.highlight(options[next]);
        if (options[next].scrollIntoView) {
          options[next].scrollIntoView({ block: 'nearest' });
        }
        return;
      }

      if (event.key === 'Home' || event.key === 'End') {
        if (!this.isOpen || !options.length) return;
        event.preventDefault();
        this.highlight(event.key === 'Home' ? options[0] : options[options.length - 1]);
        return;
      }

      // Enter/Space 必须拦掉，否则在表单里会顺手提交整张表单。
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        if (!this.isOpen) this.openMenu();
        else this.select(current);
        return;
      }

      if (event.key === 'Escape' && this.isOpen) {
        event.preventDefault();
        this.closeMenu();
        if (this.trigger) this.trigger.focus();
        return;
      }

      if (event.key === 'Tab' && this.isOpen) this.closeMenu();
    }
  }

  /* 文档级只留一个"点外面就收起"的监听（组件自己的点击在 onClick 里处理）。 */
  function closeAllSelects(except) {
    document.querySelectorAll('ada-select.is-open').forEach((element) => {
      if (element !== except && typeof element.closeMenu === 'function') {
        element.closeMenu();
      }
    });
  }

  document.addEventListener('click', (event) => {
    const target = event.target;
    if (target && target.closest && target.closest('ada-select')) return;
    closeAllSelects(null);
  });

  if (!customElements.get('ada-select')) customElements.define('ada-select', AdaSelect);

  window.AdaElements = window.AdaElements || {};
  window.AdaElements.closeAllSelects = closeAllSelects;
})();
