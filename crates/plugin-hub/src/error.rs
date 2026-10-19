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
    let message = escape_html(message);
    let body = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{status} · a_da 插件库</title><script src="https://cdn.tailwindcss.com"></script></head><body class="min-h-screen bg-slate-50 text-slate-900 flex items-center justify-center p-6"><div class="max-w-md w-full bg-white border border-slate-200 rounded-2xl p-8 shadow-sm"><p class="text-sm font-mono text-slate-400">{status}</p><h1 class="mt-2 text-xl font-semibold">{message}</h1><a class="mt-6 inline-flex text-sm text-blue-600 hover:underline" href="/">返回首页</a></div></body></html>"#
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
