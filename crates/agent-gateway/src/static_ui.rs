//! **网关内置的前端托管**（axum handler）：把 `desktop-ui` 的构建产物挂在网关端口上。
//!
//! # 为什么放在网关里
//!
//! 早先"网页访问"是**手工拼**的：先 `vite preview`/`http.server` 起一个端口给页面，
//! 再单独起网关给 `/rpc`。两个进程、两个端口、两套启动顺序，而且前端写死的端口一变就连不上。
//!
//! 现在路由在 axum 里统一：
//!
//! ```text
//! GET  /            → desktop-ui/dist/index.html
//! GET  /assets/*.js → 构建产物
//! GET  /rpc         → WebSocket（`relay::ws_handler`，本模块不参与）
//! ```
//!
//! 好处是三个"天然"：**同源**（页面推 `ws://<location.host>/rpc` 必然正确）、
//! **单进程**（一个 `ada-gateway` 就是整站）、**单端口**（反代只要转发一个端口）。
//!
//! # 鉴权边界
//!
//! **静态资源不需要 token**：页面只是 JS/CSS，不含任何用户数据；真正的权限在 `/rpc` 上。
//! 否则浏览器连"输入 token 的页面"都打不开。
//!
//! # 连接生命周期
//!
//! 手写 HTTP 时代最疼的一个坑（`peek` 了请求头却不消费，关闭时内核发 RST 而不是 FIN，
//! 客户端拿到 `ConnectionReset`）在 axum 下**结构性消失**：响应体由 hyper 统一写、
//! 连接由 hyper 统一收，不存在"我们绕过它自己 close"的路径。

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::relay::AppState;

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
    pub fn at(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
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

    /// 把一个 URL 路径映射到文件并返回响应。
    async fn serve(&self, url_path: &str, head_only: bool) -> Response {
        let Some(rel) = safe_relative(url_path) else {
            return status_page(StatusCode::FORBIDDEN, "路径不合法");
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
            Err(_) => return self.not_found(url_path, head_only).await,
        };

        // 🔴 第二道闸：符号链接可能把 `root` 里的路径指到外面去。
        // 段级校验（`safe_relative`）拦不住这个，必须用 canonicalize 后的前缀再确认一次。
        if !resolved.starts_with(&self.root) {
            return status_page(StatusCode::FORBIDDEN, "路径不合法");
        }
        if !resolved.is_file() {
            return self.not_found(url_path, head_only).await;
        }

        match tokio::fs::read(&resolved).await {
            Ok(bytes) => file_response(&resolved, bytes, head_only),
            Err(_) => self.not_found(url_path, head_only).await,
        }
    }

    /// 找不到文件时的两种收场。
    ///
    /// **有扩展名 = 真 404**，**没扩展名 = 回 index.html**。
    /// 后者是给前端客户端路由留的口子；前者必须有，否则 `/assets/missing.js`
    /// 会拿到一份 HTML，浏览器报的是 MIME 错误（真正的原因——文件不存在——就看不见了）。
    async fn not_found(&self, url_path: &str, head_only: bool) -> Response {
        if Path::new(url_path).extension().is_some() {
            // 展示**解码后**的路径：`/%3Cimg%3E.js` 对用户没有意义，`/<img>.js` 才看得懂。
            // 解码带来的注入风险由 `status_page` 的转义兜住（这也是它存在的理由）。
            return status_page(
                StatusCode::NOT_FOUND,
                &format!("未找到：{}", percent_decode(url_path)),
            );
        }
        match self.index_path().canonicalize() {
            Ok(index) if index.starts_with(&self.root) && index.is_file() => {
                match tokio::fs::read(&index).await {
                    Ok(bytes) => file_response(&index, bytes, head_only),
                    Err(_) => status_page(StatusCode::NOT_FOUND, "前端产物缺少 index.html"),
                }
            }
            _ => status_page(StatusCode::NOT_FOUND, "前端产物缺少 index.html"),
        }
    }
}

/// 静态资源的 fallback handler（`/rpc` 由 `relay::ws_handler` 独占）。
pub async fn static_handler(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return status_page(StatusCode::METHOD_NOT_ALLOWED, "静态托管只支持 GET / HEAD");
    }

    let Some(site) = state.site.clone() else {
        return status_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "网关未托管前端页面：请先构建 desktop-ui（npm --prefix desktop-ui run build），\
             或用 --ui-dir 指定构建产物目录",
        );
    };

    site.serve(uri.path(), method == Method::HEAD).await
}

fn file_response(path: &Path, bytes: Vec<u8>, head_only: bool) -> Response {
    let len = bytes.len();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(path))
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-content-type-options", "nosniff");

    // HEAD 要带真实长度：body 空着由 hyper 补 `Content-Length: 0` 就是错的
    if head_only {
        builder = builder.header(header::CONTENT_LENGTH, len);
    }

    let body = if head_only {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    builder
        .body(body)
        .unwrap_or_else(|_| status_page(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败"))
}

fn status_page(status: StatusCode, message: &str) -> Response {
    let reason = status.canonical_reason().unwrap_or("");
    // 🔴 message 里可能带着用户可控的 URL 路径（例如 `/<script>`），必须转义，
    // 否则静态托管本身就成了反射型 XSS 的入口。
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{code} {reason}</title>\
         <style>body{{font:14px/1.6 system-ui,sans-serif;margin:0;padding:48px;color:#2c2c2a;background:#fff}}\
         h1{{font-size:16px;font-weight:500;margin:0 0 12px}}p{{margin:0;color:#5f5e5a}}\
         code{{font-family:ui-monospace,monospace;background:#f1efe8;padding:1px 5px;border-radius:4px}}</style>\
         <h1>{code} {reason}</h1><p>{message}</p>",
        code = status.as_u16(),
        reason = escape_html(reason),
        message = escape_html(message),
    );

    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn error_page_escapes_user_input() {
        // 路径里的尖括号不能原样进 HTML（否则静态托管就成了反射型 XSS 入口）
        let escaped = escape_html("/<script>alert(1)</script>");
        assert!(!escaped.contains("<script>"));
        assert!(escaped.contains("&lt;script&gt;"));
    }
}
