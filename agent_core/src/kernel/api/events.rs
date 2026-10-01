/// 生成注入微内核环境的 `node:events` / `events` 模块 Polyfill 脚本
pub fn get_events_polyfill_script() -> &'static str {
    r#"
    (function() {
        if (typeof globalThis.EventEmitter !== 'undefined') return;

        class EventEmitter {
            constructor() {
                this._events = Object.create(null);
            }

            on(event, listener) {
                if (typeof listener !== 'function') {
                    throw new TypeError('Listener must be a function');
                }
                if (!this._events[event]) {
                    this._events[event] = [];
                }
                this._events[event].push(listener);
                return this;
            }

            addListener(event, listener) {
                return this.on(event, listener);
            }

            once(event, listener) {
                if (typeof listener !== 'function') {
                    throw new TypeError('Listener must be a function');
                }
                const wrapper = (...args) => {
                    this.removeListener(event, wrapper);
                    listener.apply(this, args);
                };
                wrapper.listener = listener;
                return this.on(event, wrapper);
            }

            emit(event, ...args) {
                const listeners = this._events[event];
                if (!listeners || listeners.length === 0) {
                    return false;
                }
                const copy = listeners.slice();
                for (let i = 0; i < copy.length; i++) {
                    copy[i].apply(this, args);
                }
                return true;
            }

            removeListener(event, listener) {
                if (typeof listener !== 'function') {
                    throw new TypeError('Listener must be a function');
                }
                const listeners = this._events[event];
                if (!listeners) return this;

                const index = listeners.findIndex(l => l === listener || l.listener === listener);
                if (index !== -1) {
                    listeners.splice(index, 1);
                    if (listeners.length === 0) {
                        delete this._events[event];
                    }
                }
                return this;
            }

            off(event, listener) {
                return this.removeListener(event, listener);
            }

            removeAllListeners(event) {
                if (event) {
                    delete this._events[event];
                } else {
                    this._events = Object.create(null);
                }
                return this;
            }

            listenerCount(event) {
                const listeners = this._events[event];
                return listeners ? listeners.length : 0;
            }
        }

        globalThis.EventEmitter = EventEmitter;
    })();
    "#
}
