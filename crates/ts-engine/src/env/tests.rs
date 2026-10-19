use super::*;
use boa_engine::{Context, Source};
use std::path::Path;

/// SP-13：**未声明即不注入**。
///
/// 判据是"API 在不在"（`typeof`），不是"调用会不会被拦"——不注入的能力连那行代码
/// 都写不出来，绕过的路子（换个别名、直呼原生）自然也就没有。
#[test]
fn test_needs_gate_what_gets_injected() {
    fn eval_str(ctx: &mut Context, expr: &str) -> String {
        ctx.eval(Source::from_bytes(expr))
            .unwrap_or_else(|e| panic!("eval `{expr}` 失败: {e}"))
            .to_string(ctx)
            .unwrap()
            .to_std_string_escaped()
    }

    // 1. 插件默认档：网络关、fs 开
    let mut ctx = Context::default();
    inject_environment(&mut ctx, None, Needs::plugin_default()).expect("注入失败");
    assert_eq!(
        eval_str(&mut ctx, "typeof fetch"),
        "undefined",
        "默认档下 `fetch` 必须根本不存在"
    );
    assert_ne!(
        eval_str(&mut ctx, "typeof globalThis.fs"),
        "undefined",
        "fs 在默认档里是开的"
    );

    // 2. 显式关掉 fs：全局不存在，而且 `require` 要**说清原因**地报错
    let mut ctx2 = Context::default();
    inject_environment(
        &mut ctx2,
        None,
        Needs {
            fs_read: false,
            fs_write: false,
            net: false,
        },
    )
    .expect("注入失败");
    assert_eq!(
        eval_str(&mut ctx2, "typeof globalThis.fs"),
        "undefined",
        "fs 未声明时不该注入"
    );
    let msg = eval_str(
        &mut ctx2,
        "(function(){ try { require('fs'); return 'no-error' } catch (e) { return String((e && e.message) || e) } })()",
    );
    assert!(
        msg.contains("未注入") || msg.contains("needs"),
        "未声明的模块必须给出**说清原因**的错误（而不是 undefined 或崩溃）：{msg}"
    );

    // 3. 全开档：网络回来
    let mut ctx3 = Context::default();
    inject_environment(&mut ctx3, None, Needs::everything()).expect("注入失败");
    assert_eq!(
        eval_str(&mut ctx3, "typeof fetch"),
        "function",
        "声明了网络就应当注入"
    );
}

#[test]
fn test_p0_environment_in_boa() {
    let mut ctx = Context::default();
    let ws = Path::new("E:/codes/a_da");
    inject_p0_environment(&mut ctx, Some(ws)).expect("注入环境失败");

    // 验证 process
    let res_cwd = ctx
        .eval(Source::from_bytes("process.cwd()"))
        .expect("eval cwd 失败");
    assert!(
        res_cwd
            .to_string(&mut ctx)
            .unwrap()
            .to_std_string_escaped()
            .contains("E:/codes/a_da")
    );

    // 验证 path
    let res_join = ctx
        .eval(Source::from_bytes("path.join('a', 'b', 'c.txt')"))
        .expect("eval path.join 失败");
    let join_str = res_join
        .to_string(&mut ctx)
        .unwrap()
        .to_std_string_escaped();
    assert!(join_str.contains("a") && join_str.contains("b") && join_str.contains("c.txt"));

    // 验证 Buffer
    let res_buf = ctx
        .eval(Source::from_bytes("Buffer.from('hello').toString('utf-8')"))
        .expect("eval Buffer 失败");
    assert_eq!(
        res_buf.to_string(&mut ctx).unwrap().to_std_string_escaped(),
        "hello"
    );

    // 验证 EventEmitter
    let res_ee = ctx
        .eval(Source::from_bytes(
            r#"
            const ee = new EventEmitter();
            let count = 0;
            ee.on('ping', (n) => { count += n; });
            ee.emit('ping', 5);
            count;
        "#,
        ))
        .expect("eval EventEmitter 失败");
    assert_eq!(res_ee.as_number().unwrap() as i64, 5);

    // 验证 require('node:path') 与 require('node:events')
    let res_req = ctx
        .eval(Source::from_bytes(
            r#"
            const p = require('node:path');
            const { EventEmitter: EE } = require('node:events');
            p.extname('foo.ts') + '|' + (typeof EE);
        "#,
        ))
        .expect("eval require 失败");
    assert_eq!(
        res_req.to_string(&mut ctx).unwrap().to_std_string_escaped(),
        ".ts|function"
    );

    // 验证 os
    let res_os = ctx
        .eval(Source::from_bytes(
            r#"
            const os = require('node:os');
            os.platform() !== '';
        "#,
        ))
        .expect("eval os 失败");
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

    let res_fs = fs_ctx
        .eval(Source::from_bytes(fs_test_script))
        .expect("执行 fs 测试失败");
    assert_eq!(
        res_fs
            .to_string(&mut fs_ctx)
            .unwrap()
            .to_std_string_escaped(),
        "Microkernel FS OK|true|true"
    );

    // 验证 crypto 与 node:crypto 虚拟模块
    let res_crypto = ctx
        .eval(Source::from_bytes(
            r#"
            const crypto1 = require('node:crypto');
            const crypto2 = require('crypto');
            const h1 = crypto1.createHash('sha256').update('hello').update(' world').digest('hex');
            const h2 = crypto2.createHash('md5').update('hello world').digest('hex');
            const h3 = crypto1.createHash('sha1').update('hello world').digest('hex');
            h1 + '|' + h2 + '|' + h3;
        "#,
        ))
        .expect("eval crypto 失败");
    assert_eq!(
        res_crypto
            .to_string(&mut ctx)
            .unwrap()
            .to_std_string_escaped(),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9|5eb63bbbe01eeed093cb22bb8f5acdc3|2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
    );

    // 验证 url, util, assert, timers 模块挂载与 require('node:...')
    let res_extended = ctx
        .eval(Source::from_bytes(
            r#"
            const urlMod = require('node:url');
            const utilMod = require('node:util');
            const assertMod = require('node:assert');
            const timersMod = require('node:timers');

            const formatted = utilMod.format("num: %d", 100);
            assertMod.strictEqual(formatted, "num: 100");
            const testUrl = new urlMod.URL("https://a-da.dev/test?k=v");
            const hasSetTimeout = typeof timersMod.setTimeout === 'function';

            formatted + '|' + testUrl.searchParams.get('k') + '|' + hasSetTimeout;
        "#,
        ))
        .expect("eval extended node apis 失败");
    assert_eq!(
        res_extended
            .to_string(&mut ctx)
            .unwrap()
            .to_std_string_escaped(),
        "num: 100|v|true"
    );

    // 验证全局 fetch, Response, AbortController, AbortSignal 是否成功挂载
    let res_web = ctx
        .eval(Source::from_bytes(
            r#"
            (typeof fetch === 'function') &&
            (typeof Response === 'function') &&
            (typeof AbortController === 'function') &&
            (typeof AbortSignal === 'function') &&
            (typeof AbortSignal.timeout === 'function') &&
            (typeof AbortSignal.abort === 'function') &&
            (typeof AbortSignal.any === 'function') &&
            (AbortSignal.timeout(100) instanceof AbortSignal) &&
            (AbortSignal.any([]) instanceof AbortSignal)
        "#,
        ))
        .expect("eval web api 失败");
    assert_eq!(res_web.to_boolean(), true);

    let _ = std::fs::remove_dir_all(temp_dir);
}
