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

use super::auth::is_unique;

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
            "metadata": r.metadata(),
        })).collect::<Vec<_>>(),
    })))
}

/// 插件市场列表：公开插件 + 最新版本 + 包元数据。
///
/// 与 `/api/v1/plugins` 的区别是**带上最新 release**（市场卡片要显示版本、作者），
/// 但仍然只读、只暴露公开插件，作用域与普通列表一致（`read`）。
pub async fn market(
    State(state): State<AppState>,
    auth: ApiAuth,
    Query(q): Query<ApiListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require("read")?;
    let limit = q.limit.unwrap_or(24).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = plugin::list_market(&state.pool, &q.q, limit, offset).await?;
    let total = plugin::count(&state.pool, &q.q).await?;

    let plugins: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let metadata = row.metadata_value();
            let author = metadata
                .get("author")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&row.owner)
                .to_string();
            let version = row.latest_version.clone();
            json!({
                "owner": row.owner,
                "slug": row.slug,
                "name": row.name,
                "description": row.description,
                "author": author,
                "version": version,
                "has_release": version.is_some(),
                "tags": row.tags_list(),
                "downloads": row.downloads,
                "stars": row.stars,
                "updated_at": row.updated_at,
                "metadata": metadata,
            })
        })
        .collect();

    Ok(Json(json!({
        "plugins": plugins,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ApiCreateQuery {
    /// `?upsert=1`：同名插件已存在时**更新元数据**并返回 200，而不是 409。
    /// a_da 的「上传到插件库」用它把"首次创建"与"再次发布"收敛成同一条路径。
    #[serde(default)]
    pub upsert: Option<String>,
}

/// 当前 Token 对应的用户与作用域：桌面端在发布前用它确认"有没有 write 权限"。
pub async fn me(auth: ApiAuth) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require("read")?;
    Ok(Json(json!({
        "username": auth.user.username,
        "scopes": auth.token.scopes_list(),
        "canWrite": auth.token.has_scope("write"),
    })))
}

pub async fn create(
    State(state): State<AppState>,
    auth: ApiAuth,
    Query(q): Query<ApiCreateQuery>,
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
    let upsert = matches!(q.upsert.as_deref(), Some("1" | "true" | "yes"));

    if upsert {
        if let Some(existing) = plugin::find_detail(&state.pool, &auth.user.username, &slug).await?
        {
            // 只允许更新**自己的个人插件**；组织插件/别人的插件走正常 409，
            // 不能靠 upsert 绕过权限。
            if existing.org_id.is_none()
                && existing.owner_id.as_deref() == Some(auth.user.id.as_str())
            {
                // 桌面端上传时简介/README 可以留空（最终由包内 metadata/README 同步）。
                // upsert 阶段若直接写空会把人工维护的内容清掉，所以空值保留旧值。
                let mut update_form = form.clone();
                if update_form.description.trim().is_empty() {
                    update_form.description = existing.description.clone();
                }
                if update_form.readme.trim().is_empty() {
                    update_form.readme = existing.readme.clone();
                }
                plugin::update(&state.pool, &existing.id, &update_form, &tags).await?;
                crate::models::webhook::dispatch(
                    state.pool.clone(),
                    auth.user.id.clone(),
                    "plugin.updated",
                    json!({ "plugin": { "id": existing.id, "owner": auth.user.username, "slug": slug } }),
                );
                return Ok((
                    StatusCode::OK,
                    Json(json!({
                        "ok": true,
                        "existing": true,
                        "id": existing.id,
                        "owner": auth.user.username,
                        "slug": slug,
                    })),
                )
                    .into_response());
            }
        }
    }

    let id = match plugin::create(&state.pool, Some(&auth.user.id), None, &slug, &form, &tags).await
    {
        Ok(id) => id,
        Err(err) if is_unique(&err) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!(
                    "同名插件已存在：{}/{}（发布新版本请带 ?upsert=1）",
                    auth.user.username, slug
                ),
            ));
        }
        Err(err) => return Err(err.into()),
    };
    crate::models::webhook::dispatch(
        state.pool.clone(),
        auth.user.id.clone(),
        "plugin.created",
        json!({ "plugin": { "id": id, "owner": auth.user.username, "slug": slug } }),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "ok": true,
            "existing": false,
            "id": id,
            "owner": auth.user.username,
            "slug": slug,
        })),
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
    let requested_version = version.trim().to_string();
    let Some(bytes) = asset else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "缺少 asset 文件"));
    };
    if bytes.len() > state.config.max_upload_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ZIP 超过上传上限",
        ));
    }
    let inspection = crate::metadata::inspect_package(&bytes)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    let metadata = inspection.metadata;
    let package_readme = inspection.readme;
    let version = metadata
        .effective_version(&requested_version)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    if version.is_empty()
        || version.len() > 32
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "非法版本号"));
    }
    let stored = state
        .storage
        .save_plugin_zip(&owner, &slug, &version, &bytes)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    let result = release::create(
        &state.pool,
        &detail.id,
        &version,
        notes.trim(),
        &stored.relative_path,
        stored.size as i64,
        &stored.sha256,
        &metadata.to_json_string(),
    )
    .await;
    match result {
        Ok(id) => {
            if let Err(error) = plugin::sync_package_text(
                &state.pool,
                &detail.id,
                &metadata.description,
                package_readme.as_deref(),
            )
            .await
            {
                tracing::warn!(%error, "发布成功后同步插件简介/README 失败");
            }
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
                Json(json!({
                    "ok": true,
                    "owner": detail.owner,
                    "slug": detail.slug,
                    "version": version,
                })),
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
