use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Request, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum_extra::extract::cookie::Key;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::controllers::{
    api, auth, oauth, orgs, pages, plugins, releases, stars, tokens, webhooks,
};
use crate::error::AppError;
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub storage: Arc<Storage>,
    pub key: Key,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(pool: SqlitePool, storage: Storage, config: Config) -> Self {
        let digest = Sha256::digest(config.secret.as_bytes());
        let mut material = [0u8; 64];
        material[..32].copy_from_slice(&digest);
        material[32..].copy_from_slice(&digest);
        let key = Key::from(&material);
        Self {
            pool,
            storage: Arc::new(storage),
            key,
            config: Arc::new(config),
        }
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::home))
        .route("/plugins", get(pages::browse).post(plugins::create))
        .route("/plugins/new", get(plugins::new_form))
        .route(
            "/plugins/{owner}/{slug}",
            get(plugins::detail).post(plugins::update),
        )
        .route("/plugins/{owner}/{slug}/edit", get(plugins::edit_form))
        .route("/plugins/{owner}/{slug}/delete", post(plugins::delete))
        .route(
            "/plugins/{owner}/{slug}/releases/new",
            get(releases::new_form).post(releases::create),
        )
        .route(
            "/plugins/{owner}/{slug}/releases/{version}/download",
            get(releases::download),
        )
        .route("/plugins/{owner}/{slug}/star", post(stars::toggle))
        .route("/u/{username}", get(pages::profile))
        .route("/orgs", get(orgs::list).post(orgs::create))
        .route("/orgs/new", get(orgs::new_form))
        .route("/orgs/{name}", get(orgs::detail))
        .route("/orgs/{name}/members", post(orgs::add_member))
        .route(
            "/orgs/{name}/members/{member_id}/remove",
            post(orgs::remove_member),
        )
        .route("/orgs/{name}/teams", post(orgs::create_team))
        .route(
            "/orgs/{name}/teams/{team_id}/members",
            post(orgs::add_team_member),
        )
        .route("/search", get(pages::search_partial))
        .route("/register", get(auth::register_form).post(auth::register))
        .route("/login", get(auth::login_form).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/auth/github", get(oauth::start))
        .route("/auth/github/callback", get(oauth::callback))
        .route("/settings/tokens", get(tokens::list).post(tokens::create))
        .route("/settings/tokens/{id}/revoke", post(tokens::revoke))
        .route(
            "/settings/webhooks",
            get(webhooks::list).post(webhooks::create),
        )
        .route("/settings/webhooks/{id}/delete", post(webhooks::delete))
        .route("/api/v1/plugins", get(api::list).post(api::create))
        .route("/api/v1/plugins/{owner}/{slug}", get(api::detail))
        .route(
            "/api/v1/plugins/{owner}/{slug}/releases",
            post(api::create_release),
        )
        .route("/api/v1/plugins/{owner}/{slug}/star", post(api::star))
        .route("/healthz", get(pages::healthz))
        .route("/static/app.css", get(app_css))
        .route("/static/htmx.min.js", get(htmx_js))
        .route("/static/alpine.min.js", get(alpine_js))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(
            state.config.max_upload_bytes + 1024 * 1024,
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn not_found() -> Response {
    AppError::NotFound.into_response()
}

async fn security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    response
}

async fn app_css() -> Response {
    static_asset(
        include_bytes!("../static/app.css"),
        "text/css; charset=utf-8",
    )
}

async fn htmx_js() -> Response {
    static_asset(
        include_bytes!("../static/htmx.min.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn alpine_js() -> Response {
    static_asset(
        include_bytes!("../static/alpine.min.js"),
        "text/javascript; charset=utf-8",
    )
}

/// 静态资源直接嵌进二进制：部署时不需要再带 `static/` 目录。
fn static_asset(bytes: &'static [u8], content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        axum::body::Body::from(bytes),
    )
        .into_response()
}
