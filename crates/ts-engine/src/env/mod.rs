pub mod assert;
pub mod buffer;
pub mod crypto;
pub mod events;
pub mod fs;
pub mod os;
pub mod path;
pub mod process;
pub mod timers;
pub mod url;
pub mod util;

use boa_engine::{Context, Source};
use std::path::Path;

/// 向 Boa 执行上下文全量注入 Node API 基础运行环境 (process, path, Buffer, EventEmitter, os, fs, crypto, url, util, assert, timers, require)
pub fn inject_node_environment(ctx: &mut Context, workspace: Option<&Path>) -> Result<(), String> {
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

    // 6. 注册同步 fs 原生函数与封装脚本
    fs::register_fs_native_and_script(ctx, workspace)?;

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
    ctx.eval(Source::from_bytes(fetch_polyfill_script))
        .map_err(|e| format!("注入 Web API 失败: {e}"))?;

    // 13. 挂载 require 虚拟模块加载器
    let require_loader_script = r#"
        (function() {
            const modules = {
                'path': globalThis.path,
                'node:path': globalThis.path,
                'fs': globalThis.fs,
                'node:fs': globalThis.fs,
                'fs/promises': globalThis.fs.promises,
                'node:fs/promises': globalThis.fs.promises,
                'os': globalThis.os,
                'node:os': globalThis.os,
                'events': { EventEmitter: globalThis.EventEmitter, default: { EventEmitter: globalThis.EventEmitter } },
                'node:events': { EventEmitter: globalThis.EventEmitter, default: { EventEmitter: globalThis.EventEmitter } },
                'buffer': { Buffer: globalThis.Buffer, default: { Buffer: globalThis.Buffer } },
                'node:buffer': { Buffer: globalThis.Buffer, default: { Buffer: globalThis.Buffer } },
                'process': globalThis.process,
                'node:process': globalThis.process,
                'crypto': globalThis.crypto,
                'node:crypto': globalThis.crypto,
                'url': globalThis.url,
                'node:url': globalThis.url,
                'util': globalThis.util,
                'node:util': globalThis.util,
                'assert': globalThis.assert,
                'node:assert': globalThis.assert,
                'timers': globalThis.timers,
                'node:timers': globalThis.timers,
                'timers/promises': globalThis.timers.promises,
                'node:timers/promises': globalThis.timers.promises,
            };

            globalThis.require = function(modName) {
                if (modules[modName]) {
                    return modules[modName];
                }
                throw new Error("微内核沙箱找不到模块: '" + modName + "'");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_p0_environment_in_boa() {
        let mut ctx = Context::default();
        let ws = Path::new("E:/codes/a_da");
        inject_p0_environment(&mut ctx, Some(ws)).expect("注入环境失败");

        // 验证 process
        let res_cwd = ctx.eval(Source::from_bytes("process.cwd()")).expect("eval cwd 失败");
        assert!(res_cwd.to_string(&mut ctx).unwrap().to_std_string_escaped().contains("E:/codes/a_da"));

        // 验证 path
        let res_join = ctx.eval(Source::from_bytes("path.join('a', 'b', 'c.txt')")).expect("eval path.join 失败");
        let join_str = res_join.to_string(&mut ctx).unwrap().to_std_string_escaped();
        assert!(join_str.contains("a") && join_str.contains("b") && join_str.contains("c.txt"));

        // 验证 Buffer
        let res_buf = ctx.eval(Source::from_bytes("Buffer.from('hello').toString('utf-8')")).expect("eval Buffer 失败");
        assert_eq!(res_buf.to_string(&mut ctx).unwrap().to_std_string_escaped(), "hello");

        // 验证 EventEmitter
        let res_ee = ctx.eval(Source::from_bytes(r#"
            const ee = new EventEmitter();
            let count = 0;
            ee.on('ping', (n) => { count += n; });
            ee.emit('ping', 5);
            count;
        "#)).expect("eval EventEmitter 失败");
        assert_eq!(res_ee.as_number().unwrap() as i64, 5);

        // 验证 require('node:path') 与 require('node:events')
        let res_req = ctx.eval(Source::from_bytes(r#"
            const p = require('node:path');
            const { EventEmitter: EE } = require('node:events');
            p.extname('foo.ts') + '|' + (typeof EE);
        "#)).expect("eval require 失败");
        assert_eq!(res_req.to_string(&mut ctx).unwrap().to_std_string_escaped(), ".ts|function");

        // 验证 os
        let res_os = ctx.eval(Source::from_bytes(r#"
            const os = require('node:os');
            os.platform() !== '';
        "#)).expect("eval os 失败");
        assert_eq!(res_os.to_boolean(), true);

        // 验证 fs 与沙箱防御
        let temp_dir = std::env::temp_dir().join(format!("a_da_fs_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut fs_ctx = Context::default();
        inject_node_environment(&mut fs_ctx, Some(&temp_dir)).expect("注入失败");

        let fs_test_script = r#"
            const fs = require('node:fs');
            fs.writeFileSync('sample.txt', 'Microkernel FS OK');
            const content = fs.readFileSync('sample.txt');
            const stat = fs.statSync('sample.txt');
            let sandboxBlocked = false;
            try {
                // 尝试越界访问上级敏感路径
                fs.readFileSync('../../../windows_secret.txt');
            } catch (e) {
                sandboxBlocked = true;
            }

            content + '|' + stat.isFile() + '|' + sandboxBlocked;
        "#;

        let res_fs = fs_ctx.eval(Source::from_bytes(fs_test_script)).expect("执行 fs 测试失败");
        assert_eq!(res_fs.to_string(&mut fs_ctx).unwrap().to_std_string_escaped(), "Microkernel FS OK|true|true");

        // 验证 crypto 与 node:crypto 虚拟模块
        let res_crypto = ctx.eval(Source::from_bytes(r#"
            const crypto1 = require('node:crypto');
            const crypto2 = require('crypto');
            const h1 = crypto1.createHash('sha256').update('hello').update(' world').digest('hex');
            const h2 = crypto2.createHash('md5').update('hello world').digest('hex');
            const h3 = crypto1.createHash('sha1').update('hello world').digest('hex');
            h1 + '|' + h2 + '|' + h3;
        "#)).expect("eval crypto 失败");
        assert_eq!(
            res_crypto.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9|5eb63bbbe01eeed093cb22bb8f5acdc3|2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );

        // 验证 url, util, assert, timers 模块挂载与 require('node:...')
        let res_extended = ctx.eval(Source::from_bytes(r#"
            const urlMod = require('node:url');
            const utilMod = require('node:util');
            const assertMod = require('node:assert');
            const timersMod = require('node:timers');

            const formatted = utilMod.format("num: %d", 100);
            assertMod.strictEqual(formatted, "num: 100");
            const testUrl = new urlMod.URL("https://a-da.dev/test?k=v");
            const hasSetTimeout = typeof timersMod.setTimeout === 'function';

            formatted + '|' + testUrl.searchParams.get('k') + '|' + hasSetTimeout;
        "#)).expect("eval extended node apis 失败");
        assert_eq!(
            res_extended.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "num: 100|v|true"
        );

        // 验证全局 fetch, Response, AbortController, AbortSignal 是否成功挂载
        let res_web = ctx.eval(Source::from_bytes(r#"
            (typeof fetch === 'function') &&
            (typeof Response === 'function') &&
            (typeof AbortController === 'function') &&
            (typeof AbortSignal === 'function') &&
            (typeof AbortSignal.timeout === 'function') &&
            (typeof AbortSignal.abort === 'function') &&
            (typeof AbortSignal.any === 'function') &&
            (AbortSignal.timeout(100) instanceof AbortSignal) &&
            (AbortSignal.any([]) instanceof AbortSignal)
        "#)).expect("eval web api 失败");
        assert_eq!(res_web.to_boolean(), true);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
