use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::{plugin, star};
use crate::views::{self, Common, StarButtonTemplate, StarTarget};

use super::CsrfForm;

pub async fn toggle(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    headers: HeaderMap,
    form: Result<Form<CsrfForm>, FormRejection>,
) -> Result<Response, AppError> {
    let Some(user) = auth.user.as_ref() else {
        if headers.contains_key("HX-Request") {
            let mut response = axum::response::Response::new(axum::body::Body::empty());
            response.headers_mut().insert(
                "HX-Redirect",
                axum::http::HeaderValue::from_static("/login"),
            );
            return Ok(response);
        }
        return Ok(Redirect::to(&format!("/login?next=/plugins/{owner}/{slug}")).into_response());
    };
    let form = form.map_err(|e| AppError::bad(format!("读取表单失败: {e}")))?;
    auth.validate_csrf(&form._csrf)?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    let (starred, stars) = star::toggle(&state.pool, &user.id, &detail.id).await?;
    if starred {
        crate::models::webhook::dispatch_plugin(
            &state.pool,
            detail.owner_id.as_deref(),
            detail.org_id.as_deref(),
            "star.created",
            serde_json::json!({
                "plugin": { "owner": detail.owner.clone(), "slug": detail.slug.clone() },
                "user": user.username.clone(),
                "stars": stars
            }),
        )
        .await;
    }
    if headers.contains_key("HX-Request") {
        views::render(&StarButtonTemplate {
            common: Common::from_auth(&auth, None, ""),
            plugin: StarTarget {
                owner: detail.owner.clone(),
                slug: detail.slug.clone(),
            },
            starred,
            stars,
        })
        .map(IntoResponse::into_response)
    } else {
        Ok(Redirect::to(&format!(
            "/plugins/{owner}/{slug}?ok={}",
            if starred { "starred" } else { "unstarred" }
        ))
        .into_response())
    }
}
