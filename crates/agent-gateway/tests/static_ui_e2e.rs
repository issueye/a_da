//! 静态托管的端到端测试：**真 TCP 连接**、真 HTTP 请求，覆盖分流与路径安全。
//!
//! 不起整个网关（那会拉起 agent 实例），只把 `peek_head` → 分流 → `respond`
//! 这条真实路径跑一遍。WS 分支用一条标记响应代替，验证的是"升级请求**没有**落到
//! 静态托管"，而不是 WS 本身（那由 `gateway_e2e.rs` 覆盖）。

use std::net::SocketAddr;
use std::path::PathBuf;

use agent_gateway::static_ui::{is_websocket_upgrade, peek_head, respond, StaticSite};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 建一份最小前端产物：根 index.html、assets/app.js、一个子目录。
fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!("ada_static_e2e_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("index.html"), "<!doctype html><title>a_da</title>root").unwrap();
    std::fs::write(root.join("sub/index.html"), "<!doctype html><title>sub</title>").unwrap();
    std::fs::write(root.join("assets/app.js"), "console.log('a_da')").unwrap();
    root
}

/// 起一个"迷你网关"：完全是 `main.rs` 里那段分流的同构版本。
async fn spawn_server(site: StaticSite) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let site = site.clone();
            tokio::spawn(async move {
                let head = match peek_head(&stream).await {
                    Ok(h) => h,
                    Err(_) => return,
                };
                if is_websocket_upgrade(&head) {
                    let mut stream = stream;
                    // 标记：说明这次连接走了 WS 分支，没有被静态托管接管。
                    // 这里也要先把 peek 过的字节读掉——真实网关里是 tungstenite 消费它们，
                    // 这个假分支得自己来，否则关闭时同样会 RST。
                    let mut discard = vec![0u8; head.len()];
                    let _ = stream.read_exact(&mut discard).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 204 No Content\r\nX-Branch: ws\r\n\r\n")
                        .await;
                    let _ = stream.shutdown().await;
                    return;
                }
                let _ = respond(stream, &head, Some(&site)).await;
            });
        }
    });
    addr
}

/// 发一条原始 HTTP 请求，返回（响应头，响应体）。
async fn request(addr: SocketAddr, raw: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                eprintln!(
                    "[request] {} 读取中断：{e}（已收 {} 字节：{:?}）",
                    raw.lines().next().unwrap_or("?"),
                    buf.len(),
                    String::from_utf8_lossy(&buf)
                );
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    match text.split_once("\r\n\r\n") {
        Some((head, body)) => (head.to_string(), body.to_string()),
        None => (text, String::new()),
    }
}

fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn serves_index_assets_and_rejects_traversal() {
    let root = fixture();
    let site = StaticSite::at(&root).unwrap();
    let addr = spawn_server(site).await;

    // 1. 根路径 → index.html
    let (head, body) = request(addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "根路径应 200：{head}");
    assert!(head.contains("Content-Type: text/html"), "应声明 html：{head}");
    assert!(body.contains("root"), "应返回 index.html 的内容：{body}");

    // 2. 带 query 的根路径也要能拿到页面（`?token=` 是网页访问的凭据入口）
    let (head, body) = request(addr, &get("/?token=abc")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "带 query 应 200：{head}");
    assert!(body.contains("root"));

    // 3. 静态资产：内容与 MIME 都要对
    let (head, body) = request(addr, &get("/assets/app.js")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "资产应 200：{head}");
    assert!(head.contains("Content-Type: text/javascript"), "应声明 js：{head}");
    assert!(body.contains("console.log"));

    // 4. 子目录 → 子目录的 index.html
    let (head, body) = request(addr, &get("/sub/")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert!(body.contains("<title>sub</title>"));

    // 5. 目录穿越必须拒绝（明文与编码两种写法）
    for path in ["/../secret.txt", "/assets/../../secret.txt", "/%2e%2e/secret.txt"] {
        let (head, _) = request(addr, &get(path)).await;
        assert!(
            head.starts_with("HTTP/1.1 403 Forbidden"),
            "{path} 应 403，实际：{head}"
        );
    }

    // 6. 带扩展名但不存在 → 真 404（不能拿 index.html 冒充）
    let (head, body) = request(addr, &get("/assets/missing.js")).await;
    assert!(head.starts_with("HTTP/1.1 404 Not Found"), "{head}");
    assert!(!body.contains("root"), "404 不该返回页面内容");

    // 7. 无扩展名 → SPA 兜底回 index.html
    let (head, body) = request(addr, &get("/some/client/route")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(body.contains("root"));

    // 8. HEAD：有长度、无 body
    let (head, body) = request(
        addr,
        "HEAD / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert!(head.to_lowercase().contains("content-length:"));
    assert!(body.is_empty(), "HEAD 不该有响应体");

    // 9. `/rpc` 不带升级头 → 426，而不是一份页面
    let (head, _) = request(addr, &get("/rpc")).await;
    assert!(head.starts_with("HTTP/1.1 426 Upgrade Required"), "{head}");

    // 10. 升级请求走 WS 分支，不进静态托管
    let upgrade = "GET /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: Upgrade\r\n\
                   Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
                   Sec-WebSocket-Key: x3JJHMbDL1EzLkh9GBhXDw==\r\n\r\n";
    let (head, _) = request(addr, upgrade).await;
    assert!(head.contains("X-Branch: ws"), "升级请求应走 WS 分支：{head}");

    // 11. 方法限制
    let (head, _) = request(
        addr,
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 405 Method Not Allowed"), "{head}");

    let _ = std::fs::remove_dir_all(&root);
}

/// 没有前端产物时，根路径要给**可执行的提示**，而不是空白或 500。
#[tokio::test]
async fn missing_site_explains_how_to_fix() {
    let root = fixture();
    let site = StaticSite::at(&root).unwrap();
    let addr = spawn_server(site).await;

    // 复用同一个 server，但用一条走不到文件的路径来触发 404 分支；
    // 真正的"未托管"分支（site = None）在这里单独走一遍 respond。
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let none_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let head = peek_head(&stream).await.unwrap();
        let _ = respond(stream, &head, None).await;
    });

    let (head, body) = request(none_addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 503"), "未托管应 503：{head}");
    assert!(
        body.contains("--ui-dir") && body.contains("desktop-ui"),
        "提示里要写清怎么修：{body}"
    );

    // fixture server 仍然可用（避免 unused 警告，同时证明两条路互不影响）
    let (head, _) = request(addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"));

    let _ = std::fs::remove_dir_all(&root);
}
