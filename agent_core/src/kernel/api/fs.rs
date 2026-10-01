use crate::tools::sandbox::check_workspace_sandbox;
use boa_engine::{
    js_error, js_string, Context, JsValue, NativeFunction, Source,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static FS_WORKSPACE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// 设置当前事件循环线程绑定的沙箱工作区路径
pub fn set_fs_workspace(workspace: Option<&Path>) {
    FS_WORKSPACE.with(|cell| {
        *cell.borrow_mut() = workspace.map(|p| p.to_path_buf());
    });
}

fn get_safe_path(target: &str) -> Result<PathBuf, String> {
    let ws = FS_WORKSPACE.with(|cell| {
        cell.borrow().clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        })
    });
    check_workspace_sandbox(&ws, target).map_err(|e| format!("沙箱拦截: {e}"))
}

/// 向 Boa 上下文注册同步 `fs` 原生函数与模块脚本
pub fn register_fs_native_and_script(ctx: &mut Context, workspace: Option<&Path>) -> Result<(), String> {
    set_fs_workspace(workspace);

    // 1. 同步 existsSync
    let native_exists = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        match get_safe_path(&path_str) {
            Ok(safe) => Ok(JsValue::from(safe.exists())),
            Err(_) => Ok(JsValue::from(false)),
        }
    });
    ctx.register_global_callable(js_string!("__native_fs_exists_sync"), 1, native_exists).ok();

    // 2. 同步 readFileSync
    let native_read_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        let content = std::fs::read_to_string(&safe).map_err(|e| js_error!("读取失败: {}", e))?;
        Ok(JsValue::from(js_string!(content.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_fs_read_file_sync"), 2, native_read_sync).ok();

    // 3. 同步 writeFileSync
    let native_write_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let content_str = args.get(1).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        if let Some(parent) = safe.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&safe, content_str).map_err(|e| js_error!("写入失败: {}", e))?;
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("__native_fs_write_file_sync"), 2, native_write_sync).ok();

    // 4. 同步 mkdirSync
    let native_mkdir_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let recursive = args.get(1).map(|v| v.to_boolean()).unwrap_or(true);
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        if recursive {
            std::fs::create_dir_all(&safe).map_err(|e| js_error!("创建目录失败: {}", e))?;
        } else {
            std::fs::create_dir(&safe).map_err(|e| js_error!("创建目录失败: {}", e))?;
        }
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("__native_fs_mkdir_sync"), 2, native_mkdir_sync).ok();

    // 5. 同步 readdirSync
    let native_readdir_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&safe).map_err(|e| js_error!("读取目录失败: {}", e))?;
        for entry in read_dir.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
        let js_arr = boa_engine::object::builtins::JsArray::from_iter(
            entries.into_iter().map(|s| JsValue::from(js_string!(s.as_str()))),
            ctx,
        );
        Ok(JsValue::from(js_arr))
    });
    ctx.register_global_callable(js_string!("__native_fs_readdir_sync"), 1, native_readdir_sync).ok();

    // 6. 同步 statSync
    let native_stat_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        let meta = std::fs::metadata(&safe).map_err(|e| js_error!("获取元数据失败: {}", e))?;
        let size = meta.len();
        let is_file = meta.is_file();
        let is_dir = meta.is_dir();
        let mtime_ms = meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let json = serde_json::json!({
            "size": size,
            "isFile": is_file,
            "isDirectory": is_dir,
            "mtimeMs": mtime_ms,
        }).to_string();
        Ok(JsValue::from(js_string!(json.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_fs_stat_sync"), 1, native_stat_sync).ok();

    // 7. 同步 rmSync
    let native_rm_sync = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path_str = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let recursive = args.get(1).map(|v| v.to_boolean()).unwrap_or(false);
        let safe = get_safe_path(&path_str).map_err(|e| js_error!("{}", e))?;
        if safe.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&safe).map_err(|e| js_error!("删除失败: {}", e))?;
            } else {
                std::fs::remove_dir(&safe).map_err(|e| js_error!("删除失败: {}", e))?;
            }
        } else if safe.exists() {
            std::fs::remove_file(&safe).map_err(|e| js_error!("删除失败: {}", e))?;
        }
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("__native_fs_rm_sync"), 2, native_rm_sync).ok();

    // 8. 注入统一 JavaScript 封装层（同时提供同步与异步 promises 包装）
    let fs_js_wrapper = r#"
        (function() {
            function createStatObject(rawJson) {
                const s = JSON.parse(rawJson);
                return {
                    size: s.size,
                    mtimeMs: s.mtimeMs,
                    isFile: function() { return s.isFile; },
                    isDirectory: function() { return s.isDirectory; },
                };
            }

            const fsModule = {
                existsSync: function(path) {
                    return __native_fs_exists_sync(String(path));
                },
                readFileSync: function(path, encoding) {
                    return __native_fs_read_file_sync(String(path), encoding || 'utf-8');
                },
                writeFileSync: function(path, content) {
                    return __native_fs_write_file_sync(String(path), String(content));
                },
                mkdirSync: function(path, options) {
                    const recursive = typeof options === 'object' && options !== null ? Boolean(options.recursive) : Boolean(options);
                    return __native_fs_mkdir_sync(String(path), recursive);
                },
                readdirSync: function(path) {
                    return __native_fs_readdir_sync(String(path));
                },
                statSync: function(path) {
                    const raw = __native_fs_stat_sync(String(path));
                    return createStatObject(raw);
                },
                rmSync: function(path, options) {
                    const recursive = typeof options === 'object' && options !== null ? Boolean(options.recursive) : Boolean(options);
                    return __native_fs_rm_sync(String(path), recursive);
                },
                promises: {
                    readFile: async function(path, encoding) {
                        if (typeof __native_fs_read_file_async === 'function') {
                            return await __native_fs_read_file_async(String(path), encoding || 'utf-8');
                        }
                        return __native_fs_read_file_sync(String(path), encoding || 'utf-8');
                    },
                    writeFile: async function(path, content) {
                        if (typeof __native_fs_write_file_async === 'function') {
                            return await __native_fs_write_file_async(String(path), String(content));
                        }
                        return __native_fs_write_file_sync(String(path), String(content));
                    },
                    mkdir: async function(path, options) {
                        const recursive = typeof options === 'object' && options !== null ? Boolean(options.recursive) : Boolean(options);
                        if (typeof __native_fs_mkdir_async === 'function') {
                            return await __native_fs_mkdir_async(String(path), recursive);
                        }
                        return __native_fs_mkdir_sync(String(path), recursive);
                    },
                    readdir: async function(path) {
                        if (typeof __native_fs_readdir_async === 'function') {
                            return await __native_fs_readdir_async(String(path));
                        }
                        return __native_fs_readdir_sync(String(path));
                    },
                    stat: async function(path) {
                        if (typeof __native_fs_stat_async === 'function') {
                            const raw = await __native_fs_stat_async(String(path));
                            return createStatObject(raw);
                        }
                        return createStatObject(__native_fs_stat_sync(String(path)));
                    },
                    rm: async function(path, options) {
                        const recursive = typeof options === 'object' && options !== null ? Boolean(options.recursive) : Boolean(options);
                        if (typeof __native_fs_rm_async === 'function') {
                            return await __native_fs_rm_async(String(path), recursive);
                        }
                        return __native_fs_rm_sync(String(path), recursive);
                    },
                    unlink: async function(path) {
                        return await this.rm(path, { recursive: false });
                    }
                }
            };

            globalThis.fs = fsModule;
        })();
    "#;

    ctx.eval(Source::from_bytes(fs_js_wrapper))
        .map_err(|e| format!("注入 fs 封装层失败: {e}"))?;

    Ok(())
}
