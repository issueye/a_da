use axum_extra::extract::cookie::{Cookie, SameSite, SignedCookieJar};

use super::random_token;

pub const COOKIE_NAME: &str = "ph_session";

#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: String,
    pub csrf: String,
}

pub fn start(jar: SignedCookieJar, user_id: String, secure: bool) -> (SignedCookieJar, Session) {
    let csrf = random_token(32);
    let session = Session {
        user_id: user_id.clone(),
        csrf: csrf.clone(),
    };
    let mut cookie = Cookie::new(COOKIE_NAME, format!("{user_id}|{csrf}"));
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(secure);
    (jar.add(cookie), session)
}

pub fn read(jar: &SignedCookieJar) -> Option<Session> {
    let cookie = jar.get(COOKIE_NAME)?;
    let value = cookie.value();
    let (user_id, csrf) = value.split_once('|')?;
    if user_id.is_empty() || csrf.is_empty() {
        return None;
    }
    Some(Session {
        user_id: user_id.to_string(),
        csrf: csrf.to_string(),
    })
}

pub fn clear(jar: SignedCookieJar) -> SignedCookieJar {
    jar.remove(Cookie::from(COOKIE_NAME))
}
