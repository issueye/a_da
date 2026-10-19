use sqlx::SqlitePool;

pub async fn is_starred(
    pool: &SqlitePool,
    user_id: &str,
    plugin_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM stars WHERE user_id = ? AND plugin_id = ?")
            .bind(user_id)
            .bind(plugin_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// 返回 `(是否已 Star, Star 总数)`。
pub async fn toggle(
    pool: &SqlitePool,
    user_id: &str,
    plugin_id: &str,
) -> Result<(bool, i64), sqlx::Error> {
    let starred = is_starred(pool, user_id, plugin_id).await?;
    if starred {
        sqlx::query("DELETE FROM stars WHERE user_id = ? AND plugin_id = ?")
            .bind(user_id)
            .bind(plugin_id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query(
            "INSERT OR IGNORE INTO stars (user_id, plugin_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(user_id)
        .bind(plugin_id)
        .bind(crate::views::now_ts())
        .execute(pool)
        .await?;
    }
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM stars WHERE plugin_id = ?")
        .bind(plugin_id)
        .fetch_one(pool)
        .await?;
    Ok((!starred, n))
}

pub async fn count_for_plugin(pool: &SqlitePool, plugin_id: &str) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM stars WHERE plugin_id = ?")
        .bind(plugin_id)
        .fetch_one(pool)
        .await?;
    Ok(n)
}
