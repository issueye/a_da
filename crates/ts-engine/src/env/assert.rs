/// 生成注入微内核环境的 `node:assert` / `assert` 模块 Polyfill 脚本
pub fn get_assert_polyfill_script() -> &'static str {
    r#"
    (function() {
        class AssertionError extends Error {
            constructor(options) {
                const message = (typeof options === 'string') ? options : (options && options.message);
                const actual = options && options.actual;
                const expected = options && options.expected;
                const operator = (options && options.operator) || '==';

                const defaultMsg = message || `Assertion failed: ${actual} ${operator} ${expected}`;
                super(defaultMsg);
                this.name = 'AssertionError';
                this.actual = actual;
                this.expected = expected;
                this.operator = operator;
            }
        }

        function isDeepStrictEqual(a, b) {
            if (Object.is(a, b)) return true;

            if (a === null || typeof a !== 'object' || b === null || typeof b !== 'object') {
                return false;
            }

            if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) {
                return false;
            }

            if (a instanceof Date && b instanceof Date) {
                return a.getTime() === b.getTime();
            }

            if (a instanceof RegExp && b instanceof RegExp) {
                return a.source === b.source && a.flags === b.flags;
            }

            if (typeof Buffer !== 'undefined' && Buffer.isBuffer && Buffer.isBuffer(a) && Buffer.isBuffer(b)) {
                return a.equals(b);
            }

            if (Array.isArray(a) && Array.isArray(b)) {
                if (a.length !== b.length) return false;
                for (let i = 0; i < a.length; i++) {
                    if (!isDeepStrictEqual(a[i], b[i])) return false;
                }
                return true;
            }

            if (a instanceof Set && b instanceof Set) {
                if (a.size !== b.size) return false;
                for (const item of a) {
                    if (!b.has(item)) return false;
                }
                return true;
            }

            if (a instanceof Map && b instanceof Map) {
                if (a.size !== b.size) return false;
                for (const [k, v] of a.entries()) {
                    if (!b.has(k) || !isDeepStrictEqual(v, b.get(k))) return false;
                }
                return true;
            }

            const keysA = Object.keys(a);
            const keysB = Object.keys(b);
            if (keysA.length !== keysB.length) return false;

            keysA.sort();
            keysB.sort();
            for (let i = 0; i < keysA.length; i++) {
                if (keysA[i] !== keysB[i]) return false;
            }

            for (let i = 0; i < keysA.length; i++) {
                const k = keysA[i];
                if (!isDeepStrictEqual(a[k], b[k])) return false;
            }

            return true;
        }

        function assert(value, message) {
            if (!value) {
                throw new AssertionError({
                    message: message || 'The expression evaluated to a falsy value',
                    actual: value,
                    expected: true,
                    operator: '=='
                });
            }
        }

        assert.ok = function(value, message) {
            assert(value, message);
        };

        assert.strictEqual = function(actual, expected, message) {
            if (!Object.is(actual, expected)) {
                throw new AssertionError({
                    message: message || `Expected values to be strictly equal:\n+ actual: ${actual}\n- expected: ${expected}`,
                    actual,
                    expected,
                    operator: 'strictEqual'
                });
            }
        };

        assert.notStrictEqual = function(actual, expected, message) {
            if (Object.is(actual, expected)) {
                throw new AssertionError({
                    message: message || `Expected values to not be strictly equal: ${actual}`,
                    actual,
                    expected,
                    operator: 'notStrictEqual'
                });
            }
        };

        assert.deepStrictEqual = function(actual, expected, message) {
            if (!isDeepStrictEqual(actual, expected)) {
                throw new AssertionError({
                    message: message || `Expected values to be deep strictly equal`,
                    actual,
                    expected,
                    operator: 'deepStrictEqual'
                });
            }
        };

        assert.notDeepStrictEqual = function(actual, expected, message) {
            if (isDeepStrictEqual(actual, expected)) {
                throw new AssertionError({
                    message: message || `Expected values to not be deep strictly equal`,
                    actual,
                    expected,
                    operator: 'notDeepStrictEqual'
                });
            }
        };

        assert.equal = function(actual, expected, message) {
            if (actual != expected) {
                throw new AssertionError({
                    message: message || `Expected ${actual} == ${expected}`,
                    actual,
                    expected,
                    operator: '=='
                });
            }
        };

        assert.notEqual = function(actual, expected, message) {
            if (actual == expected) {
                throw new AssertionError({
                    message: message || `Expected ${actual} != ${expected}`,
                    actual,
                    expected,
                    operator: '!='
                });
            }
        };

        assert.throws = function(fn, expected, message) {
            let threw = false;
            let caughtErr = null;
            try {
                fn();
            } catch (err) {
                threw = true;
                caughtErr = err;
            }

            if (!threw) {
                throw new AssertionError({
                    message: message || 'Missing expected exception',
                    operator: 'throws'
                });
            }

            if (expected instanceof RegExp) {
                if (!expected.test(String(caughtErr))) {
                    throw new AssertionError({
                        message: message || `The error does not match regexp ${expected}`,
                        actual: caughtErr,
                        expected,
                        operator: 'throws'
                    });
                }
            } else if (typeof expected === 'function') {
                if (expected.prototype instanceof Error || expected === Error) {
                    if (!(caughtErr instanceof expected)) {
                        throw new AssertionError({
                            message: message || `The error is not an instance of ${expected.name}`,
                            actual: caughtErr,
                            expected,
                            operator: 'throws'
                        });
                    }
                } else if (expected(caughtErr) !== true) {
                    throw new AssertionError({
                        message: message || 'The error validation function returned falsy',
                        actual: caughtErr,
                        expected,
                        operator: 'throws'
                    });
                }
            }
        };

        assert.doesNotThrow = function(fn, message) {
            try {
                fn();
            } catch (err) {
                throw new AssertionError({
                    message: message || `Got unwanted exception: ${err.message || err}`,
                    actual: err,
                    operator: 'doesNotThrow'
                });
            }
        };

        assert.fail = function(message) {
            throw new AssertionError({
                message: message || 'Failed',
                operator: 'fail'
            });
        };

        assert.match = function(string, regexp, message) {
            if (!regexp.test(String(string))) {
                throw new AssertionError({
                    message: message || `The input did not match the regular expression ${regexp}`,
                    actual: string,
                    expected: regexp,
                    operator: 'match'
                });
            }
        };

        assert.rejects = async function(asyncFn, expected, message) {
            let threw = false;
            let caughtErr = null;
            try {
                const promise = typeof asyncFn === 'function' ? asyncFn() : asyncFn;
                await promise;
            } catch (err) {
                threw = true;
                caughtErr = err;
            }

            if (!threw) {
                throw new AssertionError({
                    message: message || 'Missing expected rejection',
                    operator: 'rejects'
                });
            }
        };

        assert.AssertionError = AssertionError;

        globalThis.assert = assert;
    })();
    "#
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    #[test]
    fn test_assert_polyfill_in_boa() {
        let mut ctx = Context::default();
        let script = get_assert_polyfill_script();
        ctx.eval(Source::from_bytes(script)).expect("注入 assert polyfill 失败");

        // 1. 验证 assert.ok 与 assert.strictEqual
        let res_ok = ctx.eval(Source::from_bytes(r#"
            assert.ok(true);
            assert.strictEqual(1 + 1, 2);
            assert.strictEqual("antigravity", "antigravity");
            "OK";
        "#)).expect("eval assert.strictEqual 失败");
        assert_eq!(res_ok.to_string(&mut ctx).unwrap().to_std_string_escaped(), "OK");

        // 2. 验证 assert.deepStrictEqual
        let res_deep = ctx.eval(Source::from_bytes(r#"
            const obj1 = { a: 1, b: [2, 3], c: { d: "hello" } };
            const obj2 = { a: 1, b: [2, 3], c: { d: "hello" } };
            assert.deepStrictEqual(obj1, obj2);

            let throwsCaptured = false;
            try {
                assert.deepStrictEqual(obj1, { a: 1, b: [2, 4] });
            } catch (e) {
                throwsCaptured = (e.name === 'AssertionError');
            }
            throwsCaptured;
        "#)).expect("eval assert.deepStrictEqual 失败");
        assert_eq!(res_deep.to_boolean(), true);

        // 3. 验证 assert.throws
        let res_throws = ctx.eval(Source::from_bytes(r#"
            assert.throws(() => {
                throw new TypeError("bad input");
            }, TypeError);
            "THROWS_OK";
        "#)).expect("eval assert.throws 失败");
        assert_eq!(res_throws.to_string(&mut ctx).unwrap().to_std_string_escaped(), "THROWS_OK");
    }
}
