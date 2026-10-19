use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::AuthCtx;
use crate::error::AppError;
use crate::models::org;
use crate::models::{plugin, user};
use crate::views::{self, Common, OrgNewTemplate, OrgTemplate, OrgsTemplate, TeamView};

use super::{CsrfForm, PageQuery};

#[derive(Debug, Deserialize)]
pub struct OrgForm {
    pub _csrf: String,
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct OrgMemberForm {
    pub _csrf: String,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct TeamForm {
    pub _csrf: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct TeamMemberForm {
    pub _csrf: String,
    pub username: String,
    #[serde(default = "default_team_role")]
    pub role: String,
}

fn default_team_role() -> String {
    "member".to_string()
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthCtx,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let orgs = org::list_all(&state.pool).await?;
    views::render(&OrgsTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        orgs,
    })
}

pub async fn new_form(
    State(_state): State<AppState>,
    auth: AuthCtx,
) -> Result<Html<String>, AppError> {
    auth.require_user()?;
    views::render(&OrgNewTemplate {
        common: Common::from_auth(&auth, None, ""),
        error: None,
    })
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthCtx,
    Form(form): Form<OrgForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let name = form.name.trim().to_lowercase();
    if let Err(error) = validate_org_name(&name) {
        return render_new(&auth, Some(error)).map(IntoResponse::into_response);
    }
    if user::find_by_username(&state.pool, &name).await?.is_some()
        || org::find_by_name(&state.pool, &name).await?.is_some()
    {
        return render_new(&auth, Some("这个名称已被用户或组织占用".to_string()))
            .map(IntoResponse::into_response);
    }
    if form.display_name.trim().is_empty() || form.display_name.len() > 80 {
        return render_new(&auth, Some("显示名称长度需在 1–80 之间".to_string()))
            .map(IntoResponse::into_response);
    }
    org::create(
        &state.pool,
        &name,
        form.display_name.trim(),
        form.description.trim(),
        &user.id,
    )
    .await?;
    Ok(Redirect::to(&format!("/orgs/{name}?ok=created")).into_response())
}

pub async fn detail(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(name): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    let Some(org) = org::find_by_name(&state.pool, &name).await? else {
        return Err(AppError::NotFound);
    };
    let plugins = plugin::by_org(&state.pool, &org.name).await?;
    let members = org::members(&state.pool, &org.id).await?;
    let teams = org::teams(&state.pool, &org.id).await?;
    let mut team_views = Vec::new();
    for team in teams {
        let team_members = org::team_members(&state.pool, &team.id).await?;
        team_views.push(TeamView {
            team,
            members: team_members,
        });
    }
    let my_role = match &auth.user {
        Some(user) => org::member_role(&state.pool, &org.id, &user.id)
            .await?
            .unwrap_or_default(),
        None => String::new(),
    };
    let can_manage = org::can_manage(if my_role.is_empty() {
        None
    } else {
        Some(my_role.as_str())
    });
    views::render(&OrgTemplate {
        common: Common::from_auth(
            &auth,
            views::Flash::from_query(q.ok.as_deref(), q.err.as_deref()),
            "",
        ),
        org,
        plugins,
        members,
        teams: team_views,
        can_manage,
        my_role,
    })
}

pub async fn add_member(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(name): Path<String>,
    Form(form): Form<OrgMemberForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let org = require_org(&state, &name).await?;
    let my_role = org::member_role(&state.pool, &org.id, &user.id).await?;
    if !org::can_manage(my_role.as_deref()) {
        return Err(AppError::Forbidden);
    }
    let role = form.role.trim();
    if !matches!(role, "owner" | "admin" | "member") {
        return Err(AppError::bad("角色只能是 owner/admin/member"));
    }
    if role == "owner" && my_role.as_deref() != Some("owner") {
        return Err(AppError::Forbidden);
    }
    let Some(target) = user::find_by_username(&state.pool, form.username.trim()).await? else {
        return Err(AppError::bad("用户不存在"));
    };
    if target.id == org.owner_id && role != "owner" {
        return Err(AppError::bad("不能修改组织所有者的角色"));
    }
    org::add_member(&state.pool, &org.id, &target.id, role).await?;
    Ok(Redirect::to(&format!("/orgs/{}?ok=member_added", org.name)).into_response())
}

pub async fn remove_member(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((name, member_id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let org = require_org(&state, &name).await?;
    let my_role = org::member_role(&state.pool, &org.id, &user.id).await?;
    if !org::can_manage(my_role.as_deref()) {
        return Err(AppError::Forbidden);
    }
    if member_id == org.owner_id {
        return Err(AppError::bad("不能移除组织所有者"));
    }
    org::remove_member(&state.pool, &org.id, &member_id).await?;
    Ok(Redirect::to(&format!("/orgs/{}?ok=member_removed", org.name)).into_response())
}

pub async fn create_team(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path(name): Path<String>,
    Form(form): Form<TeamForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let org = require_org(&state, &name).await?;
    let my_role = org::member_role(&state.pool, &org.id, &user.id).await?;
    if !org::can_manage(my_role.as_deref()) {
        return Err(AppError::Forbidden);
    }
    let team_name = form.name.trim().to_lowercase();
    if team_name.len() < 2 || team_name.len() > 40 {
        return Err(AppError::bad("团队名称长度需在 2–40 之间"));
    }
    org::create_team(&state.pool, &org.id, &team_name, form.description.trim()).await?;
    Ok(Redirect::to(&format!("/orgs/{}?ok=team_created", org.name)).into_response())
}

pub async fn add_team_member(
    State(state): State<AppState>,
    auth: AuthCtx,
    Path((name, team_id)): Path<(String, String)>,
    Form(form): Form<TeamMemberForm>,
) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    let user = auth.require_user()?;
    let org = require_org(&state, &name).await?;
    let my_role = org::member_role(&state.pool, &org.id, &user.id).await?;
    if !org::can_manage(my_role.as_deref()) {
        return Err(AppError::Forbidden);
    }
    let Some(target) = user::find_by_username(&state.pool, form.username.trim()).await? else {
        return Err(AppError::bad("用户不存在"));
    };
    if org::member_role(&state.pool, &org.id, &target.id)
        .await?
        .is_none()
    {
        return Err(AppError::bad("只能把组织成员加入团队"));
    }
    let role = if form.role == "maintainer" {
        "maintainer"
    } else {
        "member"
    };
    org::add_team_member(&state.pool, &team_id, &target.id, role).await?;
    Ok(Redirect::to(&format!("/orgs/{}?ok=team_member_added", org.name)).into_response())
}

fn validate_org_name(name: &str) -> Result<(), String> {
    if name.len() < 2 || name.len() > 40 {
        return Err("组织名长度需在 2–40 之间".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || name.starts_with('-')
        || name.ends_with('-')
    {
        return Err("组织名只能包含小写字母、数字和连字符，且不能以连字符开头/结尾".to_string());
    }
    Ok(())
}

fn render_new(auth: &AuthCtx, error: Option<String>) -> Result<Html<String>, AppError> {
    views::render(&OrgNewTemplate {
        common: Common::from_auth(auth, None, ""),
        error,
    })
}

async fn require_org(state: &AppState, name: &str) -> Result<org::Org, AppError> {
    org::find_by_name(&state.pool, name)
        .await?
        .ok_or(AppError::NotFound)
}
