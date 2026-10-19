pub mod password;
pub mod session;

use axum::extract::FromRef;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Key, SignedCookieJar};
use rand::Rng;
use rand::distributions::Alphanumeric;

use crate::app::AppState;
use crate::error::AppError;
use crate::models::User;

pub use session::Session;

/// 每个请求统一解析：签名 Cookie → Session → User。
pub struct AuthCtx {
    pub jar: SignedCookieJar,
    pub session: Option<Session>,
    pub user: Option<User>,
}

impl AuthCtx {
    pub fn csrf(&self) -> &str {
        self.session.as_ref().map(|s| s.csrf.as_str()).unwrap_or("")
    }

    pub fn require_user(&self) -> Result<&User, AppError> {
        self.user
            .as_ref()
            .ok_or_else(|| AppError::Redirect("/login".to_string()))
    }

    pub fn validate_csrf(&self, token: &str) -> Result<(), AppError> {
        let Some(session) = &self.session else {
            return Err(AppError::Redirect("/login".to_string()));
        };
        if token.is_empty() || token != session.csrf {
            return Err(AppError::bad("CSRF 校验失败，请刷新页面后重试"));
        }
        Ok(())
    }
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.key.clone()
    }
}

impl FromRequestParts<AppState> for AuthCtx {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = SignedCookieJar::from_request_parts(parts, state)
            .await
            .map_err(|e| AppError::Internal(format!("读取会话 Cookie 失败: {e}")))?;
        let session = session::read(&jar);
        let user = match &session {
            Some(s) => crate::models::user::find_by_id(&state.pool, &s.user_id).await?,
            None => None,
        };
        Ok(Self { jar, session, user })
    }
}

pub fn random_token(bytes: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..bytes)
        .map(|_| rng.sample(Alphanumeric) as char)
        .collect()
}
