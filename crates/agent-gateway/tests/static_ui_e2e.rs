//! 静态托管与 `/rpc` 的端到端测试：**真 HTTP 请求**，跑的是生产同一条路由
//! （`agent_gateway::router::router`），不是测试里另抄一份分流逻辑。
//!
//! 不起真正的 agent 进程：`/rpc` 只验证握手与状态码，管理面方法由其他 e2e 覆盖。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agent_gateway::relay::AppState;
use agent_gateway::router::router;
use agent_gateway::static_ui::StaticSite;
use agent_gateway::{Gateway, WorkspacesStore};
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

/// 起一个网关实例（与生产同一条路由），返回监听地址。
async fn spawn(site: Option<StaticSite>) -> SocketAddr {
    // 🔴 注入临时的工作区存储：默认构造会指向用户真实的 `~/.a-da/gateway_workspaces.json`
    let store = std::env::temp_dir().join(format!("ada_gw_e2e_{}.json", uuid::Uuid::new_v4()));
    let gateway = Arc::new(
        Gateway::new("ada-coding", "C:/ada_static_e2e_workspace")
            .with_workspaces(Arc::new(WorkspacesStore::new(Some(store)))),
    );

    let state = AppState {
        gateway,
        default_workspace: "C:/ada_static_e2e_workspace".to_string(),
        site: site.map(Arc::new),
    };
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
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
                eprintln!("[request] 读取中断：{e}");
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

/// hyper 会把头部名小写成 `content-type`，断言不区分大小写。
fn has_header(head: &str, needle: &str) -> bool {
    head.to_lowercase().contains(&needle.to_lowercase())
}

fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn serves_index_assets_and_rejects_traversal() {
    let root = fixture();
    let site = StaticSite::at(&root).unwrap();
    let addr = spawn(Some(site)).await;

    // 1. 根路径 → index.html
    let (head, body) = request(addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "根路径应 200：{head}");
    assert!(has_header(&head, "content-type: text/html"), "应声明 html：{head}");
    assert!(body.contains("root"), "应返回 index.html 的内容：{body}");

    // 2. 带 query 的根路径也要能拿到页面（`?token=` 是网页访问的凭据入口）
    let (head, body) = request(addr, &get("/?token=abc")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "带 query 应 200：{head}");
    assert!(body.contains("root"));

    // 3. 静态资产：内容与 MIME 都要对
    let (head, body) = request(addr, &get("/assets/app.js")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "资产应 200：{head}");
    assert!(has_header(&head, "content-type: text/javascript"), "应声明 js：{head}");
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

    // 8. HEAD：有真实 Content-Length、无 body
    let (head, body) = request(
        addr,
        "HEAD / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert!(!head.to_lowercase().contains("content-length: 0"), "HEAD 的长度要真实：{head}");
    assert!(body.is_empty(), "HEAD 不该有响应体");

    // 9. 方法限制
    let (head, _) = request(
        addr,
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 405 Method Not Allowed"), "{head}");

    // 10. 404 页里的路径必须转义（否则静态托管就是反射型 XSS 入口）
    let (head, body) = request(addr, &get("/%3Cimg%20src=x%3E.js")).await;
    assert!(head.starts_with("HTTP/1.1 404 Not Found"), "{head}");
    assert!(body.contains("&lt;img"), "尖括号应被转义：{body}");
    assert!(!body.contains("<img src=x>"), "不该原样输出用户输入：{body}");

    let _ = std::fs::remove_dir_all(&root);
}

/// `/rpc` 与静态资源共用端口：不带升级头要给 426，带升级头要能握手。
#[tokio::test]
async fn rpc_shares_the_port_with_static_pages() {
    let root = fixture();
    let site = StaticSite::at(&root).unwrap();
    let addr = spawn(Some(site)).await;

    // 浏览器地址栏直接点 `/rpc` → 426（而不是一份页面）
    let (head, _) = request(addr, &get("/rpc")).await;
    assert!(head.starts_with("HTTP/1.1 426 Upgrade Required"), "{head}");

    // 真正的升级请求 → 101（`role=agent` 走控制面，不会拉起实例）
    let url = format!("ws://{addr}/rpc?role=agent");
    let (ws, resp) = tokio_tungstenite::connect_async(url).await.expect("WS 握手");
    assert_eq!(resp.status().as_u16(), 101, "应升级成功");
    drop(ws);

    // 同一端口上的页面仍然可用
    let (head, _) = request(addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK"));

    let _ = std::fs::remove_dir_all(&root);
}

/// 没有前端产物时，根路径要给**可执行的提示**，而不是空白或 500。
#[tokio::test]
async fn missing_site_explains_how_to_fix() {
    let addr = spawn(None).await;

    let (head, body) = request(addr, &get("/")).await;
    assert!(head.starts_with("HTTP/1.1 503"), "未托管应 503：{head}");
    assert!(
        body.contains("--ui-dir") && body.contains("desktop-ui"),
        "提示里要写清怎么修：{body}"
    );

    // 静态资源不可用不影响 `/rpc` 的路由存在（426 说明路由在）
    let (head, _) = request(addr, &get("/rpc")).await;
    assert!(head.starts_with("HTTP/1.1 426"), "{head}");
}
