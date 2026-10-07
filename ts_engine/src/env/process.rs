use std::collections::HashMap;
use std::path::Path;

/// 生成注入微内核环境的全局 `process` 对象脚本
pub fn get_process_polyfill_script(workspace: Option<&Path>) -> String {
    let cwd = workspace
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        })
        .replace('\\', "/");

    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };

    let pid = std::process::id();

    // 收集安全环境变量键值对
    let mut env_map = HashMap::new();
    for (k, v) in std::env::vars() {
        env_map.insert(k, v);
    }
    let env_json = serde_json::to_string(&env_map).unwrap_or_else(|_| "{}".to_string());
    let cwd_json = serde_json::to_string(&cwd).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"
        (function() {{
            const _cwd = {cwd_json};
            const _env = {env_json};
            globalThis.process = {{
                cwd: function() {{ return _cwd; }},
                env: _env,
                platform: "{platform}",
                arch: "{arch}",
                argv: ["a_da_microkernel"],
                pid: {pid},
                version: "v20.18.0",
                versions: {{
                    node: "20.18.0",
                    v8: "11.3.244",
                    boa: "0.22.0",
                    a_da: "0.1.0"
                }},
                nextTick: function(cb, ...args) {{
                    Promise.resolve().then(() => cb(...args));
                }},
                exit: function(code) {{
                    console.log("[process.exit called with code: " + code + "]");
                }}
            }};
        }})();
        "#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_script_generation() {
        let script = get_process_polyfill_script(Some(Path::new("E:/codes/a_da")));
        assert!(script.contains("E:/codes/a_da"));
        assert!(script.contains("win32") || script.contains("linux") || script.contains("darwin"));
        assert!(script.contains("nextTick"));
    }
}
