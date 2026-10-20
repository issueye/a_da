use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::{plugin, release};
use crate::views::{self, Common, ReleaseFormTemplate};

use super::auth::is_unique;

pub async fn new_form(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Html<String>, AppError> {
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !crate::controllers::plugins::can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }
    views::render(&ReleaseFormTemplate {
        common: Common::from_auth(&auth, None, ""),
        owner,
        slug,
        name: detail.name,
        error: None,
    })
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !crate::controllers::plugins::can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }

    let mut version = String::new();
    let mut notes = String::new();
    let mut csrf = String::new();
    let mut asset: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad(format!("读取上传内容失败: {e}")))?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };
        match name.as_str() {
            "version" => {
                version = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(format!("读取版本失败: {e}")))?;
            }
            "notes" => {
                notes = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(format!("读取发布说明失败: {e}")))?;
            }
            "_csrf" => {
                csrf = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(format!("读取 CSRF 失败: {e}")))?;
            }
            "asset" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::bad(format!("读取 ZIP 失败: {e}")))?;
                asset = Some(bytes.to_vec());
            }
            _ => {}
        }
    }
    auth.validate_csrf(&csrf)?;

    let requested_version = version.trim().to_string();
    let Some(bytes) = asset else {
        return render_form(
            &auth,
            &owner,
            &slug,
            &detail.name,
            Some("请选择插件 ZIP 文件".to_string()),
        )
        .map(IntoResponse::into_response);
    };
    if bytes.len() > state.config.max_upload_bytes {
        return render_form(
            &auth,
            &owner,
            &slug,
            &detail.name,
            Some(format!(
                "ZIP 超过上传上限（{} MB）",
                state.config.max_upload_bytes / 1024 / 1024
            )),
        )
        .map(IntoResponse::into_response);
    }

    // 发布前从包根 index.ts **静态**提取 metadata：必填 name/description/author/version，
    // 且表单版本（可留空）必须与包内版本一致；同时取包内 README.md 用于同步仓库展示内容。
    let inspection = match crate::metadata::inspect_package(&bytes) {
        Ok(inspection) => inspection,
        Err(error) => {
            return render_form(&auth, &owner, &slug, &detail.name, Some(error))
                .map(IntoResponse::into_response);
        }
    };
    let metadata = inspection.metadata;
    let package_readme = inspection.readme;
    let version = match metadata.effective_version(&requested_version) {
        Ok(version) => version,
        Err(error) => {
            return render_form(&auth, &owner, &slug, &detail.name, Some(error))
                .map(IntoResponse::into_response);
        }
    };
    if let Err(error) = validate_version(&version) {
        return render_form(&auth, &owner, &slug, &detail.name, Some(error))
            .map(IntoResponse::into_response);
    }

    let stored = state
        .storage
        .save_plugin_zip(&owner, &slug, &version, &bytes)
        .await
        .map_err(AppError::bad)?;
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
        Ok(_) => {
            // 插件包是展示内容的来源：简介取 metadata.description，README 取包根 README.md。
            // 包内没有 README 时保留人工维护的那份（sync_package_text 的 None 分支）。
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
                serde_json::json!({
                    "plugin": { "owner": detail.owner, "slug": detail.slug },
                    "release": { "version": version }
                }),
            )
            .await;
            Ok(Redirect::to(&format!("/plugins/{owner}/{slug}?ok=released")).into_response())
        }
        Err(err) if is_unique(&err) => {
            state.storage.remove_asset(&stored.relative_path).await;
            render_form(
                &auth,
                &owner,
                &slug,
                &detail.name,
                Some(format!("版本 {version} 已存在")),
            )
            .map(IntoResponse::into_response)
        }
        Err(err) => {
            state.storage.remove_asset(&stored.relative_path).await;
            Err(err.into())
        }
    }
}

pub async fn download(
    State(state): State<AppState>,
    Path((owner, slug, version)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    let Some(asset) = release::find_asset(&state.pool, &owner, &slug, &version).await? else {
        return Err(AppError::NotFound);
    };
    let bytes = state
        .storage
        .read(&asset.asset_path)
        .await
        .map_err(|e| AppError::Internal(format!("读取资产失败: {e}")))?;
    release::increment_downloads(&state.pool, &asset.id).await?;
    plugin::increment_downloads(&state.pool, &asset.plugin_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    let disposition = format!("attachment; filename=\"{slug}-{version}.zip\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|e| AppError::Internal(format!("构造下载头失败: {e}")))?,
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok((StatusCode::OK, headers, Body::from(bytes)).into_response())
}

fn render_form(
    auth: &AuthCtx,
    owner: &str,
    slug: &str,
    name: &str,
    error: Option<String>,
) -> Result<Html<String>, AppError> {
    views::render(&ReleaseFormTemplate {
        common: Common::from_auth(auth, None, ""),
        owner: owner.to_string(),
        slug: slug.to_string(),
        name: name.to_string(),
        error,
    })
}

fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty() || version.len() > 32 {
        return Err("版本号长度需在 1–32 之间".to_string());
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return Err("版本号只能包含字母、数字、点、加号和连字符".to_string());
    }
    Ok(())
}
