use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, SameSite, SignedCookieJar};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::session;
use crate::error::AppError;
use crate::models::{oauth, org, user};

const STATE_COOKIE: &str = "ph_oauth_state";
const PROVIDER: &str = "github";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

#[derive(Debug, Deserialize)]
pub struct OAuthCallback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

pub async fn start(
    State(state): State<AppState>,
    jar: SignedCookieJar,
) -> Result<Response, AppError> {
    let Some(client_id) = state.config.github_client_id.as_ref() else {
        return Err(AppError::bad("GitHub OAuth 未配置"));
    };
    if state.config.github_client_secret.is_none() {
        return Err(AppError::bad("GitHub OAuth 未配置"));
    }
    let oauth_state = crate::auth::random_token(32);
    let mut cookie = Cookie::new(STATE_COOKIE, oauth_state.clone());
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(state.config.secure_cookies);
    let jar = jar.add(cookie);

    let redirect_uri = format!("{}/auth/github/callback", state.config.public_url);
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("read:user user:email"),
        urlencoding::encode(&oauth_state),
    );
    Ok((jar, Redirect::to(&url)).into_response())
}

pub async fn callback(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Query(q): Query<OAuthCallback>,
) -> Result<Response, AppError> {
    let expected = jar.get(STATE_COOKIE).map(|c| c.value().to_string());
    let jar = jar.remove(Cookie::from(STATE_COOKIE));
    if q.error.is_some() {
        return Ok((jar, Redirect::to("/login?err=oauth")).into_response());
    }
    let (Some(code), Some(state_param), Some(expected)) = (q.code, q.state, expected) else {
        return Ok((jar, Redirect::to("/login?err=oauth")).into_response());
    };
    if state_param != expected {
        return Ok((jar, Redirect::to("/login?err=oauth")).into_response());
    }
    let Some(client_id) = state.config.github_client_id.as_ref() else {
        return Err(AppError::bad("GitHub OAuth 未配置"));
    };
    let Some(client_secret) = state.config.github_client_secret.as_ref() else {
        return Err(AppError::bad("GitHub OAuth 未配置"));
    };

    let redirect_uri = format!("{}/auth/github/callback", state.config.public_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Internal(format!("构建 OAuth HTTP 客户端失败: {e}")))?;
    let body = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}",
        urlencoding::encode(client_id),
        urlencoding::encode(client_secret),
        urlencoding::encode(&code),
        urlencoding::encode(&redirect_uri),
    );
    let token_resp = client
        .post(TOKEN_URL)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, "ada-plugin-hub")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("交换 GitHub token 失败: {e}")))?;
    let token: TokenResponse = token_resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("解析 GitHub token 失败: {e}")))?;
    let Some(access_token) = token.access_token else {
        tracing::warn!(
            error = token.error.unwrap_or_default(),
            description = token.error_description.unwrap_or_default(),
            "GitHub OAuth 未返回 access_token"
        );
        return Ok((jar, Redirect::to("/login?err=oauth")).into_response());
    };

    let gh_user: GithubUser = client
        .get(format!("{}/user", state.config.github_api_base))
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "ada-plugin-hub")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("读取 GitHub 用户失败: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("解析 GitHub 用户失败: {e}")))?;

    let email = fetch_email(&client, &state, &access_token)
        .await
        .or(gh_user.email.clone())
        .filter(|e| e.contains('@'))
        .unwrap_or_else(|| format!("{}@users.noreply.github.com", gh_user.login));
    let provider_user_id = gh_user.id.to_string();

    let user = match oauth::find_user(&state.pool, PROVIDER, &provider_user_id).await? {
        Some(user) => user,
        None => match user::find_by_email(&state.pool, &email).await? {
            Some(existing) => {
                oauth::link(
                    &state.pool,
                    &existing.id,
                    PROVIDER,
                    &provider_user_id,
                    &gh_user.login,
                )
                .await?;
                existing
            }
            None => {
                let username = unique_username(&state, &gh_user.login).await?;
                let created = user::create_oauth(&state.pool, &username, &email).await?;
                oauth::link(
                    &state.pool,
                    &created.id,
                    PROVIDER,
                    &provider_user_id,
                    &gh_user.login,
                )
                .await?;
                created
            }
        },
    };
    let (jar, _) = session::start(jar, user.id, state.config.secure_cookies);
    Ok((jar, Redirect::to("/")).into_response())
}

async fn fetch_email(
    client: &reqwest::Client,
    state: &AppState,
    access_token: &str,
) -> Option<String> {
    let emails: Vec<GithubEmail> = client
        .get(format!("{}/user/emails", state.config.github_api_base))
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "ada-plugin-hub")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    emails
        .iter()
        .find(|e| e.primary && e.verified)
        .or_else(|| emails.iter().find(|e| e.verified))
        .map(|e| e.email.clone())
}

async fn unique_username(state: &AppState, login: &str) -> Result<String, AppError> {
    let base: String = login
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect::<String>()
        .to_lowercase();
    let base = if base.is_empty() {
        "gh-user".to_string()
    } else {
        base.chars().take(28).collect()
    };
    for i in 0..20 {
        let candidate = if i == 0 {
            base.clone()
        } else {
            format!("{base}-gh{i}")
        };
        let taken_by_user = user::find_by_username(&state.pool, &candidate)
            .await?
            .is_some();
        let taken_by_org = org::find_by_name(&state.pool, &candidate).await?.is_some();
        if !taken_by_user && !taken_by_org {
            return Ok(candidate);
        }
    }
    Ok(format!(
        "{base}-{}",
        crate::auth::random_token(6).to_lowercase()
    ))
}
