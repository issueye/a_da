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
            sqlite: false,
            exec: false,
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

/// SQLite：`node:sqlite` 的同步子集真的能建表、读写、命名参数、BLOB 与只读打开。
///
/// 这条用例同时钉住三件事：能力位确实注入了；数据库文件落在工作区内；
/// 读出来的 BLOB 回到了 Buffer（而不是 base64 字符串）。
#[test]
fn test_sqlite_database_sync_round_trip_and_sandbox() {
    let temp_dir = std::env::temp_dir().join(format!("a_da_sqlite_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let mut ctx = Context::default();
    inject_environment(
        &mut ctx,
        Some(&temp_dir),
        Needs {
            fs_read: true,
            fs_write: true,
            net: false,
            sqlite: true,
            exec: false,
        },
    )
    .expect("注入 SQLite 环境失败");

    let result = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                const { DatabaseSync } = require('node:sqlite');
                const db = new DatabaseSync('sqlite-test.db');
                db.exec('CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, data BLOB)');
                const first = db.prepare('INSERT INTO t(name, data) VALUES (?, ?)').run('alpha', Buffer.from('hi'));
                const second = db.prepare('INSERT INTO t(name) VALUES (:name)').run({ name: 'beta' });
                const row = db.prepare('SELECT id, name, data FROM t WHERE name = ?').get('alpha');
                const rows = db.prepare('SELECT name FROM t ORDER BY id').all();
                const wasOpen = db.isOpen;
                db.close();
                return JSON.stringify({
                    changes: first.changes,
                    secondId: second.lastInsertRowid,
                    id: row.id,
                    name: row.name,
                    blob: row.data.toString(),
                    rows: rows.map(function(r) { return r.name; }),
                    wasOpen: wasOpen,
                    isOpen: db.isOpen,
                });
            })();
            "#,
        ))
        .expect("SQLite 脚本执行失败");
    let text = result.to_string(&mut ctx).unwrap().to_std_string_escaped();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["changes"], 1, "{parsed}");
    assert_eq!(parsed["secondId"], 2, "{parsed}");
    assert_eq!(parsed["id"], 1, "{parsed}");
    assert_eq!(parsed["name"], "alpha", "{parsed}");
    assert_eq!(parsed["blob"], "hi", "BLOB 应回到 Buffer：{parsed}");
    assert_eq!(parsed["rows"], serde_json::json!(["alpha", "beta"]), "{parsed}");
    assert_eq!(parsed["wasOpen"], true);
    assert_eq!(parsed["isOpen"], false);

    // 只读打开：写入必须被 SQLite 拒绝，读取仍然可用。
    let read_only = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                const { DatabaseSync } = require('node:sqlite');
                const db = new DatabaseSync('sqlite-test.db', { readOnly: true });
                let blocked = false;
                try {
                    db.exec("INSERT INTO t(name) VALUES ('blocked')");
                } catch (e) {
                    blocked = true;
                }
                const count = db.prepare('SELECT COUNT(*) AS n FROM t').get().n;
                db.close();
                return JSON.stringify({ blocked: blocked, count: count });
            })();
            "#,
        ))
        .expect("SQLite 只读脚本执行失败");
    let read_only_text = read_only
        .to_string(&mut ctx)
        .unwrap()
        .to_std_string_escaped();
    let read_only: serde_json::Value = serde_json::from_str(&read_only_text).unwrap();
    assert_eq!(read_only["blocked"], true, "{read_only}");
    assert_eq!(read_only["count"], 2, "{read_only}");

    // :memory: 与工作区沙箱：内存库可用，越界路径必须被拦。
    let sandbox = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                const { DatabaseSync } = require('node:sqlite');
                const mem = new DatabaseSync(':memory:');
                mem.exec('CREATE TABLE x(a)');
                mem.prepare('INSERT INTO x VALUES (?)').run(7);
                const value = mem.prepare('SELECT a FROM x').get().a;
                mem.close();
                let blocked = false;
                try {
                    new DatabaseSync('../outside.db');
                } catch (e) {
                    blocked = String((e && e.message) || e).indexOf('沙箱') >= 0;
                }
                return JSON.stringify({ value: value, blocked: blocked });
            })();
            "#,
        ))
        .expect("SQLite 内存/沙箱脚本执行失败");
    let sandbox_text = sandbox.to_string(&mut ctx).unwrap().to_std_string_escaped();
    let sandbox: serde_json::Value = serde_json::from_str(&sandbox_text).unwrap();
    assert_eq!(sandbox["value"], 7, "{sandbox}");
    assert_eq!(sandbox["blocked"], true, "越界数据库路径必须被沙箱拦下：{sandbox}");

    let _ = std::fs::remove_dir_all(temp_dir);
}

/// 未声明 `sqlite` 时连模块都不存在（不是"存在但调用被拦"）。
#[test]
fn test_sqlite_is_not_injected_without_the_need() {
    let mut ctx = Context::default();
    inject_environment(&mut ctx, None, Needs::plugin_default()).expect("注入失败");
    let result = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                try {
                    require('node:sqlite');
                    return 'injected';
                } catch (e) {
                    return String((e && e.message) || e);
                }
            })();
            "#,
        ))
        .expect("检查 sqlite 门禁失败");
    let message = result.to_string(&mut ctx).unwrap().to_std_string_escaped();
    assert!(
        message.contains("未注入") || message.contains("needs"),
        "未声明 sqlite 时必须给出说清原因的错误：{message}"
    );
}

/// SQL 也能打开文件：`ATTACH` 与 `VACUUM INTO` 必须和主库路径走同一条沙箱。
#[test]
fn test_sqlite_attach_and_vacuum_into_cannot_escape_the_workspace() {
    let temp_dir = std::env::temp_dir().join(format!("a_da_sqlite_guard_{}", uuid::Uuid::new_v4()));
    let workspace = temp_dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let mut ctx = Context::default();
    inject_environment(
        &mut ctx,
        Some(&workspace),
        Needs {
            fs_read: true,
            fs_write: true,
            net: false,
            sqlite: true,
            exec: false,
        },
    )
    .expect("注入 SQLite 环境失败");

    let result = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                const { DatabaseSync } = require('node:sqlite');
                const db = new DatabaseSync('guard.db');
                db.exec('CREATE TABLE t(a)');
                let attachBlocked = false;
                try {
                    db.exec("ATTACH DATABASE '../outside-attach.db' AS outside");
                } catch (e) {
                    attachBlocked = true;
                }
                let attachMemory = false;
                try {
                    db.exec("ATTACH DATABASE ':memory:' AS mem2");
                    attachMemory = true;
                } catch (e) {}
                let attachInside = false;
                let attachInsideError = '';
                try {
                    const pathMod = require('node:path');
                    const insidePath = pathMod.join(process.cwd(), 'attached.db').replace(/\\/g, '/');
                    db.exec("ATTACH DATABASE '" + insidePath.replace(/'/g, "''") + "' AS inside");
                    db.exec("CREATE TABLE inside.t(x)");
                    attachInside = true;
                } catch (e) {
                    attachInsideError = String((e && e.message) || e);
                }
                let vacuumBlocked = false;
                try {
                    db.exec("VACUUM INTO '../outside-vacuum.db'");
                } catch (e) {
                    vacuumBlocked = true;
                }
                let lengthBlocked = false;
                try {
                    db.prepare('SELECT randomblob(9000000)').get();
                } catch (e) {
                    lengthBlocked = true;
                }
                db.close();
                return JSON.stringify({
                    attachBlocked: attachBlocked,
                    attachMemory: attachMemory,
                    attachInside: attachInside,
                    attachInsideError: attachInsideError,
                    vacuumBlocked: vacuumBlocked,
                    lengthBlocked: lengthBlocked,
                });
            })();
            "#,
        ))
        .expect("执行 SQLite 沙箱脚本失败");
    let text = result.to_string(&mut ctx).unwrap().to_std_string_escaped();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["attachBlocked"], true, "{parsed}");
    assert_eq!(parsed["attachMemory"], true, "内存库应允许 ATTACH：{parsed}");
    assert_eq!(
        parsed["attachInside"], true,
        "工作区内的库应允许 ATTACH：{parsed}"
    );
    assert_eq!(parsed["vacuumBlocked"], true, "VACUUM INTO 越界必须被拦：{parsed}");
    assert_eq!(
        parsed["lengthBlocked"], true,
        "超过 SQLITE_LIMIT_LENGTH 的 BLOB 必须被拒：{parsed}"
    );
    assert!(
        workspace.join("attached.db").exists(),
        "工作区内的 ATTACH 应该真的建出文件"
    );
    assert!(
        !temp_dir.join("outside-attach.db").exists(),
        "越界 ATTACH 不应留下文件"
    );
    assert!(
        !temp_dir.join("outside-vacuum.db").exists(),
        "越界 VACUUM INTO 不应留下文件"
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}

/// child_process 同步子集：argv、shell、input、cwd、timeout、maxBuffer 与非零退出。
#[test]
fn test_child_process_sync_surface_works() {
    let temp_dir = std::env::temp_dir().join(format!("a_da_child_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let mut ctx = Context::default();
    inject_environment(
        &mut ctx,
        Some(&temp_dir),
        Needs {
            fs_read: true,
            fs_write: true,
            net: false,
            sqlite: false,
            exec: true,
        },
    )
    .expect("注入 child_process 环境失败");

    let result = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                const cp = require('node:child_process');
                const isWin = process.platform === 'win32';
                function run(file, args, options) {
                    return cp.execFileSync(file, args, options || { encoding: 'utf8' });
                }
                const argvOut = isWin
                    ? run('cmd.exe', ['/d', '/s', '/c', 'echo child-ok'])
                    : run('/bin/echo', ['child-ok']);
                const shellOut = cp.execSync('echo shell-ok', { encoding: 'utf8' });
                const shellTrue = cp.spawnSync('echo', ['shell-true'], { shell: true, encoding: 'utf8' });
                const statusResult = cp.spawnSync(
                    isWin ? 'cmd.exe' : '/bin/sh',
                    isWin ? ['/d', '/s', '/c', 'exit 3'] : ['-c', 'exit 3'],
                    { encoding: 'utf8' },
                );
                const inputOut = isWin
                    ? cp.execFileSync('cmd.exe', ['/d', '/s', '/c', 'more'], { input: 'from-stdin', encoding: 'utf8' })
                    : cp.execFileSync('/bin/cat', [], { input: 'from-stdin', encoding: 'utf8' });
                const cwdOut = isWin
                    ? cp.execFileSync('cmd.exe', ['/d', '/s', '/c', 'cd'], { cwd: process.cwd(), encoding: 'utf8' })
                    : cp.execFileSync('/bin/pwd', [], { cwd: process.cwd(), encoding: 'utf8' });
                const timeout = cp.spawnSync(
                    isWin ? 'cmd.exe' : '/bin/sleep',
                    isWin ? ['/d', '/s', '/c', 'ping 127.0.0.1 -n 10 > nul'] : ['10'],
                    { timeout: 100 },
                );
                let bufferError = null;
                try {
                    cp.execFileSync(
                        isWin ? 'cmd.exe' : '/bin/echo',
                        isWin ? ['/d', '/s', '/c', 'echo 1234567890'] : ['1234567890'],
                        { maxBuffer: 4, encoding: 'utf8' },
                    );
                } catch (e) {
                    bufferError = e.code || 'error';
                }
                let asyncError = null;
                try { cp.spawn('x'); } catch (e) { asyncError = String(e.message || e); }
                return JSON.stringify({
                    argvOut: argvOut,
                    shellOut: shellOut,
                    shellTrueStatus: shellTrue.status,
                    shellTrueOut: shellTrue.stdout,
                    status: statusResult.status,
                    inputOut: inputOut,
                    cwdOut: cwdOut,
                    timeoutCode: timeout.error ? timeout.error.code : null,
                    timeoutStatus: timeout.status,
                    bufferError: bufferError,
                    asyncError: asyncError,
                });
            })();
            "#,
        ))
        .expect("child_process 脚本执行失败");
    let text = result.to_string(&mut ctx).unwrap().to_std_string_escaped();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        parsed["argvOut"].as_str().unwrap().contains("child-ok"),
        "{parsed}"
    );
    assert!(
        parsed["shellOut"].as_str().unwrap().contains("shell-ok"),
        "{parsed}"
    );
    assert_eq!(parsed["shellTrueStatus"], 0, "{parsed}");
    assert!(
        parsed["shellTrueOut"]
            .as_str()
            .unwrap()
            .contains("shell-true"),
        "{parsed}"
    );
    assert_eq!(parsed["status"], 3, "{parsed}");
    assert!(
        parsed["inputOut"].as_str().unwrap().contains("from-stdin"),
        "{parsed}"
    );
    assert!(
        parsed["cwdOut"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains(&temp_dir.to_string_lossy().to_ascii_lowercase()),
        "cwd 选项应生效：{parsed}"
    );
    assert_eq!(parsed["timeoutCode"], "ETIMEDOUT", "{parsed}");
    assert_eq!(parsed["timeoutStatus"], serde_json::Value::Null, "{parsed}");
    assert_eq!(parsed["bufferError"], "ENOBUFS", "{parsed}");
    assert!(
        parsed["asyncError"]
            .as_str()
            .unwrap()
            .contains("spawnSync"),
        "异步 spawn 应给出可用替代：{parsed}"
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}

/// 未声明 exec 时模块不存在。
#[test]
fn test_child_process_is_not_injected_without_the_need() {
    let mut ctx = Context::default();
    inject_environment(&mut ctx, None, Needs::plugin_default()).expect("注入失败");
    let result = ctx
        .eval(Source::from_bytes(
            r#"
            (function() {
                try {
                    require('node:child_process');
                    return 'injected';
                } catch (e) {
                    return String((e && e.message) || e);
                }
            })();
            "#,
        ))
        .expect("检查 child_process 门禁失败");
    let message = result.to_string(&mut ctx).unwrap().to_std_string_escaped();
    assert!(
        message.contains("未注入") || message.contains("needs"),
        "未声明 exec 时必须给出说清原因的错误：{message}"
    );
}
