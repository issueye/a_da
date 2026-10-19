use sqlx::SqlitePool;

use super::user::User;

pub async fn find_user(
    pool: &SqlitePool,
    provider: &str,
    provider_user_id: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT u.id, u.username, u.email, u.password_hash, u.created_at
         FROM oauth_accounts a JOIN users u ON u.id = a.user_id
         WHERE a.provider = ? AND a.provider_user_id = ?",
    )
    .bind(provider)
    .bind(provider_user_id)
    .fetch_optional(pool)
    .await
}

pub async fn link(
    pool: &SqlitePool,
    user_id: &str,
    provider: &str,
    provider_user_id: &str,
    login: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_accounts (user_id, provider, provider_user_id, login, created_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(provider, provider_user_id) DO UPDATE SET login = excluded.login",
    )
    .bind(user_id)
    .bind(provider)
    .bind(provider_user_id)
    .bind(login)
    .bind(crate::views::now_ts())
    .execute(pool)
    .await?;
    Ok(())
}
