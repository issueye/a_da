/// 生成注入微内核环境的 `node:os` / `os` 模块 Polyfill 脚本
pub fn get_os_polyfill_script() -> String {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string())
        .replace('\\', "/");

    let tmp = std::env::temp_dir()
        .to_string_lossy()
        .to_string()
        .replace('\\', "/");

    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };

    let os_type = match std::env::consts::OS {
        "windows" => "Windows_NT",
        "macos" => "Darwin",
        "linux" => "Linux",
        other => other,
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };

    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string());

    let home_json = serde_json::to_string(&home).unwrap_or_else(|_| "\"\"".to_string());
    let tmp_json = serde_json::to_string(&tmp).unwrap_or_else(|_| "\"\"".to_string());
    let username_json = serde_json::to_string(&username).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"
        (function() {{
            const _home = {home_json};
            const _tmp = {tmp_json};
            const _username = {username_json};

            const osModule = {{
                homedir: function() {{ return _home; }},
                tmpdir: function() {{ return _tmp; }},
                platform: function() {{ return "{platform}"; }},
                type: function() {{ return "{os_type}"; }},
                arch: function() {{ return "{arch}"; }},
                release: function() {{ return "10.0.0"; }},
                hostname: function() {{ return "localhost"; }},
                userInfo: function() {{
                    return {{
                        username: _username,
                        homedir: _home,
                        shell: null,
                        uid: -1,
                        gid: -1
                    }};
                }},
                EOL: "{platform}" === "win32" ? "\r\n" : "\n",
                totalmem: function() {{ return 16 * 1024 * 1024 * 1024; }},
                freemem: function() {{ return 8 * 1024 * 1024 * 1024; }},
                cpus: function() {{
                    return [
                        {{ model: "Native Processor", speed: 3000, times: {{ user: 0, nice: 0, sys: 0, idle: 0, irq: 0 }} }}
                    ];
                }}
            }};

            globalThis.os = osModule;
        }})();
        "#
    )
}
