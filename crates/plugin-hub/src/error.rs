use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("请先登录")]
    Unauthorized,
    #[error("没有权限执行这个操作")]
    Forbidden,
    #[error("资源不存在")]
    NotFound,
    #[error("重定向")]
    Redirect(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Redirect(to) => Redirect::to(&to).into_response(),
            Self::Db(err) => {
                tracing::error!(error = %err, "数据库错误");
                self_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "服务器内部错误，请稍后重试",
                )
            }
            Self::Internal(err) => {
                tracing::error!(error = %err, "内部错误");
                self_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "服务器内部错误，请稍后重试",
                )
            }
            Self::BadRequest(msg) => self_page(StatusCode::BAD_REQUEST, &msg),
            Self::Unauthorized => self_page(StatusCode::UNAUTHORIZED, "请先登录"),
            Self::Forbidden => self_page(StatusCode::FORBIDDEN, "没有权限执行这个操作"),
            Self::NotFound => self_page(StatusCode::NOT_FOUND, "资源不存在"),
        }
    }
}

fn self_page(status: StatusCode, message: &str) -> Response {
    // 错误页与正常页面共用同一套设计系统：本地 /static/app.css + theme.js，
    // **不引 CDN**（README 的「本地静态资源」承诺对整个站点成立）。
    let message = escape_html(message);
    let body = format!(
        r##"<!doctype html>
<html lang="zh-CN" class="light">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{status} · a_da 插件库</title>
<script src="/static/theme.js"></script>
<link rel="stylesheet" href="/static/app.css">
</head>
<body class="app-shell">
<a class="skip-link" href="#main">跳到主要内容</a>
<main id="main" class="app-container flex flex-1 items-center justify-center py-10">
  <div class="card w-full max-w-md p-6 text-center sm:p-8">
    <span class="empty-icon mx-auto"><svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/></svg></span>
    <p class="mt-4 font-mono text-xs text-faint">{status}</p>
    <h1 class="mt-1.5 text-lg font-semibold tracking-tight text-ink">{message}</h1>
    <div class="mt-6 flex flex-wrap items-center justify-center gap-2">
      <a class="btn btn-primary btn-sm" href="/">返回首页</a>
      <a class="btn btn-secondary btn-sm" href="/plugins">浏览插件</a>
    </div>
  </div>
</main>
</body>
</html>"##
    );
    (status, Html(body)).into_response()
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
