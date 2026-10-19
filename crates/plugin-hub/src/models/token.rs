use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ApiToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub prefix: String,
    pub scopes: String,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

impl ApiToken {
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
    pub fn last_used_text(&self) -> String {
        self.last_used_at
            .map(crate::views::format_ts)
            .unwrap_or_else(|| "从未使用".to_string())
    }
    pub fn scopes_list(&self) -> Vec<String> {
        self.scopes
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.split(',').any(|s| s.trim() == scope) || self.scopes.trim() == "*"
    }
    pub fn revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

pub fn generate() -> (String, String, String) {
    let raw = format!("ada_pat_{}", crate::auth::random_token(40));
    let hash = hash_token(&raw);
    let prefix = format!("{}…{}", &raw[..16], &raw[raw.len() - 4..]);
    (raw, hash, prefix)
}

pub fn hash_token(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(raw.as_bytes()))
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
    token_hash: &str,
    prefix: &str,
    scopes: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, prefix, scopes, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .bind(prefix)
    .bind(scopes)
    .bind(crate::views::now_ts())
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn list_for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<ApiToken>, sqlx::Error> {
    sqlx::query_as::<_, ApiToken>(
        "SELECT id, user_id, name, prefix, scopes, last_used_at, created_at, revoked_at
         FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn find_active_by_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<ApiToken>, sqlx::Error> {
    sqlx::query_as::<_, ApiToken>(
        "SELECT id, user_id, name, prefix, scopes, last_used_at, created_at, revoked_at
         FROM api_tokens WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
}

pub async fn touch(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
        .bind(crate::views::now_ts())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke(pool: &SqlitePool, user_id: &str, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(crate::views::now_ts())
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
