//! **网关内置的前端托管**：把 `desktop-ui` 的构建产物直接挂在网关端口上。
//!
//! # 为什么放在网关里，而不是再起一个静态服务器
//!
//! 早先"网页访问"是**手工拼**的：先 `vite preview`/`http.server` 起一个端口给页面，
//! 再单独起网关给 `/rpc`。两个进程、两个端口、两套启动顺序，而且前端里写死的
//! `ws://127.0.0.1:52353/rpc` 一旦端口变了就连不上——这属于"能跑，但不是交付形态"。
//!
//! 现在同一个端口既是页面又是端点：
//!
//! ```text
//! GET  /            → desktop-ui/dist/index.html
//! GET  /assets/*.js → 构建产物
//! GET  /rpc（带 Upgrade: websocket） → 原有 WS 通道（不进本模块）
//! ```
//!
//! 好处是三个"天然"：**同源**（页面推 `ws://<location.host>/rpc` 必然正确）、
//! **单进程**（一个 `ada-gateway` 就是整站）、**单端口**（反代只要转发一个端口）。
//!
//! # 分流为什么用 `peek` 而不是改 tungstenite
//!
//! `tokio_tungstenite::accept_hdr_async` 只认 WS 握手，普通 `GET /` 会直接被它拒掉
//! （实测连状态码都拿不到，连接直接断）。而它又不提供"先看看这是什么请求"的钩子。
//!
//! 所以在本模块里先 `TcpStream::peek`：**只读不消费**，看完把头交回调用方分流——
//! 是升级请求就原样交给 `serve_client`（一个字节都没被吃掉），否则由本模块写 HTTP 响应。
//! 这样两条路都保留原语义，也没有把整个网关换成 hyper 的收益对不上改动量。
//!
//! # 鉴权边界
//!
//! **静态资源不需要 token**：页面本身不含任何用户数据（它只是一份 JS/CSS），
//! 真正的权限在 `/rpc` 的鉴权里——没有 token 的 WS 连接只能调 `gateway.info`。
//! 所以这里刻意不看 `AuthConfig`：否则浏览器连"输入 token 的页面"都打不开。

use std::io;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// 允许读取的请求头上限。超过就当 431 / 坏请求——HTTP 头不该有这么长。
const MAX_HEAD: usize = 16 * 1024;

/// 等一个完整请求头的上限。慢客户端（或被中间设备半开连接）不能一直占着连接。
const HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

/// 网关托管的一份前端产物。
///
/// `root` 在构造时就 `canonicalize` 过：之后所有路径校验都以它为基准，
/// 避免"校验时是相对路径、落盘时被 cwd 变化影响"。
#[derive(Clone, Debug)]
pub struct StaticSite {
    root: PathBuf,
}

impl StaticSite {
    /// 按候选顺序找一个**真的含 `index.html`** 的目录。
    ///
    /// 顺序刻意是"显式 → 环境变量 → 可执行文件旁边 → 当前目录"：
    /// 打包后 `ui/` 随二进制走；开发时从 `dist/` 或仓库根跑都能命中。
    /// 找不到就返回 `None`——**不 panic**，因为"没有前端产物"只是网页访问不可用，
    /// 桌面端和 `/rpc` 都不受影响。
    pub fn resolve(explicit: Option<&str>) -> Option<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Some(dir) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
            candidates.push(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var("A_DA_UI_DIR") {
            if !dir.trim().is_empty() {
                candidates.push(PathBuf::from(dir.trim()));
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("ui"));
                candidates.push(dir.join("../desktop-ui/dist"));
                candidates.push(dir.join("desktop-ui/dist"));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("desktop-ui/dist"));
            candidates.push(cwd.join("../desktop-ui/dist"));
            candidates.push(cwd.join("../../desktop-ui/dist"));
        }

        candidates
            .into_iter()
            .find(|dir| dir.join("index.html").is_file())
            .and_then(|dir| Self::at(dir).ok())
    }

    /// 以指定目录为站点根（要求目录存在）。
    pub fn at(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} 不是目录", root.display()),
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 日志用的路径。
    ///
    /// Windows 的 `canonicalize` 会返回 `\\?\E:\...`（长路径前缀）；`starts_with` 比较要它，
    /// 但打在日志里像乱码。只在展示时去掉。
    pub fn display_root(&self) -> String {
        let text = self.root.display().to_string();
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.html")
    }

    /// 把一个 URL 路径映射到文件并发出去。
    async fn serve(&self, stream: &mut TcpStream, url_path: &str, head_only: bool) -> io::Result<()> {
        let Some(rel) = safe_relative(url_path) else {
            return write_status(stream, 403, "Forbidden", "路径不合法", head_only).await;
        };

        // 目录（含根 `/`）落到 index.html：这是静态托管的标准行为，
        // 也是"打开 http://host:port/ 就能用"的前提。
        let mut candidate = if rel.as_os_str().is_empty() {
            self.index_path()
        } else {
            self.root.join(&rel)
        };
        if candidate.is_dir() {
            candidate.push("index.html");
        }

        let resolved = match candidate.canonicalize() {
            Ok(p) => p,
            // 不存在的路径 → 交给 `not_found`：可能是 SPA 的客户端路由，也可能真没有
            Err(_) => return self.not_found(stream, url_path, head_only).await,
        };

        // 🔴 第二道闸：符号链接可能把 `root` 里的路径指到外面去。
        // 段级校验（`safe_relative`）拦不住这个，必须用 canonicalize 后的前缀再确认一次。
        if !resolved.starts_with(&self.root) {
            return write_status(stream, 403, "Forbidden", "路径不合法", head_only).await;
        }
        if !resolved.is_file() {
            return self.not_found(stream, url_path, head_only).await;
        }

        self.send_file(stream, &resolved, head_only).await
    }

    /// 找不到文件时的两种收场。
    ///
    /// **有扩展名 = 真 404**，**没扩展名 = 回 index.html**。
    /// 后者是给前端客户端路由留的口子；前者必须有，否则 `/assets/missing.js`
    /// 会拿到一份 HTML，浏览器报的是 MIME 错误（真正的原因——文件不存在——就看不见了）。
    async fn not_found(
        &self,
        stream: &mut TcpStream,
        url_path: &str,
        head_only: bool,
    ) -> io::Result<()> {
        if Path::new(url_path).extension().is_some() {
            return write_status(
                stream,
                404,
                "Not Found",
                &format!("未找到：{url_path}"),
                head_only,
            )
            .await;
        }
        match self.index_path().canonicalize() {
            Ok(index) if index.starts_with(&self.root) && index.is_file() => {
                self.send_file(stream, &index, head_only).await
            }
            _ => write_status(stream, 404, "Not Found", "前端产物缺少 index.html", head_only).await,
        }
    }

    async fn send_file(&self, stream: &mut TcpStream, path: &Path, head_only: bool) -> io::Result<()> {
        let meta = tokio::fs::metadata(path).await?;
        let content_type = content_type(path);
        // `no-cache` 而不是长缓存：这是本地回环托管，开发时改完前端一刷新就要生效；
        // 带 hash 的资产本来也不会因为 no-cache 出问题（只是多一次 304 级别的往返）。
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: {content_type}\r\n\
             Content-Length: {}\r\n\
             Cache-Control: no-cache\r\n\
             X-Content-Type-Options: nosniff\r\n\
             Connection: close\r\n\r\n",
            meta.len()
        );
        stream.write_all(header.as_bytes()).await?;
        if !head_only {
            let mut file = tokio::fs::File::open(path).await?;
            tokio::io::copy(&mut file, stream).await?;
        }
        stream.flush().await?;
        drain_input(stream);
        let _ = stream.shutdown().await;
        Ok(())
    }
}

/// **只读不消费**地把请求头取出来。
///
/// 循环 `peek` 是因为 TCP 是流：头部可能分几段到达。每次 peek 都从 socket 的
/// **起点**重新读（不会被之前那次消费），所以只要对端继续发，`n` 就会变大。
pub async fn peek_head(stream: &TcpStream) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; MAX_HEAD];
    let deadline = tokio::time::Instant::now() + HEAD_TIMEOUT;

    loop {
        let n = stream.peek(&mut buf).await?;
        if n == 0 {
            // 对端已经关了（端口探测、健康检查都会这样）：交给调用方按坏请求收场
            return Ok(Vec::new());
        }
        if headers_complete(&buf[..n]) || n == buf.len() {
            return Ok(buf[..n].to_vec());
        }
        if tokio::time::Instant::now() >= deadline {
            // 超时就把"目前看到的"交回去：解析不出请求行 → 400，而不是永远挂着
            return Ok(buf[..n].to_vec());
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

fn headers_complete(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

/// 这是不是一个 WebSocket 升级请求（决定走 WS 还是静态资源）。
///
/// 判据只有一条：请求行是 `GET`，且头部有 `Upgrade: websocket`。
/// `Connection: Upgrade` 不单独判——有 `Upgrade` 头就够了，多判一条只会在
/// 某些代理改写头部的场景里误判成"普通 GET"。
pub fn is_websocket_upgrade(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");

    let Some(request_line) = lines.next() else {
        return false;
    };
    if !request_line
        .split_whitespace()
        .next()
        .map(|m| m.eq_ignore_ascii_case("GET"))
        .unwrap_or(false)
    {
        return false;
    }

    lines.any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("upgrade") && value.trim().to_ascii_lowercase().contains("websocket")
    })
}

/// 对一条**非升级**请求写 HTTP 响应（含 200 的静态文件、404、403、426、503）。
pub async fn respond(
    mut stream: TcpStream,
    head: &[u8],
    site: Option<&StaticSite>,
) -> io::Result<()> {
    // 🔴 第一件事：把 `peek` 看过的字节**真正读走**。
    //
    // `peek` 不消费，这些字节一直躺在接收缓冲里；等到写响应后 `close`，
    // 内核看到"还有未读数据"就发 **RST 而不是 FIN**——客户端读到一半拿到
    // `ConnectionReset`（Windows 上是 os error 10054），表现为页面偶尔白屏。
    // 头部长度是确定的，`read_exact` 立刻返回，不会等任何东西。
    consume_peeked(&mut stream, head.len()).await;

    let Some((method, target)) = parse_request_line(head) else {
        return write_status(&mut stream, 400, "Bad Request", "无法解析的 HTTP 请求", false).await;
    };

    let head_only = method.eq_ignore_ascii_case("HEAD");
    if !head_only && !method.eq_ignore_ascii_case("GET") {
        return write_status(
            &mut stream,
            405,
            "Method Not Allowed",
            "静态托管只支持 GET / HEAD",
            false,
        )
        .await;
    }

    // 去掉 query / fragment：`/?token=...` 也要能拿到页面。
    // token 由前端自己从 `location.search` 读，这里不解释它。
    let raw_path = target
        .split(['?', '#'])
        .next()
        .unwrap_or("/");

    // `/rpc` 是 WS 端点。不带 Upgrade 直接访问它的人通常是在浏览器地址栏里点，
    // 426 比一份 index.html 更能说明"这里不是网页"。
    if raw_path == "/rpc" || raw_path.starts_with("/rpc/") {
        return write_status(
            &mut stream,
            426,
            "Upgrade Required",
            "该路径是 WebSocket 端点（/rpc）；浏览页面请访问 /",
            head_only,
        )
        .await;
    }

    let Some(site) = site else {
        return write_status(
            &mut stream,
            503,
            "Service Unavailable",
            "网关未托管前端页面：请先构建 desktop-ui（npm --prefix desktop-ui run build），\
             或用 --ui-dir 指定构建产物目录",
            head_only,
        )
        .await;
    };

    site.serve(&mut stream, raw_path, head_only).await
}

fn parse_request_line(head: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    Some((method.to_string(), target.to_string()))
}

/// URL 路径 → 站点内的相对路径。
///
/// 三道筛：**段级黑名单**（`..` / 反斜杠 / 冒号 / NUL）、保证不越过 root、
/// 空段折叠。返回 `None` 一律当 403——宁可拒绝，也不"尽力猜一个安全的路径"。
fn safe_relative(url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path);
    let mut rel = PathBuf::new();

    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        // 反斜杠在 Windows 上是分隔符（`..\..\x`），冒号能拼出盘符或 NTFS 数据流，
        // NUL 会截断底层调用——都按非法处理。
        if segment.contains('\\') || segment.contains(':') || segment.contains('\0') {
            return None;
        }
        rel.push(segment);
    }

    Some(rel)
}

/// 最小百分号解码（`%XX` → 字节）。
///
/// 和 `auth::percent_decode` 同一套语义，但那边是私有实现、且这里要处理路径而非 query，
/// 所以单独留一份——路径里 `+` 是字面量，不能当空格。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn write_status(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    message: &str,
    head_only: bool,
) -> io::Result<()> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{code} {reason}</title>\
         <style>body{{font:14px/1.6 system-ui,sans-serif;margin:0;padding:48px;color:#2c2c2a;background:#fff}}\
         h1{{font-size:16px;font-weight:500;margin:0 0 12px}}p{{margin:0;color:#5f5e5a}}\
         code{{font-family:ui-monospace,monospace;background:#f1efe8;padding:1px 5px;border-radius:4px}}</style>\
         <h1>{code} {reason}</h1><p>{message}</p>"
    );
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );

    stream.write_all(header.as_bytes()).await?;
    if !head_only {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.flush().await?;
    drain_input(stream);
    let _ = stream.shutdown().await;
    Ok(())
}

/// 把 `peek` 已看到的字节消费掉（长度来自我们自己 peek 的结果，确定可读）。
async fn consume_peeked(stream: &mut TcpStream, len: usize) {
    if len == 0 {
        return;
    }
    let mut scratch = vec![0u8; len];
    // 读失败也只说明对端跑得比我们快，不值得中断响应——收尾的 `drain_input` 会兜底
    let _ = stream.read_exact(&mut scratch).await;
}

/// **关闭连接前必须把入站缓冲读空。**
///
/// 🔴 这是 `peek` 分流带来的副作用：`peek` 只看不消费，请求字节仍然躺在 socket 的
/// 接收缓冲里。此时直接 `close`，内核会判定"还有未读数据"，发的是 **RST 而不是 FIN**——
/// 客户端（尤其是还没读完响应体的浏览器/curl）会拿到 `ConnectionReset`，
/// 表现为"页面偶尔白屏""资源加载失败"。端到端测试就是这么抓到的。
///
/// 用 `try_read` 而不是 `read`：只排空**已经到达**的字节，不为了等一个可能永远不来的
/// 请求体把每个响应拖慢几百毫秒。GET/HEAD 本来就没有 body，这里通常只排掉那几百字节请求头。
fn drain_input(stream: &mut TcpStream) {
    let mut scratch = [0u8; 8 * 1024];
    loop {
        match stream.try_read(&mut scratch) {
            Ok(0) => break,                                      // 对端已关闭
            Ok(_) => continue,                                   // 还有，继续读
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_of(request: &str) -> Vec<u8> {
        request.as_bytes().to_vec()
    }

    fn req(method: &str, target: &str, upgrade: bool) -> Vec<u8> {
        let mut s = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:52353\r\n");
        if upgrade {
            s.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: x3JJHMbDL1EzLkh9GBhXDw==\r\n");
        }
        s.push_str("\r\n");
        head_of(&s)
    }

    #[test]
    fn upgrade_detection_is_case_insensitive() {
        let head = head_of(
            "GET /rpc HTTP/1.1\r\nhost: x\r\nUPGRADE: WebSocket\r\n\r\n",
        );
        assert!(is_websocket_upgrade(&head));
    }

    #[test]
    fn plain_get_is_not_upgrade() {
        assert!(!is_websocket_upgrade(&req("GET", "/", false)));
        assert!(!is_websocket_upgrade(&req("GET", "/rpc", false)));
    }

    #[test]
    fn post_upgrade_is_not_treated_as_ws() {
        // 请求行不是 GET 时，即使头里有 upgrade 也不算——WS 握手必须是 GET
        let head = head_of("POST /rpc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(!is_websocket_upgrade(&head));
    }

    #[test]
    fn request_line_parses() {
        let (m, t) = parse_request_line(&req("GET", "/assets/app.js", false)).unwrap();
        assert_eq!(m, "GET");
        assert_eq!(t, "/assets/app.js");
    }

    #[test]
    fn traversal_is_rejected() {
        assert!(safe_relative("/../etc/passwd").is_none());
        assert!(safe_relative("/assets/../../secret").is_none());
        assert!(safe_relative("/%2e%2e/secret").is_none());
        assert!(safe_relative("/..%5c..%5cwindows").is_none());
        assert!(safe_relative("/C:/Windows/win.ini").is_none());
        assert!(safe_relative("/a%00b").is_none());
    }

    #[test]
    fn normal_paths_are_kept() {
        assert_eq!(safe_relative("/").unwrap(), PathBuf::new());
        assert_eq!(
            safe_relative("/assets/index-abc123.js").unwrap(),
            PathBuf::from("assets").join("index-abc123.js")
        );
        // 重复斜杠折叠，不影响解析
        assert_eq!(
            safe_relative("//assets//app.css").unwrap(),
            PathBuf::from("assets").join("app.css")
        );
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(content_type(Path::new("index.html")), "text/html; charset=utf-8");
        assert_eq!(
            content_type(Path::new("app.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("logo.svg")), "image/svg+xml");
        assert_eq!(
            content_type(Path::new("unknown.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn percent_decode_keeps_plus_literal() {
        assert_eq!(percent_decode("a+b%20c"), "a+b c");
    }
}
