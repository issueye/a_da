//! 后台任务动作：终止一个任务。
//!
//! 任务列表由 `task.list` 投影到 `props.data.tasks`；这里只处理“停止”这一个
//! 有副作用的动作。终止的是 agent 进程里的后台进程树，不是网关进程。

use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use serde::Deserialize;

use super::{
    encode_query, partial_err, partial_ok, public_reason, redirect, wants_partial, AppError,
    AppState,
};

#[derive(Debug, Deserialize)]
pub struct KillTaskForm {
    #[serde(default)]
    thread_id: Option<String>,
}

pub async fn kill_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<KillTaskForm>,
) -> Result<Response, AppError> {
    match state.agent.kill_task(&task_id).await {
        Ok(_) => {
            if wants_partial(&headers) {
                return Ok(partial_ok("task_killed"));
            }
            Ok(redirect(&with_query(
                &redirect_target(&headers, form.thread_id.as_deref()),
                "notice=task_killed",
            )))
        }
        Err(error) => {
            let message = public_reason(&error);
            if wants_partial(&headers) {
                return Ok(partial_err(message));
            }
            Ok(redirect(&with_query(
                &redirect_target(&headers, form.thread_id.as_deref()),
                &format!("error={}", encode_query(&message)),
            )))
        }
    }
}

/// 只接受同源相对路径，避免开放重定向。
fn redirect_target(headers: &HeaderMap, thread_id: Option<&str>) -> String {
    let from_referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.parse::<axum::http::Uri>().ok())
        .map(|uri| {
            uri.path_and_query()
                .map(|path| path.as_str().to_string())
                .unwrap_or_else(|| uri.path().to_string())
        })
        .filter(|path| {
            path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.split('/').any(|part| part == "..")
        });
    if let Some(path) = from_referer {
        return path;
    }
    thread_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!("/threads/{id}"))
        .unwrap_or_else(|| "/".to_string())
}

fn with_query(path: &str, query: &str) -> String {
    if path.contains('?') {
        format!("{path}&{query}")
    } else {
        format!("{path}?{query}")
    }
}
