/// 生成注入微内核环境的 `node:util` / `util` 模块 Polyfill 脚本
pub fn get_util_polyfill_script() -> &'static str {
    r#"
    (function() {
        const customPromisify = Symbol('util.promisify.custom');
        const customInspect = Symbol('nodejs.util.inspect.custom');

        function inspect(obj, opts) {
            const options = Object.assign({
                depth: 2,
                colors: false,
                maxArrayLength: 100
            }, opts);

            function formatValue(val, currentDepth) {
                if (val === null) return 'null';
                if (val === undefined) return 'undefined';

                const type = typeof val;
                if (type === 'string') return JSON.stringify(val);
                if (type === 'number' || type === 'boolean') return String(val);
                if (type === 'bigint') return val.toString() + 'n';
                if (type === 'symbol') return val.toString();
                if (type === 'function') {
                    const name = val.name ? ': ' + val.name : '';
                    return '[Function' + name + ']';
                }

                if (typeof val[customInspect] === 'function') {
                    return val[customInspect](currentDepth, options);
                }

                if (val instanceof Date) return val.toISOString();
                if (val instanceof RegExp) return val.toString();
                if (val instanceof Error) return val.stack || `${val.name}: ${val.message}`;

                if (currentDepth > options.depth) {
                    return Array.isArray(val) ? '[Array]' : '[Object]';
                }

                if (Array.isArray(val)) {
                    if (val.length === 0) return '[]';
                    const items = val.slice(0, options.maxArrayLength).map(item => formatValue(item, currentDepth + 1));
                    if (val.length > options.maxArrayLength) {
                        items.push(`... ${val.length - options.maxArrayLength} more items`);
                    }
                    return '[ ' + items.join(', ') + ' ]';
                }

                if (val instanceof Set) {
                    const items = Array.from(val).map(item => formatValue(item, currentDepth + 1));
                    return 'Set(' + val.size + ') { ' + items.join(', ') + ' }';
                }

                if (val instanceof Map) {
                    const items = [];
                    for (const [k, v] of val.entries()) {
                        items.push(formatValue(k, currentDepth + 1) + ' => ' + formatValue(v, currentDepth + 1));
                    }
                    return 'Map(' + val.size + ') { ' + items.join(', ') + ' }';
                }

                // 普通对象
                const keys = Object.keys(val);
                if (keys.length === 0) return '{}';
                const entries = keys.map(k => {
                    return k + ': ' + formatValue(val[k], currentDepth + 1);
                });
                return '{ ' + entries.join(', ') + ' }';
            }

            return formatValue(obj, 0);
        }
        inspect.custom = customInspect;

        function format(f, ...args) {
            if (typeof f !== 'string') {
                const parts = [];
                if (f !== undefined) parts.push(inspect(f));
                for (let i = 0; i < args.length; i++) {
                    parts.push(inspect(args[i]));
                }
                return parts.join(' ');
            }

            let argIndex = 0;
            let str = '';
            let lastPos = 0;

            for (let i = 0; i < f.length; i++) {
                if (f.charCodeAt(i) === 37 /* % */ && i + 1 < f.length) {
                    const spec = f.charAt(i + 1);
                    let replacement = null;

                    if (spec === 's') {
                        const a = args[argIndex++];
                        replacement = a !== undefined ? String(a) : '%s';
                    } else if (spec === 'd') {
                        const a = args[argIndex++];
                        replacement = a !== undefined ? String(Number(a)) : '%d';
                    } else if (spec === 'i') {
                        const a = args[argIndex++];
                        replacement = a !== undefined ? String(parseInt(a, 10)) : '%i';
                    } else if (spec === 'f') {
                        const a = args[argIndex++];
                        replacement = a !== undefined ? String(parseFloat(a)) : '%f';
                    } else if (spec === 'j') {
                        const a = args[argIndex++];
                        try {
                            replacement = a !== undefined ? JSON.stringify(a) : '%j';
                        } catch (_) {
                            replacement = '[Circular]';
                        }
                    } else if (spec === 'o' || spec === 'O') {
                        const a = args[argIndex++];
                        replacement = a !== undefined ? inspect(a) : '%' + spec;
                    } else if (spec === '%') {
                        replacement = '%';
                    }

                    if (replacement !== null) {
                        str += f.slice(lastPos, i) + replacement;
                        i++; // 跳过占位字符
                        lastPos = i + 1;
                    }
                }
            }

            str += f.slice(lastPos);

            // 追加剩余的多余参数
            while (argIndex < args.length) {
                const extra = args[argIndex++];
                if (typeof extra !== 'object' || extra === null) {
                    str += ' ' + extra;
                } else {
                    str += ' ' + inspect(extra);
                }
            }

            return str;
        }

        function promisify(original) {
            if (typeof original !== 'function') {
                throw new TypeError('The "original" argument must be of type Function');
            }

            if (original[customPromisify]) {
                const fn = original[customPromisify];
                if (typeof fn !== 'function') {
                    throw new TypeError('The custom promisified function must be of type Function');
                }
                return fn;
            }

            function fn(...args) {
                return new Promise((resolve, reject) => {
                    try {
                        original.call(this, ...args, (err, ...values) => {
                            if (err) {
                                reject(err);
                            } else {
                                resolve(values.length === 1 ? values[0] : (values.length === 0 ? undefined : values));
                            }
                        });
                    } catch (err) {
                        reject(err);
                    }
                });
            }

            Object.setPrototypeOf(fn, Object.getPrototypeOf(original));
            Object.defineProperties(fn, Object.getOwnPropertyDescriptors(original));
            return fn;
        }
        promisify.custom = customPromisify;

        const types = {
            isPromise: (val) => Boolean(val && typeof val.then === 'function'),
            isAsyncFunction: (val) => Boolean(val && val[Symbol.toStringTag] === 'AsyncFunction'),
            isDate: (val) => val instanceof Date,
            isRegExp: (val) => val instanceof RegExp,
            isNativeError: (val) => val instanceof Error,
            isMap: (val) => val instanceof Map,
            isSet: (val) => val instanceof Set,
            isArrayBuffer: (val) => typeof ArrayBuffer !== 'undefined' && val instanceof ArrayBuffer,
            isTypedArray: (val) => ArrayBuffer.isView(val) && !(val instanceof DataView),
            isUint8Array: (val) => typeof Uint8Array !== 'undefined' && val instanceof Uint8Array,
            isAnyArrayBuffer: (val) => typeof ArrayBuffer !== 'undefined' && val instanceof ArrayBuffer,
        };

        function inherits(ctor, superCtor) {
            if (ctor === undefined || ctor === null) {
                throw new TypeError('The constructor to "inherits" must not be null or undefined');
            }
            if (superCtor === undefined || superCtor === null) {
                throw new TypeError('The super constructor to "inherits" must not be null or undefined');
            }
            if (superCtor.prototype === undefined) {
                throw new TypeError('The super constructor to "inherits" must have a prototype');
            }
            ctor.super_ = superCtor;
            Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
        }

        function deprecate(fn, msg, code) {
            let warned = false;
            function deprecated(...args) {
                if (!warned) {
                    warned = true;
                    if (typeof console !== 'undefined' && console.warn) {
                        console.warn(`[DEPRECATION${code ? ' ' + code : ''}] ${msg}`);
                    }
                }
                return fn.apply(this, args);
            }
            return deprecated;
        }

        const utilModule = {
            format,
            inspect,
            promisify,
            types,
            inherits,
            deprecate,
            callbackify: function(original) {
                return function(...args) {
                    const maybeCb = args.pop();
                    if (typeof maybeCb !== 'function') {
                        throw new TypeError('The last argument must be of type Function');
                    }
                    original.apply(this, args).then(
                        res => maybeCb(null, res),
                        err => maybeCb(err)
                    );
                };
            }
        };

        globalThis.util = utilModule;
    })();
    "#
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    #[test]
    fn test_util_polyfill_in_boa() {
        let mut ctx = Context::default();
        let script = get_util_polyfill_script();
        ctx.eval(Source::from_bytes(script)).expect("注入 util polyfill 失败");

        // 1. 验证 util.format
        let res_format = ctx.eval(Source::from_bytes(r#"
            util.format("Hello %s, count: %d, json: %j, pct: %%", "World", 42, { a: 1 });
        "#)).expect("eval util.format 失败");
        assert_eq!(
            res_format.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "Hello World, count: 42, json: {\"a\":1}, pct: %"
        );

        // 2. 验证 util.promisify
        let res_promisify = ctx.eval(Source::from_bytes(r#"
            function nodeStyleFn(x, y, cb) {
                if (x < 0) cb(new Error("neg"));
                else cb(null, x + y);
            }
            const promiseFn = util.promisify(nodeStyleFn);
            let result = 0;
            promiseFn(10, 20).then(val => { result = val; });
            result;
        "#)).expect("eval util.promisify 失败");
        assert_eq!(res_promisify.as_number().unwrap() as i64, 0);

        // 运行 microtask 排空
        let _ = ctx.run_jobs();
        let check_res = ctx.eval(Source::from_bytes("result")).unwrap();
        assert_eq!(check_res.as_number().unwrap() as i64, 30);

        // 3. 验证 util.types
        let res_types = ctx.eval(Source::from_bytes(r#"
            const isP = util.types.isPromise(Promise.resolve(1));
            const isD = util.types.isDate(new Date());
            const isR = util.types.isRegExp(/abc/);
            isP + '|' + isD + '|' + isR;
        "#)).expect("eval util.types 失败");
        assert_eq!(res_types.to_string(&mut ctx).unwrap().to_std_string_escaped(), "true|true|true");
    }
}
