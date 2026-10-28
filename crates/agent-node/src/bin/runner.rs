//! **插件子进程入口**（P4 · SP-70）：读一条 JSON 请求、干一件事、写一条 JSON 结果。
//!
//! # 为什么要有独立的可执行文件
//!
//! 进程内档（Boa）没有内存护栏：Boa 没有堆上限 API，宿主的墙钟超时也拦不住同步段里的
//! 内存暴涨——插件把宿主进程吃光，整个 agent 一起死。放到子进程里，最坏结果是**这个子进程**
//! 死掉，宿主如实回报并按需重启下一次调用。
//!
//! 协议刻意做成**一次请求一条结果**（不是长连接）：
//!
//! - 没有常驻状态，就没有"上一个插件的残留影响了这一个"这类问题；
//! - 超时 = 杀掉进程（`kill_on_drop`），不需要在协议里设计"中止"消息；
//! - 代价是每次调用一次进程启动（数十毫秒级）。这是**非默认档**，值得。
//!
//! # 请求 / 响应
//!
//! ```json
//! {"kind":"inspect","path":"…","workspace":"…","needs":{"fs":true,"net":false},"timeoutMs":30000}
//! {"kind":"call","path":"…","workspace":"…","export":"web_search","args":{…}, …}
//! {"kind":"hook","path":"…","workspace":"…","point":"beforeToolCall","payload":{…}, …}
//! ```
//!
//! 响应一律是一个 JSON 对象：`{"ok":true,…}` 或 `{"ok":false,"error":"…"}`。
//! **错误也走 stdout**（进程退出码保持 0）：把"插件自己炸了"和"这个入口没跑起来"分开，
//! 是调用方唯一能依赖的判据——靠退出码分会让两种失败长得一模一样。

use std::io::{Read, Write};

use agent_node::plugins::PluginSandbox;
use ada_ts_engine::{Limits, Needs};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    kind: String,
    path: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    export: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default)]
    point: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    needs: Option<NeedsSpec>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    loop_iterations: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct NeedsSpec {
    #[serde(default)]
    fs_read: bool,
    #[serde(default)]
    fs_write: bool,
    #[serde(default)]
    net: bool,
    #[serde(default)]
    sqlite: bool,
    #[serde(default)]
    exec: bool,
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn main() {
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        return reply(&serde_json::json!({ "ok": false, "error": format!("读取请求失败: {e}") }));
    }
    let req: Request = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            return reply(&serde_json::json!({
                "ok": false,
                "error": format!("请求不是合法 JSON: {e}"),
            }))
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("建 tokio 运行时失败");
    let outcome = runtime.block_on(handle(req));
    reply(&outcome);
}

async fn handle(req: Request) -> serde_json::Value {
    let path = std::path::PathBuf::from(&req.path);
    let workspace = req
        .workspace
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let needs = match &req.needs {
        Some(n) => Needs {
            fs_read: n.fs_read,
            fs_write: n.fs_write,
            net: n.net,
            sqlite: n.sqlite,
            exec: n.exec,
        },
        // 缺省按**最紧**（什么都不给）：这个入口是"不可信第三方"跑的地方，
        // 默认值不该是宽的那一端。
        None => Needs {
            fs_read: false,
            fs_write: false,
            net: false,
            sqlite: false,
            exec: false,
        },
    };
    let limits = req
        .loop_iterations
        .map(Limits::tight)
        .unwrap_or_default();
    let timeout_s = req.timeout_ms.div_ceil(1000).max(1);

    match req.kind.as_str() {
        "inspect" => match PluginSandbox::inspect_plugin_full(&path, &workspace, needs).await {
            Ok(ins) => serde_json::json!({
                "ok": true,
                "tools": ins.tools,
                "prompts": ins.prompts,
                "skills": ins.skills,
                "hooks": ins.hooks,
            }),
            Err(e) => serde_json::json!({ "ok": false, "error": format!("{e}") }),
        },
        "call" => {
            let Some(export) = req.export.clone() else {
                return serde_json::json!({ "ok": false, "error": "call 请求缺少 export" });
            };
            let args = req.args.clone().unwrap_or(serde_json::json!({}));
            match PluginSandbox::call_tool_budgeted(
                &path,
                &export,
                args,
                &workspace,
                req.thread_id.as_deref(),
                timeout_s,
                needs,
                limits,
            )
            .await
            {
                Ok(res) => serde_json::json!({ "ok": true, "result": res }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e}") }),
            }
        }
        "hook" => {
            let Some(point) = req.point.clone() else {
                return serde_json::json!({ "ok": false, "error": "hook 请求缺少 point" });
            };
            let payload = req.payload.clone().unwrap_or(serde_json::json!({}));
            match PluginSandbox::call_hook_budgeted(
                &path, &point, payload, &workspace, timeout_s, needs, limits,
            )
            .await
            {
                Ok(raw) => serde_json::json!({ "ok": true, "result": raw }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e}") }),
            }
        }
        other => serde_json::json!({ "ok": false, "error": format!("未知的 kind: {other}") }),
    }
}

fn reply(value: &serde_json::Value) {
    let mut out = std::io::stdout();
    let _ = out.write_all(value.to_string().as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
