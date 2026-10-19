use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::{plugin, release, user};
use crate::views::{
    self, BrowseTemplate, Common, HomeTemplate, ProfileTemplate, ProfileView,
    SearchResultsTemplate, Stats,
};

use super::PageQuery;

pub async fn home(
    State(state): State<AppState>,
    auth: AuthCtx,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let latest = plugin::latest(&state.pool, 9).await?;
    let top = plugin::top_downloads(&state.pool, 5).await?;
    let stats = Stats {
        users: user::count(&state.pool).await?,
        plugins: plugin::count_all(&state.pool).await?,
        releases: release::count_all(&state.pool).await?,
    };
    views::render(&HomeTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        latest,
        top,
        stats,
    })
}

pub async fn browse(
    State(state): State<AppState>,
    auth: AuthCtx,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let query = q.q.unwrap_or_default();
    let sort = match q.sort.as_deref() {
        Some("downloads") => "downloads",
        Some("stars") => "stars",
        Some("name") => "name",
        _ => "updated",
    };
    let page = q.page.unwrap_or(1).max(1);
    let per_page = 12;
    let total = plugin::count(&state.pool, &query).await?;
    let pages = ((total + per_page - 1) / per_page).max(1);
    let plugins = plugin::list(&state.pool, &query, sort, per_page, (page - 1) * per_page).await?;
    views::render(&BrowseTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            &query,
        ),
        plugins,
        total,
        page,
        pages,
        sort: sort.to_string(),
        q_url: urlencoding::encode(&query).into_owned(),
        q: query,
    })
}

pub async fn search_partial(
    State(state): State<AppState>,
    Query(q): Query<PageQuery>,
) -> Result<Response, AppError> {
    let query = q.q.unwrap_or_default();
    let plugins = plugin::list(&state.pool, &query, "updated", 8, 0).await?;
    views::render(&SearchResultsTemplate { plugins, q: query }).map(IntoResponse::into_response)
}

pub async fn profile(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(username): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let Some(user) = user::find_by_username(&state.pool, &username).await? else {
        return Err(AppError::NotFound);
    };
    let plugins = plugin::by_owner(&state.pool, &user.username).await?;
    views::render(&ProfileTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        profile: ProfileView {
            username: user.username.clone(),
            joined: views::format_ts(user.created_at),
            plugin_count: plugins.len() as i64,
        },
        plugins,
    })
}

pub async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "ada-plugin-hub" }))
}
