use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::token;
use crate::views::{self, Common, TokenPageTemplate};

use super::{CsrfForm, PageQuery};

#[derive(Debug, Deserialize)]
pub struct TokenForm {
    pub _csrf: String,
    pub name: String,
    #[serde(default)]
    pub read: Option<String>,
    #[serde(default)]
    pub write: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthCtx,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let user = auth.require_user()?;
    let tokens = token::list_for_user(&state.pool, &user.id).await?;
    views::render(&TokenPageTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        tokens,
        new_token: None,
        error: None,
    })
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthCtx,
    Form(form): Form<TokenForm>,
) -> Result<Html<String>, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let name = form.name.trim();
    if name.is_empty() || name.len() > 60 {
        return render_page(&state, &auth, None, Some("Token 名称长度需在 1–60 之间")).await;
    }
    let scopes = normalize_scopes(form.read.is_some(), form.write.is_some());
    if scopes.is_empty() {
        return render_page(&state, &auth, None, Some("至少选择一个作用域")).await;
    }
    let (raw, hash, prefix) = token::generate();
    token::create(&state.pool, &user.id, name, &hash, &prefix, &scopes).await?;
    render_page(&state, &auth, Some(raw), None).await
}

pub async fn revoke(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    token::revoke(&state.pool, &user.id, &id).await?;
    Ok(Redirect::to("/settings/tokens?ok=token_revoked").into_response())
}

async fn render_page(
    state: &AppState,
    auth: &AuthCtx,
    new_token: Option<String>,
    error: Option<&str>,
) -> Result<Html<String>, AppError> {
    let user = auth.user.as_ref().ok_or(AppError::Unauthorized)?;
    let tokens = token::list_for_user(&state.pool, &user.id).await?;
    views::render(&TokenPageTemplate {
        common: Common::from_auth(auth, None, ""),
        tokens,
        new_token,
        error: error.map(str::to_string),
    })
}

fn normalize_scopes(read: bool, write: bool) -> String {
    let mut out = Vec::new();
    if read {
        out.push("read");
    }
    if write {
        out.push("write");
    }
    out.join(",")
}
