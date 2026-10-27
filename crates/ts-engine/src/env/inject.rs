use boa_engine::{Context, Source};
use std::path::Path;

use super::*;
/// 按声明注入环境（**唯一入口**；`process`/`path`/`Buffer`/`url`/`util`/`assert`/`timers`
/// 这些纯计算或信息类能力始终注入，见 [`Needs`] 的说明）。
pub fn inject_environment(
    ctx: &mut Context,
    workspace: Option<&Path>,
    needs: Needs,
) -> Result<(), String> {
    inject_common(ctx, workspace, needs)
}

/// 全量注入（保留给 CLI 与既有调用点；插件路径请用 [`inject_environment`]）。
pub fn inject_node_environment(ctx: &mut Context, workspace: Option<&Path>) -> Result<(), String> {
    inject_environment(ctx, workspace, Needs::everything())
}

fn inject_common(ctx: &mut Context, workspace: Option<&Path>, needs: Needs) -> Result<(), String> {
    // 1. 注入 process 全局对象
    let process_script = process::get_process_polyfill_script(workspace);
    ctx.eval(Source::from_bytes(&process_script))
        .map_err(|e| format!("注入 process 失败: {e}"))?;

    // 2. 注入 node:path
    let path_script = path::get_path_polyfill_script();
    ctx.eval(Source::from_bytes(path_script))
        .map_err(|e| format!("注入 path 失败: {e}"))?;

    // 3. 注入 Buffer
    let buffer_script = buffer::get_buffer_polyfill_script();
    ctx.eval(Source::from_bytes(buffer_script))
        .map_err(|e| format!("注入 Buffer 失败: {e}"))?;

    // 4. 注入 EventEmitter
    let events_script = events::get_events_polyfill_script();
    ctx.eval(Source::from_bytes(events_script))
        .map_err(|e| format!("注入 events 失败: {e}"))?;

    // 5. 注入 node:os
    let os_script = os::get_os_polyfill_script();
    ctx.eval(Source::from_bytes(&os_script))
        .map_err(|e| format!("注入 os 失败: {e}"))?;

    // 6. 注册同步 fs 原生函数与封装脚本（**按声明**：未声明 fs 就不注册）
    if needs.fs_read || needs.fs_write {
        fs::register_fs_native_and_script(ctx, workspace)?;
    }

    // 6.5 注册 SQLite 原生绑定（**按声明**：`@ada-needs sqlite`）
    if needs.sqlite {
        sqlite::register_sqlite_native_and_script(ctx, workspace)?;
    }

    // 6.6 注册 child_process 同步子集（**按声明**：`@ada-needs exec`）
    if needs.exec {
        child_process::register_child_process_native_and_script(ctx, workspace)?;
    }

    // 7. 注册 crypto 原生哈希与加密能力
    crypto::register_crypto_native_and_script(ctx)?;

    // 8. 注入 url 模块 (URL, URLSearchParams, fileURLToPath, pathToFileURL)
    let url_script = url::get_url_polyfill_script();
    ctx.eval(Source::from_bytes(url_script))
        .map_err(|e| format!("注入 url 失败: {e}"))?;

    // 9. 注入 util 模块 (format, promisify, types, inspect, inherits)
    let util_script = util::get_util_polyfill_script();
    ctx.eval(Source::from_bytes(util_script))
        .map_err(|e| format!("注入 util 失败: {e}"))?;

    // 10. 注入 assert 模块 (assert, ok, strictEqual, deepStrictEqual, throws)
    let assert_script = assert::get_assert_polyfill_script();
    ctx.eval(Source::from_bytes(assert_script))
        .map_err(|e| format!("注入 assert 失败: {e}"))?;

    // 11. 注入 timers 模块与全局定时器 (setTimeout, clearTimeout, setInterval, clearInterval)
    let timers_script = timers::get_timers_polyfill_script();
    ctx.eval(Source::from_bytes(timers_script))
        .map_err(|e| format!("注入 timers 失败: {e}"))?;

    // 12. 注入全局 Web API AbortSignal, AbortController, fetch 与 Response
    //
    // 🔴 **按声明注入**（SP-13）：`needs.net == false` 时整段不注入，于是 `fetch` 在沙箱里
    // **根本不存在**（`typeof fetch === 'undefined'`）。这是插件唯一能把数据送出去的口子，
    // 值得"显式声明才开"。
    let fetch_polyfill_script = r#"
        (function() {
            if (typeof globalThis.AbortSignal === 'undefined') {
                class AbortSignal {
                    constructor() {
                        this.aborted = false;
                        this.reason = undefined;
                        this._listeners = [];
                        this.onabort = null;
                    }
                    addEventListener(type, listener) {
                        if (type === 'abort' && typeof listener === 'function') {
                            if (this.aborted) {
                                try { listener({ type: 'abort', target: this }); } catch (_) {}
                            } else {
                                this._listeners.push(listener);
                            }
                        }
                    }
                    removeEventListener(type, listener) {
                        if (type === 'abort') {
                            this._listeners = this._listeners.filter(l => l !== listener);
                        }
                    }
                    dispatchEvent(event) {
                        if (event && event.type === 'abort') {
                            for (const l of this._listeners) {
                                try { l(event); } catch (_) {}
                            }
                            if (typeof this.onabort === 'function') {
                                try { this.onabort(event); } catch (_) {}
                            }
                        }
                        return true;
                    }
                    throwIfAborted() {
                        if (this.aborted) {
                            throw this.reason || new Error("This operation was aborted");
                        }
                    }
                    static abort(reason) {
                        const sig = new AbortSignal();
                        sig.aborted = true;
                        sig.reason = reason || new Error("This operation was aborted");
                        return sig;
                    }
                    static timeout(delayMs) {
                        const sig = new AbortSignal();
                        const timer = setTimeout(() => {
                            if (!sig.aborted) {
                                sig.aborted = true;
                                const err = new Error("The operation timed out");
                                err.name = "TimeoutError";
                                sig.reason = err;
                                sig.dispatchEvent({ type: 'abort', target: sig });
                            }
                        }, Math.max(0, Number(delayMs) || 0));
                        if (typeof timer === 'object' && timer && typeof timer.unref === 'function') {
                            timer.unref();
                        }
                        return sig;
                    }
                    static any(signals) {
                        const composite = new AbortSignal();
                        const sigList = Array.isArray(signals) ? signals : Array.from(signals || []);
                        for (const s of sigList) {
                            if (!s) continue;
                            if (s.aborted) {
                                composite.aborted = true;
                                composite.reason = s.reason;
                                return composite;
                            }
                        }
                        const onAbort = (e) => {
                            if (!composite.aborted) {
                                composite.aborted = true;
                                composite.reason = (e && e.target && e.target.reason) || (e && e.reason) || new Error("The operation was aborted");
                                composite.dispatchEvent({ type: 'abort', target: composite });
                            }
                        };
                        for (const s of sigList) {
                            if (s && typeof s.addEventListener === 'function') {
                                s.addEventListener('abort', onAbort);
                            }
                        }
                        return composite;
                    }
                }
                globalThis.AbortSignal = AbortSignal;
            }

            if (typeof globalThis.AbortController === 'undefined') {
                class AbortController {
                    constructor() {
                        this.signal = new AbortSignal();
                    }
                    abort(reason) {
                        if (!this.signal.aborted) {
                            this.signal.aborted = true;
                            this.signal.reason = reason || new Error("This operation was aborted");
                            this.signal.dispatchEvent({ type: 'abort', target: this.signal });
                        }
                    }
                }
                globalThis.AbortController = AbortController;
            }

            if (typeof globalThis.Response === 'undefined') {
                class Response {
                    constructor(payload) {
                        this.status = payload.status || 200;
                        this.statusText = payload.statusText || '';
                        this.ok = payload.ok !== undefined ? payload.ok : (this.status >= 200 && this.status < 300);
                        this._body = payload.body || '';
                        this._headers = payload.headers || {};
                    }
                    get headers() {
                        const h = this._headers;
                        return {
                            get: function(name) {
                                return h[String(name).toLowerCase()] || null;
                            },
                            has: function(name) {
                                return String(name).toLowerCase() in h;
                            }
                        };
                    }
                    async text() {
                        return this._body;
                    }
                    async json() {
                        return JSON.parse(this._body);
                    }
                }
                globalThis.Response = Response;
            }

            if (typeof globalThis.fetch === 'undefined') {
                globalThis.fetch = async function(url, options) {
                    if (typeof __native_fetch_async !== 'function') {
                        throw new Error("微内核尚未注册 __native_fetch_async 原生网络桥接");
                    }
                    const opts = options || {};
                    if (opts.signal && opts.signal.aborted) {
                        throw opts.signal.reason || new Error("The user aborted a request.");
                    }
                    const serializedOpts = JSON.stringify({
                        method: opts.method || 'GET',
                        headers: opts.headers || {},
                        body: opts.body ? String(opts.body) : null,
                        proxy: opts.proxy ? String(opts.proxy) : null
                    });

                    const fetchPromise = (async () => {
                        const raw = await __native_fetch_async(String(url), serializedOpts);
                        const parsed = typeof raw === 'string' ? JSON.parse(raw) : raw;
                        return new Response(parsed);
                    })();

                    if (opts.signal) {
                        const abortPromise = new Promise((_, reject) => {
                            opts.signal.addEventListener('abort', () => {
                                reject(opts.signal.reason || new Error("The user aborted a request."));
                            });
                        });
                        return await Promise.race([fetchPromise, abortPromise]);
                    }

                    return await fetchPromise;
                };
            }
        })();
    "#;
    if needs.net {
        ctx.eval(Source::from_bytes(fetch_polyfill_script))
            .map_err(|e| format!("注入 Web API 失败: {e}"))?;
    }

    // 13. 挂载 require 虚拟模块加载器
    //
    // 🔴 **必须容忍"某个能力没注入"**（SP-13）：老实现直接在对象字面量里取
    // `globalThis.fs.promises`，于是 `needs.fs == false` 时**注入期就崩**
    // （`cannot convert 'null' or 'undefined' to object`）——也就是说在"按声明注入"
    // 真正用起来之前，这条路径从没被缺省过。现在只有存在的全局才登记，
    // 未声明的模块 `require` 时给一条**说清原因**的错误（而不是 undefined 或崩溃）。
    let require_loader_script = r#"
        (function() {
            const modules = {};
            const put = (name, value) => {
                if (value !== undefined && value !== null) modules[name] = value;
            };
            const putAlias = (name, value) => {
                put(name, value);
                put('node:' + name, value);
            };

            putAlias('path', globalThis.path);
            if (globalThis.fs) {
                putAlias('fs', globalThis.fs);
                if (globalThis.fs.promises) putAlias('fs/promises', globalThis.fs.promises);
            }
            if (globalThis.sqlite) putAlias('sqlite', globalThis.sqlite);
            if (globalThis.childProcess) putAlias('child_process', globalThis.childProcess);
            putAlias('os', globalThis.os);
            putAlias('events', { EventEmitter: globalThis.EventEmitter, default: { EventEmitter: globalThis.EventEmitter } });
            putAlias('buffer', { Buffer: globalThis.Buffer, default: { Buffer: globalThis.Buffer } });
            putAlias('process', globalThis.process);
            putAlias('crypto', globalThis.crypto);
            putAlias('url', globalThis.url);
            putAlias('util', globalThis.util);
            putAlias('assert', globalThis.assert);
            if (globalThis.timers) {
                putAlias('timers', globalThis.timers);
                if (globalThis.timers.promises) putAlias('timers/promises', globalThis.timers.promises);
            }

            globalThis.require = function(modName) {
                if (Object.prototype.hasOwnProperty.call(modules, modName)) {
                    return modules[modName];
                }
                throw new Error(
                    "微内核沙箱里没有模块: '" + modName + "'" +
                    "（该能力未注入——插件需要在 needs 里声明它，见 docs/plugins/README.md §1 的包形态与发现规则）"
                );
            };

            if (typeof globalThis.module === 'undefined') {
                globalThis.module = { exports: {} };
                globalThis.exports = globalThis.module.exports;
            }
        })();
    "#;
    ctx.eval(Source::from_bytes(require_loader_script))
        .map_err(|e| format!("注入 require 虚拟模块加载器失败: {e}"))?;

    Ok(())
}

// 保持历史兼容别名
pub fn inject_p0_environment(ctx: &mut Context, workspace: Option<&Path>) -> Result<(), String> {
    inject_node_environment(ctx, workspace)
}
