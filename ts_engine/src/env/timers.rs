/// 生成注入微内核环境的 `node:timers` / 定时器 Polyfill 脚本
pub fn get_timers_polyfill_script() -> &'static str {
    r#"
    (function() {
        let nextTimerId = 1;
        const activeTimers = new Map();

        function setTimeout(callback, delay = 0, ...args) {
            const id = nextTimerId++;
            const ms = Math.max(0, Number(delay) || 0);
            let cancelled = false;

            activeTimers.set(id, {
                cancel: () => { cancelled = true; }
            });

            if (typeof __native_sleep_async === 'function') {
                __native_sleep_async(String(ms)).then(() => {
                    if (!cancelled && activeTimers.has(id)) {
                        activeTimers.delete(id);
                        if (typeof callback === 'function') {
                            callback(...args);
                        } else if (typeof callback === 'string') {
                            (0, eval)(callback);
                        }
                    }
                }).catch(() => {
                    activeTimers.delete(id);
                });
            } else {
                // 原生 sleep 桥接尚未就绪时的降级同步队列
                Promise.resolve().then(() => {
                    if (!cancelled && activeTimers.has(id)) {
                        activeTimers.delete(id);
                        if (typeof callback === 'function') {
                            callback(...args);
                        }
                    }
                });
            }

            return id;
        }

        function clearTimeout(id) {
            const timer = activeTimers.get(id);
            if (timer) {
                timer.cancel();
                activeTimers.delete(id);
            }
        }

        function setInterval(callback, delay = 0, ...args) {
            const id = nextTimerId++;
            const ms = Math.max(0, Number(delay) || 0);
            let cancelled = false;

            activeTimers.set(id, {
                cancel: () => { cancelled = true; }
            });

            async function runIntervalLoop() {
                while (!cancelled && activeTimers.has(id)) {
                    if (typeof __native_sleep_async === 'function') {
                        await __native_sleep_async(String(ms));
                    } else {
                        await Promise.resolve();
                    }

                    if (cancelled || !activeTimers.has(id)) {
                        break;
                    }

                    try {
                        if (typeof callback === 'function') {
                            callback(...args);
                        } else if (typeof callback === 'string') {
                            (0, eval)(callback);
                        }
                    } catch (err) {
                        if (typeof console !== 'undefined' && console.error) {
                            console.error('[Interval Error]', err);
                        }
                    }
                }
            }

            runIntervalLoop();
            return id;
        }

        function clearInterval(id) {
            clearTimeout(id);
        }

        function setImmediate(callback, ...args) {
            return setTimeout(callback, 0, ...args);
        }

        function clearImmediate(id) {
            clearTimeout(id);
        }

        const timersPromises = {
            setTimeout: function(delay = 0, value) {
                const ms = Math.max(0, Number(delay) || 0);
                if (typeof __native_sleep_async === 'function') {
                    return __native_sleep_async(String(ms)).then(() => value);
                }
                return Promise.resolve(value);
            }
        };

        const timersModule = {
            setTimeout,
            clearTimeout,
            setInterval,
            clearInterval,
            setImmediate,
            clearImmediate,
            promises: timersPromises
        };

        // 挂载到全局
        globalThis.setTimeout = setTimeout;
        globalThis.clearTimeout = clearTimeout;
        globalThis.setInterval = setInterval;
        globalThis.clearInterval = clearInterval;
        globalThis.setImmediate = setImmediate;
        globalThis.clearImmediate = clearImmediate;
        globalThis.timers = timersModule;
    })();
    "#
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    #[test]
    fn test_timers_polyfill_in_boa() {
        let mut ctx = Context::default();
        let script = get_timers_polyfill_script();
        ctx.eval(Source::from_bytes(script)).expect("注入 timers polyfill 失败");

        // 验证 setTimeout 与 clearTimeout 基础逻辑
        let eval_res = ctx.eval(Source::from_bytes(r#"
            let executed = false;
            const tId = setTimeout(() => { executed = true; }, 100);
            const isNumberId = typeof tId === 'number';
            clearTimeout(tId);
            isNumberId;
        "#)).expect("eval setTimeout 失败");
        assert_eq!(eval_res.to_boolean(), true);
    }
}
