use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::webhook;
use crate::views::{self, Common, WebhookPageTemplate};

use super::{CsrfForm, PageQuery};

#[derive(Debug, Deserialize)]
pub struct WebhookForm {
    pub _csrf: String,
    pub url: String,
    #[serde(default)]
    pub plugin_created: Option<String>,
    #[serde(default)]
    pub plugin_updated: Option<String>,
    #[serde(default)]
    pub release_published: Option<String>,
    #[serde(default)]
    pub star_created: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthCtx,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let user = auth.require_user()?;
    let hooks = webhook::list_for_user(&state.pool, &user.id).await?;
    let deliveries = webhook::recent_deliveries(&state.pool, &user.id, 30).await?;
    views::render(&WebhookPageTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        webhooks: hooks,
        deliveries,
        new_webhook: None,
        error: None,
    })
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthCtx,
    Form(form): Form<WebhookForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let url = form.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 500 {
        return render_page(&state, &auth, None, Some("Webhook URL 必须是 http(s) 地址")).await;
    }
    let events = normalize_events(&form);
    if events.is_empty() {
        return render_page(&state, &auth, None, Some("至少选择一个事件")).await;
    }
    webhook::create(&state.pool, &user.id, url, &events).await?;
    Ok(Redirect::to("/settings/webhooks?ok=webhook_created").into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    webhook::delete(&state.pool, &user.id, &id).await?;
    Ok(Redirect::to("/settings/webhooks?ok=webhook_deleted").into_response())
}

async fn render_page(
    state: &AppState,
    auth: &AuthCtx,
    new_webhook: Option<webhook::Webhook>,
    error: Option<&str>,
) -> Result<Response, AppError> {
    let user = auth.user.as_ref().ok_or(AppError::Unauthorized)?;
    let hooks = webhook::list_for_user(&state.pool, &user.id).await?;
    let deliveries = webhook::recent_deliveries(&state.pool, &user.id, 30).await?;
    views::render(&WebhookPageTemplate {
        common: Common::from_auth(auth, None, ""),
        webhooks: hooks,
        deliveries,
        new_webhook,
        error: error.map(str::to_string),
    })
    .map(IntoResponse::into_response)
}

fn normalize_events(form: &WebhookForm) -> String {
    let mut out = Vec::new();
    if form.plugin_created.is_some() {
        out.push("plugin.created");
    }
    if form.plugin_updated.is_some() {
        out.push("plugin.updated");
    }
    if form.release_published.is_some() {
        out.push("release.published");
    }
    if form.star_created.is_some() {
        out.push("star.created");
    }
    out.join(",")
}
