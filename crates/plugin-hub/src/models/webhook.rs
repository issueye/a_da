use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use super::org;

#[derive(Debug, Clone, FromRow)]
pub struct Webhook {
    pub id: String,
    pub user_id: String,
    pub url: String,
    pub secret: String,
    pub events: String,
    pub active: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct Delivery {
    pub id: String,
    pub webhook_id: String,
    pub event: String,
    pub status_code: Option<i64>,
    pub ok: i64,
    pub error: Option<String>,
    pub created_at: i64,
}

impl Webhook {
    pub fn events_list(&self) -> Vec<String> {
        self.events
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
    pub fn matches(&self, event: &str) -> bool {
        self.events.trim() == "*" || self.events_list().iter().any(|e| e == event)
    }
}

impl Delivery {
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    url: &str,
    events: &str,
) -> Result<Webhook, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let secret = crate::auth::random_token(32);
    let now = crate::views::now_ts();
    sqlx::query(
        "INSERT INTO webhooks (id, user_id, url, secret, events, active, created_at)
         VALUES (?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(url)
    .bind(&secret)
    .bind(events)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Webhook {
        id,
        user_id: user_id.to_string(),
        url: url.to_string(),
        secret,
        events: events.to_string(),
        active: 1,
        created_at: now,
    })
}

pub async fn list_for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<Webhook>, sqlx::Error> {
    sqlx::query_as::<_, Webhook>(
        "SELECT id, user_id, url, secret, events, active, created_at
         FROM webhooks WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM webhooks WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn deliveries(
    pool: &SqlitePool,
    webhook_id: &str,
    limit: i64,
) -> Result<Vec<Delivery>, sqlx::Error> {
    sqlx::query_as::<_, Delivery>(
        "SELECT id, webhook_id, event, status_code, ok, error, created_at
         FROM webhook_deliveries WHERE webhook_id = ? ORDER BY created_at DESC LIMIT ?",
    )
    .bind(webhook_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn recent_deliveries(
    pool: &SqlitePool,
    user_id: &str,
    limit: i64,
) -> Result<Vec<Delivery>, sqlx::Error> {
    sqlx::query_as::<_, Delivery>(
        "SELECT d.id, d.webhook_id, d.event, d.status_code, d.ok, d.error, d.created_at
         FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id
         WHERE w.user_id = ? ORDER BY d.created_at DESC LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 触发用户的所有匹配 webhook；不阻塞请求，投递结果落 `webhook_deliveries`。
/// 按插件所有者派发：个人插件直接找 owner；组织插件找组织 owner。
pub async fn dispatch_plugin(
    pool: &SqlitePool,
    owner_id: Option<&str>,
    org_id: Option<&str>,
    event: &'static str,
    payload: serde_json::Value,
) {
    let user_id = match (owner_id, org_id) {
        (Some(id), _) => Some(id.to_string()),
        (None, Some(org_id)) => org::owner_id(pool, org_id).await.ok().flatten(),
        _ => None,
    };
    if let Some(user_id) = user_id {
        dispatch(pool.clone(), user_id, event, payload);
    }
}

pub fn dispatch(
    pool: SqlitePool,
    user_id: String,
    event: &'static str,
    payload: serde_json::Value,
) {
    tokio::spawn(async move {
        let hooks = match sqlx::query_as::<_, Webhook>(
            "SELECT id, user_id, url, secret, events, active, created_at
             FROM webhooks WHERE user_id = ? AND active = 1",
        )
        .bind(&user_id)
        .fetch_all(&pool)
        .await
        {
            Ok(hooks) => hooks,
            Err(err) => {
                tracing::warn!(error = %err, "读取 webhook 失败");
                return;
            }
        };

        for hook in hooks {
            if !hook.matches(event) {
                continue;
            }
            let pool = pool.clone();
            let body = payload.to_string();
            let signature = sign(&hook.secret, &body);
            tokio::spawn(async move {
                let client = reqwest::Client::new();
                let result = client
                    .post(&hook.url)
                    .timeout(std::time::Duration::from_secs(8))
                    .header(reqwest::header::USER_AGENT, "ada-plugin-hub")
                    .header("X-Plugin-Hub-Event", event)
                    .header("X-Plugin-Hub-Signature-256", signature)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .send()
                    .await;

                let (status, ok, error) = match result {
                    Ok(resp) => (
                        Some(resp.status().as_u16() as i64),
                        resp.status().is_success(),
                        None,
                    ),
                    Err(err) => (None, false, Some(err.to_string())),
                };
                let _ = sqlx::query(
                    "INSERT INTO webhook_deliveries (id, webhook_id, event, status_code, ok, error, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(Uuid::new_v4().to_string())
                .bind(&hook.id)
                .bind(event)
                .bind(status)
                .bind(ok as i64)
                .bind(error)
                .bind(crate::views::now_ts())
                .execute(&pool)
                .await;
            });
        }
    });
}

pub fn sign(secret: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC 接受任意长度 key");
    mac.update(payload.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}
