//! Hermes 引擎嵌入式宿主封装
//!
//! 通过 Microsoft Hermes C-API / Node-API 将 Meta Hermes 轻量引擎集成至 Rust 宿主，
//! 负责加载预编译 React 19 UI Bundle 并驱动 GPUI 原生窗口渲染。

use std::ffi::{c_char, c_void, CString};
use std::ptr;

// 原始指针与状态定义
pub type JsrRuntime = *mut c_void;
pub type JsrConfig = *mut c_void;
pub type NapiEnv = *mut c_void;
pub type NapiValue = *mut c_void;
pub type NapiStatus = i32;

pub const NAPI_OK: NapiStatus = 0;

#[link(name = "hermes")]
unsafe extern "C" {
    pub fn jsr_create_config(config: *mut JsrConfig) -> NapiStatus;
    pub fn jsr_delete_config(config: JsrConfig) -> NapiStatus;
    pub fn jsr_config_enable_gc_api(config: JsrConfig, value: bool) -> NapiStatus;
    pub fn jsr_create_runtime(config: JsrConfig, runtime: *mut JsrRuntime) -> NapiStatus;
    pub fn jsr_delete_runtime(runtime: JsrRuntime) -> NapiStatus;
    pub fn jsr_runtime_get_node_api_env(runtime: JsrRuntime, env: *mut NapiEnv) -> NapiStatus;

    pub fn napi_create_string_utf8(
        env: NapiEnv,
        str_: *const c_char,
        length: usize,
        result: *mut NapiValue,
    ) -> NapiStatus;

    pub fn napi_run_script(env: NapiEnv, script: NapiValue, result: *mut NapiValue) -> NapiStatus;

    pub fn napi_get_value_string_utf8(
        env: NapiEnv,
        value: NapiValue,
        buf: *mut c_char,
        bufsize: usize,
        result: *mut usize,
    ) -> NapiStatus;

    pub fn napi_coerce_to_string(env: NapiEnv, value: NapiValue, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_get_and_clear_last_exception(env: NapiEnv, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_get_last_error_info(
        env: NapiEnv,
        result: *mut *const NapiExtendedErrorInfo,
    ) -> NapiStatus;
    pub fn napi_get_global(env: NapiEnv, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_create_object(env: NapiEnv, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_set_named_property(
        env: NapiEnv,
        object: NapiValue,
        utf8name: *const c_char,
        value: NapiValue,
    ) -> NapiStatus;
    pub fn jsr_drain_microtasks(
        env: NapiEnv,
        max_count_hint: i32,
        result: *mut bool,
    ) -> NapiStatus;
    pub fn napi_create_function(
        env: NapiEnv,
        utf8name: *const c_char,
        length: usize,
        cb: NapiCallback,
        data: *mut c_void,
        result: *mut NapiValue,
    ) -> NapiStatus;
    pub fn napi_get_cb_info(
        env: NapiEnv,
        cbinfo: NapiCallbackInfo,
        argc: *mut usize,
        argv: *mut NapiValue,
        this_arg: *mut NapiValue,
        data: *mut *mut c_void,
    ) -> NapiStatus;
    pub fn napi_create_uint32(env: NapiEnv, value: u32, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_get_value_uint32(env: NapiEnv, value: NapiValue, result: *mut u32) -> NapiStatus;
    pub fn napi_get_boolean(env: NapiEnv, value: bool, result: *mut NapiValue) -> NapiStatus;
    pub fn napi_get_undefined(env: NapiEnv, result: *mut NapiValue) -> NapiStatus;
}

pub type NapiCallbackInfo = *mut c_void;
pub type NapiCallback = unsafe extern "C" fn(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue;

#[repr(C)]
pub struct NapiExtendedErrorInfo {
    pub error_message: *const c_char,
    pub engine_reserved: *mut c_void,
    pub engine_error_code: u32,
    pub error_code: NapiStatus,
}

pub struct HermesHost {
    runtime: JsrRuntime,
    env: NapiEnv,
}

unsafe impl Send for HermesHost {}
unsafe impl Sync for HermesHost {}

impl HermesHost {
    /// 初始化 Hermes 运行时
    pub fn new() -> Result<Self, String> {
        unsafe {
            let mut config: JsrConfig = ptr::null_mut();
            let status = jsr_create_config(&mut config);
            if status != NAPI_OK {
                return Err(format!("jsr_create_config 失败，状态码: {}", status));
            }

            let _ = jsr_config_enable_gc_api(config, true);

            let mut runtime: JsrRuntime = ptr::null_mut();
            let status = jsr_create_runtime(config, &mut runtime);
            let _ = jsr_delete_config(config);
            if status != NAPI_OK {
                return Err(format!("jsr_create_runtime 失败，状态码: {}", status));
            }

            let mut env: NapiEnv = ptr::null_mut();
            let status = jsr_runtime_get_node_api_env(runtime, &mut env);
            if status != NAPI_OK {
                let _ = jsr_delete_runtime(runtime);
                return Err(format!("jsr_runtime_get_node_api_env 失败，状态码: {}", status));
            }

            Ok(Self { runtime, env })
        }
    }

    /// 获取底层 Node-API env 指针
    pub fn env(&self) -> NapiEnv {
        self.env
    }

    /// 将 NapiValue 转化为 Rust String
    unsafe fn value_to_string(&self, val: NapiValue) -> String {
        unsafe {
            let mut str_val: NapiValue = ptr::null_mut();
            let status = napi_coerce_to_string(self.env, val, &mut str_val);
            if status != NAPI_OK {
                return "<coerce_failed>".to_string();
            }

            let mut len: usize = 0;
            let status = napi_get_value_string_utf8(self.env, str_val, ptr::null_mut(), 0, &mut len);
            if status != NAPI_OK {
                return "<get_length_failed>".to_string();
            }

            let mut buf = vec![0u8; len + 1];
            let mut written: usize = 0;
            let _ = napi_get_value_string_utf8(
                self.env,
                str_val,
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
                &mut written,
            );
            buf.truncate(written);
            String::from_utf8(buf).unwrap_or_else(|_| "<utf8_decode_failed>".to_string())
        }
    }

    /// 执行一段 JavaScript 代码并返回字符串结果
    pub fn eval_to_string(&self, code: &str) -> Result<String, String> {
        unsafe {
            let c_code = CString::new(code).map_err(|e| e.to_string())?;
            let mut script_val: NapiValue = ptr::null_mut();
            let status = napi_create_string_utf8(self.env, c_code.as_ptr(), code.len(), &mut script_val);
            if status != NAPI_OK {
                return Err(format!("napi_create_string_utf8 失败: {}", status));
            }

            let mut result_val: NapiValue = ptr::null_mut();
            let status = napi_run_script(self.env, script_val, &mut result_val);
            if status != NAPI_OK {
                let mut exc_val: NapiValue = ptr::null_mut();
                let _ = napi_get_and_clear_last_exception(self.env, &mut exc_val);
                let mut exc_msg = String::new();
                if !exc_val.is_null() {
                    exc_msg = self.value_to_string(exc_val);
                }

                let mut error_info_ptr: *const NapiExtendedErrorInfo = ptr::null();
                let _ = napi_get_last_error_info(self.env, &mut error_info_ptr);
                let info_msg = if !error_info_ptr.is_null() && !(*error_info_ptr).error_message.is_null() {
                    std::ffi::CStr::from_ptr((*error_info_ptr).error_message)
                        .to_string_lossy()
                        .to_string()
                } else {
                    String::new()
                };

                let err_msg = format!(
                    "执行脚本错误(status={}): exc='{}', info='{}'",
                    status, exc_msg, info_msg
                );
                return Err(err_msg);
            }

            Ok(self.value_to_string(result_val))
        }
    }

    /// 动态加载 Node-API 原生插件 (.node / DLL) 并注入为全局变量
    pub fn load_native_addon(&self, dll_path: &str, global_prop_name: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;

            unsafe extern "system" {
                fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
                fn GetProcAddress(hModule: *mut c_void, lpProcName: *const c_char) -> *mut c_void;
                fn GetLastError() -> u32;
            }

            type NapiAddonRegisterFunc = unsafe extern "C" fn(NapiEnv, NapiValue) -> NapiValue;

            let wide: Vec<u16> = OsStr::new(dll_path).encode_wide().chain(Some(0)).collect();
            let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
            if handle.is_null() {
                let err = unsafe { GetLastError() };
                return Err(format!("LoadLibraryW 无法加载插件 '{}'，错误码: {}", dll_path, err));
            }

            let symbol_name = CString::new("napi_register_module_v1").map_err(|e| e.to_string())?;
            let proc = unsafe { GetProcAddress(handle, symbol_name.as_ptr()) };
            if proc.is_null() {
                return Err(format!("在插件 '{}' 中未找到 napi_register_module_v1 符号", dll_path));
            }

            let register_fn: NapiAddonRegisterFunc = unsafe { std::mem::transmute(proc) };

            unsafe {
                let mut exports: NapiValue = ptr::null_mut();
                let status = napi_create_object(self.env, &mut exports);
                if status != NAPI_OK {
                    return Err(format!("napi_create_object 失败: {}", status));
                }

                let registered_exports = register_fn(self.env, exports);
                let final_exports = if !registered_exports.is_null() {
                    registered_exports
                } else {
                    exports
                };

                let mut global: NapiValue = ptr::null_mut();
                let status = napi_get_global(self.env, &mut global);
                if status != NAPI_OK {
                    return Err(format!("napi_get_global 失败: {}", status));
                }

                let c_prop = CString::new(global_prop_name).map_err(|e| e.to_string())?;
                let status = napi_set_named_property(self.env, global, c_prop.as_ptr(), final_exports);
                if status != NAPI_OK {
                    return Err(format!("napi_set_named_property 失败: {}", status));
                }
            }

            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err("原生插件动态加载目前仅支持 Windows".to_string())
        }
    }
}

unsafe extern "C" fn js_ws_connect(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 1;
        let mut argv: [NapiValue; 1] = [ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        if argc < 1 || argv[0].is_null() {
            let mut res: NapiValue = ptr::null_mut();
            let _ = napi_create_uint32(env, 0, &mut res);
            return res;
        }

        let mut len: usize = 0;
        let _ = napi_get_value_string_utf8(env, argv[0], ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len + 1];
        let mut written: usize = 0;
        let _ = napi_get_value_string_utf8(env, argv[0], buf.as_mut_ptr() as *mut c_char, buf.len(), &mut written);
        buf.truncate(written);
        let url = String::from_utf8(buf).unwrap_or_default();

        let id = crate::native_ws::ws_connect(&url);
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_create_uint32(env, id, &mut result);
        result
    }
}

unsafe extern "C" fn js_ws_send(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 2;
        let mut argv: [NapiValue; 2] = [ptr::null_mut(), ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        if argc < 2 {
            let mut res: NapiValue = ptr::null_mut();
            let _ = napi_get_boolean(env, false, &mut res);
            return res;
        }

        let mut id: u32 = 0;
        let _ = napi_get_value_uint32(env, argv[0], &mut id);

        let mut len: usize = 0;
        let _ = napi_get_value_string_utf8(env, argv[1], ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len + 1];
        let mut written: usize = 0;
        let _ = napi_get_value_string_utf8(env, argv[1], buf.as_mut_ptr() as *mut c_char, buf.len(), &mut written);
        buf.truncate(written);
        let text = String::from_utf8(buf).unwrap_or_default();

        let ok = crate::native_ws::ws_send(id, &text);
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_get_boolean(env, ok, &mut result);
        result
    }
}

unsafe extern "C" fn js_ws_poll(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 1;
        let mut argv: [NapiValue; 1] = [ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        let mut id: u32 = 0;
        if argc >= 1 && !argv[0].is_null() {
            let _ = napi_get_value_uint32(env, argv[0], &mut id);
        }

        let json = crate::native_ws::ws_poll(id);
        let c_json = CString::new(json).unwrap_or_default();
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_create_string_utf8(env, c_json.as_ptr(), c_json.as_bytes().len(), &mut result);
        result
    }
}

unsafe extern "C" fn js_ws_close(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 1;
        let mut argv: [NapiValue; 1] = [ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        let mut id: u32 = 0;
        if argc >= 1 && !argv[0].is_null() {
            let _ = napi_get_value_uint32(env, argv[0], &mut id);
        }

        crate::native_ws::ws_close(id);
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_get_undefined(env, &mut result);
        result
    }
}

unsafe extern "C" fn js_fs_append(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 2;
        let mut argv: [NapiValue; 2] = [ptr::null_mut(), ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        if argc >= 2 && !argv[0].is_null() && !argv[1].is_null() {
            let mut len: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[0], ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len + 1];
            let mut written: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[0], buf.as_mut_ptr() as *mut c_char, buf.len(), &mut written);
            buf.truncate(written);
            let path = String::from_utf8(buf).unwrap_or_default();

            let mut len2: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[1], ptr::null_mut(), 0, &mut len2);
            let mut buf2 = vec![0u8; len2 + 1];
            let mut written2: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[1], buf2.as_mut_ptr() as *mut c_char, buf2.len(), &mut written2);
            buf2.truncate(written2);
            let text = String::from_utf8(buf2).unwrap_or_default();

            use std::io::Write;
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(text.as_bytes());
            }
        }
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_get_undefined(env, &mut result);
        result
    }
}

unsafe extern "C" fn js_fs_exists(env: NapiEnv, info: NapiCallbackInfo) -> NapiValue {
    unsafe {
        let mut argc: usize = 1;
        let mut argv: [NapiValue; 1] = [ptr::null_mut()];
        let _ = napi_get_cb_info(env, info, &mut argc, argv.as_mut_ptr(), ptr::null_mut(), ptr::null_mut());
        let mut exists = false;
        if argc >= 1 && !argv[0].is_null() {
            let mut len: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[0], ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len + 1];
            let mut written: usize = 0;
            let _ = napi_get_value_string_utf8(env, argv[0], buf.as_mut_ptr() as *mut c_char, buf.len(), &mut written);
            buf.truncate(written);
            let path = String::from_utf8(buf).unwrap_or_default();
            exists = std::path::Path::new(&path).exists();
        }
        let mut result: NapiValue = ptr::null_mut();
        let _ = napi_get_boolean(env, exists, &mut result);
        result
    }
}

impl HermesHost {

    /// 向 Hermes 虚拟机注入轻量 Node.js 运行时环境 Shim
    pub fn inject_node_shims(
        &self,
        workspace: &str,
        host_port: u16,
        token: &str,
    ) -> Result<(), String> {
        // 挂载原生桥接函数
        unsafe {
            let mut global: NapiValue = ptr::null_mut();
            let _ = napi_get_global(self.env, &mut global);

            let funcs: &[(&str, NapiCallback)] = &[
                ("__native_ws_connect", js_ws_connect),
                ("__native_ws_send", js_ws_send),
                ("__native_ws_poll", js_ws_poll),
                ("__native_ws_close", js_ws_close),
                ("__native_fs_append", js_fs_append),
                ("__native_fs_exists", js_fs_exists),
            ];

            for &(name, cb) in funcs {
                let c_name = CString::new(name).unwrap();
                let mut fn_val: NapiValue = ptr::null_mut();
                let _ = napi_create_function(
                    self.env,
                    c_name.as_ptr(),
                    name.len(),
                    cb,
                    ptr::null_mut(),
                    &mut fn_val,
                );
                let _ = napi_set_named_property(self.env, global, c_name.as_ptr(), fn_val);
            }
        }

        let escaped_workspace = workspace.replace('\\', "\\\\").replace('"', "\\\"");
        let userprofile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into()).replace('\\', "\\\\");
        let temp = std::env::var("TEMP").unwrap_or_else(|_| "C:\\Windows\\Temp".into()).replace('\\', "\\\\");
        let current_pid = std::process::id();

        let shim_js = format!(
            r#"
            (function() {{
                var global = globalThis;

                // 0. console 基础实现
                global.console = global.console || {{}};
                var _print = typeof print === "function" ? print : function() {{}};
                ["log", "info", "warn", "error", "debug", "trace"].forEach(function(m) {{
                    if (typeof global.console[m] !== "function") {{
                        global.console[m] = function() {{
                            var args = Array.prototype.slice.call(arguments);
                            _print("[JS " + m.toUpperCase() + "] " + args.join(" "));
                        }};
                    }}
                }});

                // 1. process 基础环境
                global.process = global.process || {{}};
                global.process.platform = "win32";
                global.process.arch = "x64";
                global.process.pid = {current_pid};
                global.process.cwd = function() {{ return "{escaped_workspace}"; }};
                global.process.version = "v20.0.0";
                global.process.versions = {{ node: "20.0.0", hermes: "0.1.27" }};
                
                global.process.env = global.process.env || {{}};
                global.process.env.NODE_ENV = "production";
                global.process.env.A_DA_WS_PORT = "{host_port}";
                global.process.env.A_DA_WS_TOKEN = "{token}";
                global.process.env.USERPROFILE = "{userprofile}";
                global.process.env.TEMP = "{temp}";
                global.process.env.A_DA_TRANSPORT = "ws";

                global.__A_DA_HOST_PORT = {host_port};
                global.__A_DA_HOST_TOKEN = "{token}";

                var _procListeners = {{}};
                global.process.on = function(evt, fn) {{
                    _procListeners[evt] = _procListeners[evt] || [];
                    _procListeners[evt].push(fn);
                    return global.process;
                }};
                global.process.off = function(evt, fn) {{
                    if (_procListeners[evt]) {{
                        _procListeners[evt] = _procListeners[evt].filter(function(f) {{ return f !== fn; }});
                    }}
                    return global.process;
                }};
                global.process.emit = function(evt) {{
                    var args = Array.prototype.slice.call(arguments, 1);
                    if (_procListeners[evt]) {{
                        _procListeners[evt].forEach(function(fn) {{
                            try {{ fn.apply(null, args); }} catch(e) {{ console.error(e); }}
                        }});
                    }}
                }};
                global.process.nextTick = function(fn) {{
                    var args = Array.prototype.slice.call(arguments, 1);
                    Promise.resolve().then(function() {{ fn.apply(null, args); }});
                }};
                global.process.exit = function(code) {{
                    global.process.emit("exit", code || 0);
                }};

                // 2. 基础计时器与性能接口
                global.performance = global.performance || {{
                    now: function() {{ return Date.now(); }}
                }};
                global.queueMicrotask = global.queueMicrotask || function(cb) {{
                    Promise.resolve().then(cb);
                }};

                // 全局定时器队列与调度器
                var _timers = [];
                var _nextTimerId = 1;

                global.setTimeout = function(fn, delay) {{
                    var id = _nextTimerId++;
                    var args = Array.prototype.slice.call(arguments, 2);
                    var due = Date.now() + Math.max(0, delay || 0);
                    _timers.push({{ id: id, fn: fn, due: due, args: args, repeat: false }});
                    return id;
                }};

                global.clearTimeout = function(id) {{
                    _timers = _timers.filter(function(t) {{ return t.id !== id; }});
                }};

                global.setInterval = function(fn, delay) {{
                    var id = _nextTimerId++;
                    var args = Array.prototype.slice.call(arguments, 2);
                    var interval = Math.max(1, delay || 0);
                    var due = Date.now() + interval;
                    _timers.push({{ id: id, fn: fn, due: due, args: args, repeat: true, interval: interval }});
                    return id;
                }};

                global.clearInterval = global.clearTimeout;

                global.setImmediate = function(fn) {{
                    var args = Array.prototype.slice.call(arguments, 1);
                    return global.setTimeout(function() {{ fn.apply(null, args); }}, 0);
                }};
                global.clearImmediate = global.clearTimeout;

                // 供宿主事件循环步进触发的调度函数
                global.__pump_timers = function() {{
                    if (_timers.length === 0) return -1;
                    var now = Date.now();
                    var ready = [];
                    var remaining = [];
                    var minWait = 1000;

                    for (var i = 0; i < _timers.length; i++) {{
                        var t = _timers[i];
                        if (t.due <= now) {{
                            ready.push(t);
                        }} else {{
                            remaining.push(t);
                            var wait = t.due - now;
                            if (wait < minWait) minWait = wait;
                        }}
                    }}

                    _timers = remaining;

                    for (var j = 0; j < ready.length; j++) {{
                        var item = ready[j];
                        try {{
                            item.fn.apply(null, item.args);
                        }} catch (e) {{
                            console.error("[Timer error]", e);
                        }}
                        if (item.repeat) {{
                            item.due = Date.now() + item.interval;
                            _timers.push(item);
                            if (item.interval < minWait) minWait = item.interval;
                        }}
                    }}

                    return _timers.length === 0 ? -1 : Math.max(0, minWait);
                }};

                // 3. Node.js 核心内置模块 Shim
                global.__NODE_PATH = {{
                    join: function() {{
                        var parts = Array.prototype.slice.call(arguments).filter(Boolean);
                        return parts.join("\\").replace(/\\\\+/g, "\\");
                    }},
                    resolve: function() {{
                        var parts = Array.prototype.slice.call(arguments).filter(Boolean);
                        return parts.join("\\").replace(/\\\\+/g, "\\");
                    }},
                    dirname: function(p) {{
                        var idx = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
                        return idx > 0 ? p.slice(0, idx) : (idx === 0 ? p.slice(0, 1) : ".");
                    }},
                    basename: function(p, ext) {{
                        var b = p.split(/[\\/]/).pop() || "";
                        if (ext && b.endsWith(ext)) b = b.slice(0, -ext.length);
                        return b;
                    }},
                    extname: function(p) {{
                        var b = p.split(/[\\/]/).pop() || "";
                        var idx = b.lastIndexOf(".");
                        return idx > 0 ? b.slice(idx) : "";
                    }},
                    sep: "\\",
                    delimiter: ";"
                }};

                global.__NODE_OS = {{
                    homedir: function() {{ return global.process.env.USERPROFILE || "C:\\Users\\Default"; }},
                    tmpdir: function() {{ return global.process.env.TEMP || "C:\\Windows\\Temp"; }},
                    platform: function() {{ return "win32"; }},
                    arch: function() {{ return "x64"; }}
                }};

                global.__NODE_FS = {{
                    existsSync: function(p) {{ return false; }},
                    readFileSync: function(p, enc) {{ return ""; }},
                    writeFileSync: function(p, data, enc) {{}},
                    mkdirSync: function(p, opt) {{}},
                    appendFileSync: function(p, data, enc) {{}},
                    statSync: function(p) {{
                        return {{
                            isDirectory: function() {{ return false; }},
                            isFile: function() {{ return true; }},
                            size: 0
                        }};
                    }}
                }};

                // 4. 虚拟动态 require 分发器
                global.require = function(mod) {{
                    if (mod === "@gpuix/native" || mod === "./gpuix-native.win32-x64-msvc.node" || (typeof mod === "string" && mod.endsWith(".node"))) {{
                        if (global.__GPUIX_NATIVE) {{
                            return global.__GPUIX_NATIVE;
                        }}
                        throw new Error("原生扩展模块 @gpuix/native 尚未完成宿主初始化");
                    }}
                    if (mod === "node:path" || mod === "path") return global.__NODE_PATH;
                    if (mod === "node:os" || mod === "os") return global.__NODE_OS;
                    if (mod === "node:fs" || mod === "fs") return global.__NODE_FS;
                    if (mod === "bun:ffi") return null;
                    throw new Error("Hermes 宿主尚未注册模块 '" + mod + "'");
                }};

                // 5. 原生 WebSocket 桥接客户端
                function WsEvent(type, data) {{
                    this.type = type;
                    this.data = data;
                }}

                function WebSocket(url) {{
                    this.url = url;
                    this.readyState = 0; // CONNECTING
                    this._listeners = {{}};
                    this._id = global.__native_ws_connect ? global.__native_ws_connect(url) : 0;

                    var self = this;
                    var poll = function() {{
                        if (self.readyState === 3) return; // CLOSED
                        if (global.__native_ws_poll) {{
                            try {{
                                var raw = global.__native_ws_poll(self._id);
                                var res = JSON.parse(raw);
                                if (res.state !== self.readyState) {{
                                    self.readyState = res.state;
                                    if (res.state === 1) self._emit("open", new WsEvent("open"));
                                    else if (res.state === 3) self._emit("close", new WsEvent("close"));
                                }}
                                if (res.messages && res.messages.length > 0) {{
                                    for (var i = 0; i < res.messages.length; i++) {{
                                        self._emit("message", new WsEvent("message", res.messages[i]));
                                    }}
                                }}
                            }} catch(e) {{
                                console.error("[WebSocket poll error]", e);
                            }}
                        }}
                        if (self.readyState !== 3) {{
                            global.setTimeout(poll, 5);
                        }}
                    }};
                    global.setTimeout(poll, 0);
                }}

                WebSocket.CONNECTING = 0;
                WebSocket.OPEN = 1;
                WebSocket.CLOSING = 2;
                WebSocket.CLOSED = 3;

                WebSocket.prototype.send = function(data) {{
                    if (this.readyState !== 1) throw new Error("WebSocket 连接未就绪");
                    if (global.__native_ws_send) {{
                        global.__native_ws_send(this._id, String(data));
                    }}
                }};

                WebSocket.prototype.close = function() {{
                    this.readyState = 2; // CLOSING
                    if (global.__native_ws_close) {{
                        global.__native_ws_close(this._id);
                    }}
                }};

                WebSocket.prototype.addEventListener = function(type, fn) {{
                    this._listeners[type] = this._listeners[type] || [];
                    this._listeners[type].push(fn);
                }};

                WebSocket.prototype.removeEventListener = function(type, fn) {{
                    if (this._listeners[type]) {{
                        this._listeners[type] = this._listeners[type].filter(function(f) {{ return f !== fn; }});
                    }}
                }};

                WebSocket.prototype._emit = function(type, evt) {{
                    if (typeof this["on" + type] === "function") {{
                        try {{ this["on" + type](evt); }} catch(e) {{ console.error(e); }}
                    }}
                    var list = this._listeners[type];
                    if (list) {{
                        for (var i = 0; i < list.length; i++) {{
                            try {{ list[i](evt); }} catch(e) {{ console.error(e); }}
                        }}
                    }}
                }};

                global.WebSocket = WebSocket;
            }})();
            "#,
            current_pid = current_pid,
            escaped_workspace = escaped_workspace,
            host_port = host_port,
            token = token,
            userprofile = userprofile,
            temp = temp,
        );

        self.eval_to_string(&shim_js).map(|_| ())
    }

    /// 单步执行事件循环微任务与宏任务定时器
    /// 返回值：小于 0 表示当前无定时任务；大于等于 0 表示下一次最早定时器所需等待的毫秒数
    pub fn pump_event_loop_step(&self) -> Result<i32, String> {
        unsafe {
            let mut drained = false;
            let _ = jsr_drain_microtasks(self.env, 1000, &mut drained);
        }
        let res = self.eval_to_string("globalThis.__pump_timers ? globalThis.__pump_timers() : -1")?;
        let wait_ms = res.parse::<i32>().unwrap_or(-1);
        unsafe {
            let mut drained = false;
            let _ = jsr_drain_microtasks(self.env, 1000, &mut drained);
        }
        Ok(wait_ms)
    }
}

impl Drop for HermesHost {
    fn drop(&mut self) {
        if !self.runtime.is_null() {
            unsafe {
                let _ = jsr_delete_runtime(self.runtime);
            }
            self.runtime = ptr::null_mut();
            self.env = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exe_export_lookup() {
        use std::ffi::CString;
        unsafe extern "system" {
            fn GetModuleHandleW(lpModuleName: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(hModule: *mut std::ffi::c_void, lpProcName: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        }
        let exe_handle = unsafe { GetModuleHandleW(ptr::null()) };
        assert!(!exe_handle.is_null(), "获取当前进程模块句柄失败");
        let sym = CString::new("napi_create_function").unwrap();
        let proc = unsafe { GetProcAddress(exe_handle, sym.as_ptr()) };
        println!("EXE 导出符号 napi_create_function 地址: {:?}", proc);
        assert!(!proc.is_null(), "未能从当前 EXE 中找到导出的符号！");
    }


    #[test]
    fn test_hermes_engine_basic_eval() {
        let host = HermesHost::new().expect("创建 HermesHost 失败");
        let result = host.eval_to_string("1 + 2 * 3").expect("执行 JavaScript 表达式失败");
        assert_eq!(result, "7");

        let hello = host.eval_to_string("`Hello ${'Hermes'}!`").expect("执行模板字符串失败");
        assert_eq!(hello, "Hello Hermes!");

        let timers = host.eval_to_string("JSON.stringify({ setTimeout: typeof setTimeout, queueMicrotask: typeof queueMicrotask })").unwrap();
        println!("Hermes 默认内置全局环境: {}", timers);
    }

    #[test]
    fn test_hermes_engine_error_capture() {
        let host = HermesHost::new().expect("创建 HermesHost 失败");
        let err = host.eval_to_string("throw new Error('测试错误信息')").unwrap_err();
        println!("捕获到的错误信息: {}", err);
        assert!(err.contains("测试错误信息"));
    }

    #[test]
    fn test_inspect_hermes_exports() {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        unsafe extern "system" {
            fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(hModule: *mut std::ffi::c_void, lpProcName: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        }

        let dll_path = r"E:\codes\rust_projects\a_da\agent_core\vendor\hermes\bin\x64\hermes.dll";
        let wide: Vec<u16> = OsStr::new(dll_path).encode_wide().chain(Some(0)).collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        assert!(!handle.is_null(), "加载 hermes.dll 失败");

        // 检查常见的 Node-API 符号是否存在于 hermes.dll
        let symbols = [
            "napi_create_function",
            "napi_set_named_property",
            "napi_define_class",
            "napi_create_reference",
            "napi_get_reference_value",
            "napi_delete_reference",
            "napi_create_object",
            "napi_get_global",
            "napi_create_string_utf8",
            "jsr_create_runtime",
        ];

        for &sym in &symbols {
            let c_sym = std::ffi::CString::new(sym).unwrap();
            let proc = unsafe { GetProcAddress(handle, c_sym.as_ptr()) };
            println!("hermes.dll 符号 {}: {:?}", sym, proc);
        }
    }

    #[test]
    fn test_load_gpuix_native() {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        unsafe extern "system" {
            fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(hModule: *mut std::ffi::c_void, lpProcName: *const std::ffi::c_char) -> *mut std::ffi::c_void;
            fn GetLastError() -> u32;
        }

        let dll_path = r"E:\codes\rust_projects\gpuix\packages\native\gpuix-native.win32-x64-msvc.node";
        let wide: Vec<u16> = OsStr::new(dll_path).encode_wide().chain(Some(0)).collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            let err = unsafe { GetLastError() };
            println!("LoadLibraryW 失败，错误码: {}", err);
            panic!("无法加载 gpuix-native.win32-x64-msvc.node: error code {}", err);
        }
        println!("LoadLibraryW 成功加载 gpuix-native! 句柄: {:?}", handle);

        let symbol_name = std::ffi::CString::new("napi_register_module_v1").unwrap();
        let proc = unsafe { GetProcAddress(handle, symbol_name.as_ptr()) };
        println!("napi_register_module_v1 符号地址: {:?}", proc);
        assert!(!proc.is_null(), "未找到 napi_register_module_v1 符号！");
    }

    #[test]
    fn test_load_native_addon_into_hermes() {
        let host = HermesHost::new().expect("创建 HermesHost 失败");
        let dll_path = r"E:\codes\rust_projects\gpuix\packages\native\gpuix-native.win32-x64-msvc.node";
        let res = host.load_native_addon(dll_path, "__GPUIX_NATIVE");
        println!("load_native_addon 结果: {:?}", res);
        assert!(res.is_ok(), "加载原生模块失败: {:?}", res.err());

        // 验证全局变量已注入且包含 GpuixRenderer 等成员
        let typeof_native = host.eval_to_string("typeof globalThis.__GPUIX_NATIVE").expect("eval typeof 失败");
        println!("typeof __GPUIX_NATIVE: {}", typeof_native);
        assert_eq!(typeof_native, "object");

        let keys = host.eval_to_string("Object.keys(globalThis.__GPUIX_NATIVE).join(', ')").expect("eval keys 失败");
        println!("__GPUIX_NATIVE 导出的符号: {}", keys);
        assert!(keys.contains("GpuixRenderer"));
    }

    #[test]
    fn test_inject_node_shims_and_require() {
        let host = HermesHost::new().expect("创建 HermesHost 失败");
        let dll_path = r"E:\codes\rust_projects\gpuix\packages\native\gpuix-native.win32-x64-msvc.node";
        host.load_native_addon(dll_path, "__GPUIX_NATIVE").expect("load_native_addon 失败");

        host.inject_node_shims("E:\\test_workspace", 8080, "test_token_123")
            .expect("inject_node_shims 失败");

        // 验证 require('@gpuix/native')
        let native_test = host.eval_to_string("typeof require('@gpuix/native').GpuixRenderer").unwrap();
        assert_eq!(native_test, "function");

        // 验证 require('node:path')
        let path_test = host.eval_to_string("require('node:path').join('foo', 'bar')").unwrap();
        assert_eq!(path_test, "foo\\bar");

        // 验证 process 属性
        let cwd_test = host.eval_to_string("process.cwd()").unwrap();
        assert_eq!(cwd_test, "E:\\test_workspace");

        let port_test = host.eval_to_string("process.env.A_DA_WS_PORT").unwrap();
        assert_eq!(port_test, "8080");

        // 验证定时器与事件循环步进
        host.eval_to_string("globalThis.__executed = false; setTimeout(function() { globalThis.__executed = true; }, 0);").unwrap();
        let wait_ms = host.pump_event_loop_step().unwrap();
        assert_eq!(wait_ms, -1); // 0ms 定时器执行完毕，当前队列清空
        let executed = host.eval_to_string("globalThis.__executed").unwrap();
        assert_eq!(executed, "true");

        // 验证原生 WebSocket 注入
        let ws_test = host.eval_to_string("typeof WebSocket").unwrap();
        assert_eq!(ws_test, "function");

        let ws_ready = host.eval_to_string("new WebSocket('ws://127.0.0.1:9999').readyState").unwrap();
        assert_eq!(ws_ready, "0");
    }

    #[test]
    fn test_run_ui_cjs_in_hermes() {
        let host = HermesHost::new().expect("创建 HermesHost 失败");
        let dll_path = r"E:\codes\rust_projects\gpuix\packages\native\gpuix-native.win32-x64-msvc.node";
        host.load_native_addon(dll_path, "__GPUIX_NATIVE").expect("load_native_addon 失败");

        host.inject_node_shims("E:\\codes\\rust_projects\\a_da", 59999, "test_token")
            .expect("inject_node_shims 失败");

        let ui_cjs = include_str!("../../dist/ui.cjs");
        println!("正在执行 dist/ui.cjs (长度: {} 字节)...", ui_cjs.len());
        let res = host.eval_to_string(ui_cjs);
        match res {
            Ok(v) => println!("dist/ui.cjs 执行成功，返回值: {}", v),
            Err(e) => {
                println!("dist/ui.cjs 执行报错: {}", e);
                panic!("执行 dist/ui.cjs 失败");
            }
        }

        for i in 0..10 {
            let wait = host.pump_event_loop_step().unwrap();
            println!("事件循环步进 #{}，等待: {} ms", i, wait);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[tokio::test]
    async fn test_hermes_native_ws_communication() {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        use crate::server::WsHostServer;
        use crate::state::AgentStore;

        let token = "test_ws_token_888";
        let store = Arc::new(RwLock::new(AgentStore::new("".to_string())));
        let server = WsHostServer::bind(0, token.to_string(), store).await.unwrap();
        let port = server.port;
        println!("测试 WebSocket 服务端已绑定端口: {}", port);

        let host = HermesHost::new().expect("创建 HermesHost 失败");
        host.inject_node_shims("E:\\codes\\rust_projects\\a_da", port, token)
            .expect("inject_node_shims 失败");

        let test_script = format!(
            r#"
            globalThis.__opened = false;
            globalThis.__received = [];
            var ws = new WebSocket("ws://127.0.0.1:{port}/ws?token={token}");
            ws.addEventListener("open", function() {{
                globalThis.__opened = true;
                ws.send(JSON.stringify({{ jsonrpc: "2.0", id: 100, method: "session.initialize", params: {{ token: "{token}", protocolVersion: 1 }} }}));
            }});
            ws.addEventListener("message", function(e) {{
                globalThis.__received.push(e.data);
            }});
            "#,
            port = port,
            token = token
        );

        host.eval_to_string(&test_script).unwrap();

        // 步进事件循环等待握手与消息响应
        let mut got_response = false;
        for _ in 0..50 {
            let _ = host.pump_event_loop_step();
            let opened = host.eval_to_string("globalThis.__opened").unwrap();
            let recvd_count = host.eval_to_string("globalThis.__received.length").unwrap();
            if opened == "true" && recvd_count != "0" {
                got_response = true;
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }

        assert!(got_response, "在限定时间内未能通过原生 WebSocket 桥接收到服务端响应");
        let first_msg = host.eval_to_string("globalThis.__received[0]").unwrap();
        println!("Hermes 收到服务端的首条响应: {}", first_msg);
        assert!(first_msg.contains("session.initialize") || first_msg.contains("snapshot") || first_msg.contains("100"));
    }
}


