use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::User;
use crate::models::org;
use crate::models::plugin::{self, PluginFormData};
use crate::models::release;
use crate::models::star;
use crate::views::{self, Common, PluginFormTemplate, PluginTemplate};

use super::PageQuery;
use super::auth::is_unique;

#[derive(Debug, Deserialize)]
pub struct PluginForm {
    pub _csrf: String,
    #[serde(default)]
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
    #[serde(default)]
    pub org_id: String,
}

pub async fn new_form(
    State(state): State<AppState>,
    auth: AuthCtx,
) -> Result<Html<String>, AppError> {
    let user = auth.require_user()?;
    let orgs = org::for_user(&state.pool, &user.id).await?;
    views::render(&PluginFormTemplate {
        common: Common::from_auth(&auth, None, ""),
        form: PluginFormData::default(),
        orgs,
        action: "/plugins".to_string(),
        heading: "新建插件".to_string(),
        submit: "创建插件".to_string(),
        error: None,
        sync_action: String::new(),
    })
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthCtx,
    Form(form): Form<PluginForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let form_data = PluginFormData {
        name: form.name.trim().to_string(),
        description: form.description.trim().to_string(),
        readme: form.readme.clone(),
        homepage: form.homepage.trim().to_string(),
        license: form.license.trim().to_string(),
        tags: form.tags.clone(),
    };
    let slug = form.slug.trim().to_lowercase();
    if let Err(error) = validate(&slug, &form_data) {
        return render_form(&auth, form_data, "new", Some(error)).map(IntoResponse::into_response);
    }
    let tags = plugin::normalize_tags(&form.tags);
    let (owner_id, org_id, owner_name) = if form.org_id.trim().is_empty() {
        (Some(user.id.clone()), None, user.username.clone())
    } else {
        let Some(target) = org::find_by_name(&state.pool, form.org_id.trim()).await? else {
            return render_form(&auth, form_data, "new", Some("组织不存在".to_string()))
                .map(IntoResponse::into_response);
        };
        let role = org::member_role(&state.pool, &target.id, &user.id).await?;
        if role.is_none() {
            return render_form(
                &auth,
                form_data,
                "new",
                Some("你不是该组织成员".to_string()),
            )
            .map(IntoResponse::into_response);
        }
        (None, Some(target.id), target.name)
    };
    match plugin::create(
        &state.pool,
        owner_id.as_deref(),
        org_id.as_deref(),
        &slug,
        &form_data,
        &tags,
    )
    .await
    {
        Ok(id) => {
            crate::models::webhook::dispatch_plugin(
                &state.pool,
                owner_id.as_deref(),
                org_id.as_deref(),
                "plugin.created",
                serde_json::json!({
                    "plugin": { "id": id, "owner": owner_name.clone(), "slug": slug.clone() }
                }),
            )
            .await;
            Ok(Redirect::to(&format!("/plugins/{owner_name}/{slug}?ok=created")).into_response())
        }
        Err(err) if is_unique(&err) => render_form(
            &auth,
            form_data,
            "new",
            Some("该所有者下已经有同名插件".to_string()),
        )
        .map(IntoResponse::into_response),
        Err(err) => Err(err.into()),
    }
}

pub async fn detail(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    let releases = release::list_for_plugin(&state.pool, &detail.id).await?;
    let starred = match &auth.user {
        Some(user) => star::is_starred(&state.pool, &user.id, &detail.id).await?,
        None => false,
    };
    let can_edit = match &auth.user {
        Some(user) => can_manage(&state.pool, user, &detail).await?,
        None => false,
    };
    let readme_html = crate::markdown::render(&detail.readme);
    let stars = detail.stars;
    views::render(&PluginTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        plugin: detail,
        releases,
        starred,
        stars,
        can_edit,
        readme_html,
    })
}

pub async fn edit_form(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }
    views::render(&PluginFormTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        form: PluginFormData::from(&detail),
        orgs: Vec::new(),
        action: format!("/plugins/{owner}/{slug}"),
        heading: format!("编辑 {}", detail.name),
        submit: "保存修改".to_string(),
        error: None,
        sync_action: format!("/plugins/{owner}/{slug}/sync-package"),
    })
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    Form(form): Form<PluginForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }
    let form_data = PluginFormData {
        name: form.name.trim().to_string(),
        description: form.description.trim().to_string(),
        readme: form.readme.clone(),
        homepage: form.homepage.trim().to_string(),
        license: form.license.trim().to_string(),
        tags: form.tags.clone(),
    };
    if let Err(error) = validate(&slug, &form_data) {
        return views::render(&PluginFormTemplate {
            common: Common::from_auth(&auth, None, ""),
            form: form_data,
            orgs: Vec::new(),
            action: format!("/plugins/{owner}/{slug}"),
            heading: format!("编辑 {}", detail.name),
            submit: "保存修改".to_string(),
            error: Some(error),
            sync_action: format!("/plugins/{owner}/{slug}/sync-package"),
        })
        .map(IntoResponse::into_response);
    }
    let tags = plugin::normalize_tags(&form.tags);
    plugin::update(&state.pool, &detail.id, &form_data, &tags).await?;
    crate::models::webhook::dispatch_plugin(
        &state.pool,
        detail.owner_id.as_deref(),
        detail.org_id.as_deref(),
        "plugin.updated",
        serde_json::json!({
            "plugin": { "id": detail.id, "owner": detail.owner, "slug": detail.slug }
        }),
    )
    .await;
    Ok(Redirect::to(&format!("/plugins/{owner}/{slug}?ok=updated")).into_response())
}

pub async fn sync_package(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    Form(form): Form<super::CsrfForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }

    let releases = release::list_for_plugin(&state.pool, &detail.id).await?;
    let Some(latest) = releases.first() else {
        return Ok(
            Redirect::to(&format!("/plugins/{owner}/{slug}/edit?err=no_release"))
                .into_response(),
        );
    };
    let asset_path = latest.asset_path.clone();
    let bytes = state
        .storage
        .read(&asset_path)
        .await
        .map_err(|e| AppError::Internal(format!("读取最新版本插件包失败: {e}")))?;
    let inspection = crate::metadata::inspect_package(&bytes).map_err(AppError::bad)?;
    plugin::sync_package_text(
        &state.pool,
        &detail.id,
        &inspection.metadata.description,
        inspection.readme.as_deref(),
    )
    .await?;
    crate::models::webhook::dispatch_plugin(
        &state.pool,
        detail.owner_id.as_deref(),
        detail.org_id.as_deref(),
        "plugin.updated",
        serde_json::json!({
            "plugin": { "id": detail.id, "owner": detail.owner, "slug": detail.slug }
        }),
    )
    .await;
    Ok(Redirect::to(&format!("/plugins/{owner}/{slug}/edit?ok=synced")).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((owner, slug)): Path<(String, String)>,
    Form(form): Form<super::CsrfForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let Some(detail) = plugin::find_detail(&state.pool, &owner, &slug).await? else {
        return Err(AppError::NotFound);
    };
    if !can_manage(&state.pool, user, &detail).await? {
        return Err(AppError::Forbidden);
    }
    plugin::delete(&state.pool, &detail.id).await?;
    state.storage.remove_plugin_dir(&owner, &slug).await;
    Ok(Redirect::to("/plugins?ok=deleted").into_response())
}

pub(crate) async fn can_manage(
    pool: &sqlx::SqlitePool,
    user: &User,
    detail: &plugin::PluginDetail,
) -> Result<bool, sqlx::Error> {
    if detail.owner_id.as_deref() == Some(user.id.as_str()) {
        return Ok(true);
    }
    if let Some(org_id) = detail.org_id.as_deref() {
        let role = org::member_role(pool, org_id, &user.id).await?;
        return Ok(org::can_manage(role.as_deref()));
    }
    Ok(false)
}

fn render_form(
    auth: &AuthCtx,
    form: PluginFormData,
    mode: &str,
    error: Option<String>,
) -> Result<Html<String>, AppError> {
    let is_edit = mode == "edit";
    views::render(&PluginFormTemplate {
        common: Common::from_auth(auth, None, ""),
        form,
        orgs: Vec::new(),
        action: "/plugins".to_string(),
        heading: if is_edit {
            "编辑插件"
        } else {
            "新建插件"
        }
        .to_string(),
        submit: if is_edit {
            "保存修改"
        } else {
            "创建插件"
        }
        .to_string(),
        error,
        sync_action: String::new(),
    })
}

fn validate(slug: &str, form: &PluginFormData) -> Result<(), String> {
    if slug.len() < 2 || slug.len() > 64 {
        return Err("slug 长度需在 2–64 之间".to_string());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || slug.starts_with('-')
        || slug.ends_with('-')
    {
        return Err("slug 只能包含小写字母、数字和连字符，且不能以连字符开头/结尾".to_string());
    }
    if form.name.is_empty() || form.name.len() > 80 {
        return Err("名称长度需在 1–80 之间".to_string());
    }
    if form.description.len() > 300 {
        return Err("简介不能超过 300 字".to_string());
    }
    if form.readme.len() > 200_000 {
        return Err("README 不能超过 200 KB".to_string());
    }
    if !form.homepage.is_empty()
        && !(form.homepage.starts_with("http://") || form.homepage.starts_with("https://"))
    {
        return Err("主页地址必须以 http:// 或 https:// 开头".to_string());
    }
    Ok(())
}
