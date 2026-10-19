use axum::Json;
use axum::extract::{FromRequestParts, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::models::User;
use crate::models::{plugin, release, star, token, user};

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("数据库错误: {err}"),
        )
    }
}

pub struct ApiAuth {
    pub user: User,
    pub token: token::ApiToken,
}

impl ApiAuth {
    pub fn require(&self, scope: &str) -> Result<(), ApiError> {
        if self.token.has_scope(scope) {
            Ok(())
        } else {
            Err(ApiError::new(
                StatusCode::FORBIDDEN,
                format!("token 缺少作用域 `{scope}`"),
            ))
        }
    }
}

impl FromRequestParts<AppState> for ApiAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "缺少 Bearer token"))?;
        let hash = token::hash_token(raw);
        let token = token::find_active_by_hash(&state.pool, &hash)
            .await?
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "token 无效或已撤销"))?;
        token::touch(&state.pool, &token.id).await?;
        let user = user::find_by_id(&state.pool, &token.user_id)
            .await?
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "token 对应用户不存在"))?;
        Ok(Self { user, token })
    }
}

#[derive(Debug, Deserialize)]
pub struct ApiListQuery {
    #[serde(default)]
    pub q: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ApiCreatePlugin {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub readme: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub tags: String,
}

pub async fn list(
    State(state): State<AppState>,
    auth: ApiAuth,
    Query(q): Query<ApiListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require("read")?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);
    let plugins = plugin::list(&state.pool, &q.q, "updated", limit, offset).await?;
    Ok(Json(json!({
        "plugins": plugins.iter().map(|p| json!({
            "owner": p.owner,
            "owner_kind": p.owner_kind,
            "slug": p.slug,
            "name": p.name,
            "description": p.description,
            "tags": p.tags_list(),
            "downloads": p.downloads,
            "stars": p.stars,
            "updated_at": p.updated_at,
        })).collect::<Vec<_>>(),
        "limit": limit,
        "offset": offset,
    })))
}

pub async fn detail(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require("read")?;
    let Some(p) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "插件不存在"));
    };
    let releases = release::list_for_plugin(&state.pool, &p.id).await?;
    Ok(Json(json!({
        "owner": p.owner,
        "owner_kind": p.owner_kind,
        "slug": p.slug,
        "name": p.name,
        "description": p.description,
        "readme": p.readme,
        "homepage": p.homepage,
        "license": p.license,
        "tags": p.tags_list(),
        "downloads": p.downloads,
        "stars": p.stars,
        "created_at": p.created_at,
        "updated_at": p.updated_at,
        "releases": releases.iter().map(|r| json!({
            "version": r.version,
            "notes": r.notes,
            "size": r.asset_size,
            "sha256": r.sha256,
            "downloads": r.downloads,
            "created_at": r.created_at,
        })).collect::<Vec<_>>(),
    })))
}

pub async fn create(
    State(state): State<AppState>,
    auth: ApiAuth,
    Json(body): Json<ApiCreatePlugin>,
) -> Result<Response, ApiError> {
    auth.require("write")?;
    let slug = body.slug.trim().to_lowercase();
    if slug.len() < 2
        || slug.len() > 64
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "非法 slug"));
    }
    if body.name.trim().is_empty() || body.name.len() > 80 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "名称长度需在 1–80 之间",
        ));
    }
    let form = plugin::PluginFormData {
        name: body.name.trim().to_string(),
        description: body.description.trim().to_string(),
        readme: body.readme,
        homepage: body.homepage.trim().to_string(),
        license: body.license.trim().to_string(),
        tags: body.tags.clone(),
    };
    let tags = plugin::normalize_tags(&body.tags);
    let id = plugin::create(&state.pool, Some(&auth.user.id), None, &slug, &form, &tags).await?;
    crate::models::webhook::dispatch(
        state.pool.clone(),
        auth.user.id.clone(),
        "plugin.created",
        json!({ "plugin": { "id": id, "owner": auth.user.username, "slug": slug } }),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "id": id, "owner": auth.user.username, "slug": slug })),
    )
        .into_response())
}

pub async fn create_release(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path((owner, slug)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    auth.require("write")?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "插件不存在"));
    };
    if !crate::controllers::plugins::can_manage(&state.pool, &auth.user, &detail).await? {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "没有权限发布该插件"));
    }
    let mut version = String::new();
    let mut notes = String::new();
    let mut asset: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("读取上传失败: {e}")))?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };
        match name.as_str() {
            "version" => {
                version = field.text().await.map_err(|e| {
                    ApiError::new(StatusCode::BAD_REQUEST, format!("读取版本失败: {e}"))
                })?
            }
            "notes" => {
                notes = field.text().await.map_err(|e| {
                    ApiError::new(StatusCode::BAD_REQUEST, format!("读取说明失败: {e}"))
                })?
            }
            "asset" => {
                asset = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| {
                            ApiError::new(StatusCode::BAD_REQUEST, format!("读取 ZIP 失败: {e}"))
                        })?
                        .to_vec(),
                )
            }
            _ => {}
        }
    }
    let version = version.trim();
    if version.is_empty()
        || version.len() > 32
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "非法版本号"));
    }
    let Some(bytes) = asset else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "缺少 asset 文件"));
    };
    if bytes.len() > state.config.max_upload_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ZIP 超过上传上限",
        ));
    }
    let stored = state
        .storage
        .save_plugin_zip(&owner, &slug, version, &bytes)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    let result = release::create(
        &state.pool,
        &detail.id,
        version,
        notes.trim(),
        &stored.relative_path,
        stored.size as i64,
        &stored.sha256,
    )
    .await;
    match result {
        Ok(id) => {
            crate::models::webhook::dispatch_plugin(
                &state.pool,
                detail.owner_id.as_deref(),
                detail.org_id.as_deref(),
                "release.published",
                json!({ "plugin": { "owner": detail.owner.clone(), "slug": detail.slug.clone() }, "release": { "id": id, "version": version } }),
            )
            .await;
            Ok((
                StatusCode::CREATED,
                Json(json!({ "ok": true, "version": version })),
            )
                .into_response())
        }
        Err(err)
            if err
                .as_database_error()
                .map(|e| e.is_unique_violation())
                .unwrap_or(false) =>
        {
            state.storage.remove_asset(&stored.relative_path).await;
            Err(ApiError::new(StatusCode::CONFLICT, "版本已存在"))
        }
        Err(err) => {
            state.storage.remove_asset(&stored.relative_path).await;
            Err(err.into())
        }
    }
}

pub async fn star(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require("write")?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "插件不存在"));
    };
    let (starred, stars) = star::toggle(&state.pool, &auth.user.id, &detail.id).await?;
    if starred {
        crate::models::webhook::dispatch_plugin(
            &state.pool,
            detail.owner_id.as_deref(),
            detail.org_id.as_deref(),
            "star.created",
            json!({
                "plugin": { "owner": detail.owner.clone(), "slug": detail.slug.clone() },
                "user": auth.user.username.clone(),
                "stars": stars
            }),
        )
        .await;
    }
    Ok(Json(json!({ "starred": starred, "stars": stars })))
}
