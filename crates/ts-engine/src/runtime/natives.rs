use std::path::PathBuf;

use crate::env::Needs;
use crate::net::NetPolicy;

use super::PureTsRuntime;

/// 按声明注册 fs / fetch / sleep 三类异步原生能力。
pub(super) fn register(
    runtime: &PureTsRuntime,
    workspace: Option<PathBuf>,
    needs: Needs,
    net: NetPolicy,
) {
    let net_for_thread = net.clone();
    // 自动注册 Tokio 异步 fs 原生能力并附加沙箱安全防御
    let ws_r = workspace.clone();
    runtime.register_async_fn_gated(needs.fs_read, "__native_fs_read_file_async", move |args| {
        let rel = args.get(0).cloned().unwrap_or_default();
        let base = ws_r.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        async move {
            let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                .map_err(|e| format!("沙箱拦截: {e}"))?;
            tokio::fs::read_to_string(&safe)
                .await
                .map_err(|e| format!("读取失败: {e}"))
        }
    });

    let ws_w = workspace.clone();
    runtime.register_async_fn_gated(
        needs.fs_write,
        "__native_fs_write_file_async",
        move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let content = args.get(1).cloned().unwrap_or_default();
            let base = ws_w.clone().unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                if let Some(parent) = safe.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                tokio::fs::write(&safe, content)
                    .await
                    .map_err(|e| format!("写入失败: {e}"))?;
                Ok("ok".to_string())
            }
        },
    );

    let ws_mkdir = workspace.clone();
    runtime.register_async_fn_gated(needs.fs_write, "__native_fs_mkdir_async", move |args| {
        let rel = args.get(0).cloned().unwrap_or_default();
        let base = ws_mkdir.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        async move {
            let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                .map_err(|e| format!("沙箱拦截: {e}"))?;
            tokio::fs::create_dir_all(&safe)
                .await
                .map_err(|e| format!("创建目录失败: {e}"))?;
            Ok("ok".to_string())
        }
    });

    let ws_readdir = workspace.clone();
    runtime.register_async_fn_gated(needs.fs_read, "__native_fs_readdir_async", move |args| {
        let rel = args.get(0).cloned().unwrap_or_default();
        let base = ws_readdir.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        async move {
            let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                .map_err(|e| format!("沙箱拦截: {e}"))?;
            let mut entries = Vec::new();
            let mut dir = tokio::fs::read_dir(&safe)
                .await
                .map_err(|e| format!("读取目录失败: {e}"))?;
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Ok(name) = entry.file_name().into_string() {
                    entries.push(name);
                }
            }
            Ok(serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()))
        }
    });

    let ws_stat = workspace.clone();
    runtime.register_async_fn_gated(
        needs.fs_read || needs.fs_write,
        "__native_fs_stat_async",
        move |args| {
            let rel = args.get(0).cloned().unwrap_or_default();
            let base = ws_stat.clone().unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            async move {
                let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                    .map_err(|e| format!("沙箱拦截: {e}"))?;
                let meta = tokio::fs::metadata(&safe)
                    .await
                    .map_err(|e| format!("获取元数据失败: {e}"))?;
                let size = meta.len();
                let is_file = meta.is_file();
                let is_dir = meta.is_dir();
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                Ok(serde_json::json!({
                    "size": size,
                    "isFile": is_file,
                    "isDirectory": is_dir,
                    "mtimeMs": mtime_ms
                })
                .to_string())
            }
        },
    );

    let ws_rm = workspace.clone();
    runtime.register_async_fn_gated(needs.fs_write, "__native_fs_rm_async", move |args| {
        let rel = args.get(0).cloned().unwrap_or_default();
        let base = ws_rm.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        async move {
            let safe = crate::sandbox::check_workspace_sandbox(&base, &rel)
                .map_err(|e| format!("沙箱拦截: {e}"))?;
            if safe.is_dir() {
                tokio::fs::remove_dir_all(&safe)
                    .await
                    .map_err(|e| format!("删除目录失败: {e}"))?;
            } else if safe.exists() {
                tokio::fs::remove_file(&safe)
                    .await
                    .map_err(|e| format!("删除文件失败: {e}"))?;
            }
            Ok("ok".to_string())
        }
    });

    // 自动注册 Tokio 驱动的原生异步 HTTP fetch 桥接能力（**按声明**：未声明就整条不注册）
    runtime.register_async_fn_gated(needs.net, "__native_fetch_async", move |args| {
        let url = args.get(0).cloned().unwrap_or_default();
        let policy = net_for_thread.clone();
        let opts_str = args.get(1).cloned().unwrap_or_default();
        async move {
            // 🔴 **策略先于请求**（P4 SP-72）：白名单之外的域名连连接都不该建，
            // 而且失败原因要说人话（插件作者据此知道该改哪个域名）。
            if let Err(reason) = policy.check_url(&url) {
                return Err(format!("网络请求被宿主策略拒绝：{reason}"));
            }
            let (method, headers, body_str, proxy_str) =
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&opts_str) {
                    let m = val
                        .get("method")
                        .and_then(|v| v.as_str())
                        .unwrap_or("GET")
                        .to_string();
                    let mut h_map = std::collections::HashMap::new();
                    if let Some(h_obj) = val.get("headers").and_then(|v| v.as_object()) {
                        for (k, v) in h_obj {
                            if let Some(vs) = v.as_str() {
                                h_map.insert(k.clone(), vs.to_string());
                            }
                        }
                    }
                    let b = val
                        .get("body")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let p = val
                        .get("proxy")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    (m, h_map, b, p)
                } else {
                    (
                        "GET".to_string(),
                        std::collections::HashMap::new(),
                        None,
                        None,
                    )
                };

            // 获取代理设置：优先使用请求显式指定的 proxy，其次读取环境变量。
            //
            // 两种写法都认（`fetch(url, { proxy })` 与宿主进程的 `HTTPS_PROXY` 等）：
            // 插件作者改不了宿主环境，运维也改不了插件源码，两边都得有入口。
            // 地址统一归一化：`127.0.0.1:7897` 这种没有 scheme 的写法最常见。
            let effective_proxy = proxy_str
                .or_else(|| {
                    std::env::var("HTTPS_PROXY")
                        .or_else(|_| std::env::var("https_proxy"))
                        .or_else(|_| std::env::var("HTTP_PROXY"))
                        .or_else(|_| std::env::var("http_proxy"))
                        .or_else(|_| std::env::var("ALL_PROXY"))
                        .or_else(|_| std::env::var("all_proxy"))
                        .ok()
                })
                .and_then(|p| crate::net::normalize_proxy_url(&p));

            // 超时来自宿主策略（此前是硬编码 30s）
            let mut client_builder = reqwest::Client::builder().timeout(policy.timeout);

            if let Some(ref proxy_url) = effective_proxy {
                // 🔴 配错就**报错**，不能 warn 完偷偷直连：用户设代理意味着"流量必须走代理"，
                // 静默直连会让插件以为自己走了代理，实际把请求直接发到了目标站
                // （隐私/合规上比"请求失败"严重得多）。
                let p = reqwest::Proxy::all(proxy_url)
                    .map_err(|e| format!("代理配置无效（{proxy_url}）：{e}"))?;
                client_builder = client_builder.proxy(p);
            }

            let client = client_builder
                .build()
                .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;

            let http_method = match method.to_uppercase().as_str() {
                "GET" => reqwest::Method::GET,
                "POST" => reqwest::Method::POST,
                "PUT" => reqwest::Method::PUT,
                "DELETE" => reqwest::Method::DELETE,
                "PATCH" => reqwest::Method::PATCH,
                "HEAD" => reqwest::Method::HEAD,
                _ => reqwest::Method::GET,
            };

            let mut req = client.request(http_method, &url);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            if let Some(b) = body_str {
                req = req.body(b);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| format!("HTTP 请求失败: {e}"))?;
            let status = resp.status().as_u16();
            let status_text = resp.status().canonical_reason().unwrap_or("").to_string();

            let mut resp_headers = std::collections::HashMap::new();
            for (k, v) in resp.headers() {
                if let Ok(val_str) = v.to_str() {
                    resp_headers.insert(k.as_str().to_lowercase(), val_str.to_string());
                }
            }

            let body_text = resp
                .text()
                .await
                .map_err(|e| format!("读取响应文本失败: {e}"))?;

            let out_json = serde_json::json!({
                "status": status,
                "statusText": status_text,
                "ok": (200..300).contains(&status),
                "headers": resp_headers,
                "body": body_text
            });

            Ok(out_json.to_string())
        }
    });

    // 自动注册 Tokio 驱动的原生异步 sleep 桥接能力（赋能 setTimeout / setInterval）
    runtime.register_async_fn("__native_sleep_async", move |args| {
        let ms_str = args.get(0).cloned().unwrap_or_else(|| "0".to_string());
        let ms = ms_str.parse::<u64>().unwrap_or(0);
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Ok("ok".to_string())
        }
    });
}
