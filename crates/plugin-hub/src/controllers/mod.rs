pub mod api;
pub mod auth;
pub mod oauth;
pub mod orgs;
pub mod pages;
pub mod plugins;
pub mod releases;
pub mod stars;
pub mod tokens;
pub mod webhooks;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub ok: Option<String>,
    pub err: Option<String>,
    pub q: Option<String>,
    pub sort: Option<String>,
    pub page: Option<i64>,
    pub next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    pub _csrf: String,
}

/// 只允许站内相对路径，挡掉 `//evil.com` 与绝对 URL。
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") => n.to_string(),
        _ => "/".to_string(),
    }
}
