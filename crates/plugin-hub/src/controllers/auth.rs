use axum::Form;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::{AuthCtx, password, session};
use crate::error::AppError;
use crate::models::user;
use crate::views::{self, AuthTemplate, Common};

use super::{CsrfForm, PageQuery, safe_next};

#[derive(Debug, Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub email: String,
    pub password: String,
    pub password2: String,
    pub next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub login: String,
    pub password: String,
    pub next: Option<String>,
}

pub async fn register_form(
    State(state): State<AppState>,
    auth: AuthCtx,
    axum::extract::Query(q): axum::extract::Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    views::render(&AuthTemplate {
        common: Common::from_auth(&auth, None, ""),
        heading: "创建账号".to_string(),
        action: "/register".to_string(),
        submit: "注册".to_string(),
        next: q.next.unwrap_or_default(),
        github_enabled: state.config.github_enabled(),
        error: q.err.as_deref().map(oauth_error_message),
    })
}

pub async fn register(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    let auth = AuthCtx {
        jar: jar.clone(),
        session: None,
        user: None,
    };
    let error = validate_register(&form);
    if let Some(error) = error {
        return views::render(&AuthTemplate {
            common: Common::from_auth(&auth, None, ""),
            heading: "创建账号".to_string(),
            action: "/register".to_string(),
            submit: "注册".to_string(),
            next: form.next.clone().unwrap_or_default(),
            github_enabled: state.config.github_enabled(),
            error: Some(error),
        })
        .map(IntoResponse::into_response);
    }

    let hash = password::hash_password(&form.password).map_err(|e| AppError::Internal(e))?;
    if crate::models::org::find_by_name(&state.pool, form.username.trim())
        .await?
        .is_some()
    {
        return views::render(&AuthTemplate {
            common: Common::from_auth(&auth, None, ""),
            heading: "创建账号".to_string(),
            action: "/register".to_string(),
            submit: "注册".to_string(),
            next: form.next.clone().unwrap_or_default(),
            github_enabled: state.config.github_enabled(),
            error: Some("这个用户名已被组织占用".to_string()),
        })
        .map(IntoResponse::into_response);
    }
    match user::create(&state.pool, form.username.trim(), form.email.trim(), &hash).await {
        Ok(user) => {
            let (jar, _) = session::start(jar, user.id, state.config.secure_cookies);
            Ok((jar, Redirect::to(&safe_next(form.next.as_deref()))).into_response())
        }
        Err(err) if is_unique(&err) => views::render(&AuthTemplate {
            common: Common::from_auth(&auth, None, ""),
            heading: "创建账号".to_string(),
            action: "/register".to_string(),
            submit: "注册".to_string(),
            next: form.next.clone().unwrap_or_default(),
            github_enabled: state.config.github_enabled(),
            error: Some("用户名或邮箱已被使用".to_string()),
        })
        .map(IntoResponse::into_response),
        Err(err) => Err(err.into()),
    }
}

pub async fn login_form(
    State(state): State<AppState>,
    auth: AuthCtx,
    axum::extract::Query(q): axum::extract::Query<PageQuery>,
) -> Result<Html<String>, AppError> {
    views::render(&AuthTemplate {
        common: Common::from_auth(&auth, None, ""),
        heading: "登录".to_string(),
        action: "/login".to_string(),
        submit: "登录".to_string(),
        next: q.next.unwrap_or_default(),
        github_enabled: state.config.github_enabled(),
        error: q.err.as_deref().map(oauth_error_message),
    })
}

pub async fn login(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let auth = AuthCtx {
        jar: jar.clone(),
        session: None,
        user: None,
    };
    let found = user::find_by_login(&state.pool, form.login.trim()).await?;
    let Some(user) = found else {
        return views::render(&AuthTemplate {
            common: Common::from_auth(&auth, None, ""),
            heading: "登录".to_string(),
            action: "/login".to_string(),
            submit: "登录".to_string(),
            next: form.next.clone().unwrap_or_default(),
            github_enabled: state.config.github_enabled(),
            error: Some("用户名或密码错误".to_string()),
        })
        .map(IntoResponse::into_response);
    };
    if !password::verify_password(&form.password, &user.password_hash) {
        return views::render(&AuthTemplate {
            common: Common::from_auth(&auth, None, ""),
            heading: "登录".to_string(),
            action: "/login".to_string(),
            submit: "登录".to_string(),
            next: form.next.clone().unwrap_or_default(),
            github_enabled: state.config.github_enabled(),
            error: Some("用户名或密码错误".to_string()),
        })
        .map(IntoResponse::into_response);
    }
    let (jar, _) = session::start(jar, user.id, state.config.secure_cookies);
    Ok((jar, Redirect::to(&safe_next(form.next.as_deref()))).into_response())
}

pub async fn logout(auth: AuthCtx, Form(form): Form<CsrfForm>) -> Result<Response, AppError> {
    auth.validate_csrf(&form._csrf)?;
    Ok((session::clear(auth.jar), Redirect::to("/")).into_response())
}

fn validate_register(form: &RegisterForm) -> Option<String> {
    let username = form.username.trim();
    if username.len() < 3 || username.len() > 32 {
        return Some("用户名长度需在 3–32 之间".to_string());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Some("用户名只能包含字母、数字、下划线和连字符".to_string());
    }
    let email = form.email.trim();
    if email.len() < 5 || email.len() > 120 || !email.contains('@') {
        return Some("请输入合法邮箱".to_string());
    }
    if form.password.len() < 8 || form.password.len() > 128 {
        return Some("密码长度需在 8–128 之间".to_string());
    }
    if form.password != form.password2 {
        return Some("两次输入的密码不一致".to_string());
    }
    None
}

pub fn is_unique(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|e| e.is_unique_violation())
        .unwrap_or(false)
}

fn oauth_error_message(code: &str) -> String {
    match code {
        "oauth" => "GitHub 登录失败或已取消，请重试".to_string(),
        _ => "操作失败，请重试".to_string(),
    }
}
