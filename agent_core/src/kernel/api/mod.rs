pub mod buffer;
pub mod events;
pub mod path;
pub mod process;

use boa_engine::{Context, Source};
use std::path::Path;

/// 向 Boa 执行上下文全量注入 P0 基础运行底座（process, path, Buffer, EventEmitter, require）
pub fn inject_p0_environment(ctx: &mut Context, workspace: Option<&Path>) -> Result<(), String> {
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

    // 5. 挂载 require 虚拟模块加载器
    let require_loader_script = r#"
        (function() {
            const modules = {
                'path': globalThis.path,
                'node:path': globalThis.path,
                'events': { EventEmitter: globalThis.EventEmitter, default: { EventEmitter: globalThis.EventEmitter } },
                'node:events': { EventEmitter: globalThis.EventEmitter, default: { EventEmitter: globalThis.EventEmitter } },
                'buffer': { Buffer: globalThis.Buffer, default: { Buffer: globalThis.Buffer } },
                'node:buffer': { Buffer: globalThis.Buffer, default: { Buffer: globalThis.Buffer } },
                'process': globalThis.process,
                'node:process': globalThis.process,
            };

            globalThis.require = function(modName) {
                if (modules[modName]) {
                    return modules[modName];
                }
                throw new Error("微内核沙箱找不到模块: '" + modName + "'");
            };
        })();
    "#;
    ctx.eval(Source::from_bytes(require_loader_script))
        .map_err(|e| format!("注入 require 虚拟模块加载器失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_p0_environment_in_boa() {
        let mut ctx = Context::default();
        let ws = Path::new("E:/codes/a_da");
        inject_p0_environment(&mut ctx, Some(ws)).expect("注入 P0 失败");

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
    }
}
