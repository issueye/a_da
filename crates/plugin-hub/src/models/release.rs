use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Release {
    pub id: String,
    pub plugin_id: String,
    pub version: String,
    pub notes: String,
    pub asset_path: String,
    pub asset_size: i64,
    pub sha256: String,
    pub downloads: i64,
    pub created_at: i64,
    /// 包根 `index.ts` 的 metadata（JSON 字符串，发布时已校验必填字段）。
    pub metadata_json: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ReleaseAsset {
    pub id: String,
    pub plugin_id: String,
    pub owner: String,
    pub slug: String,
    pub version: String,
    pub asset_path: String,
    pub asset_size: i64,
    pub sha256: String,
}

impl Release {
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
    pub fn size_text(&self) -> String {
        format_size(self.asset_size)
    }

    pub fn metadata(&self) -> serde_json::Value {
        serde_json::from_str(&self.metadata_json).unwrap_or_else(|_| serde_json::json!({}))
    }

    fn meta_text(&self, key: &str) -> String {
        self.metadata()
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub fn meta_name(&self) -> String {
        self.meta_text("name")
    }
    pub fn meta_description(&self) -> String {
        self.meta_text("description")
    }
    pub fn meta_author(&self) -> String {
        self.meta_text("author")
    }
    pub fn meta_version(&self) -> String {
        self.meta_text("version")
    }
}

pub fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    }
}

pub async fn create(
    pool: &SqlitePool,
    plugin_id: &str,
    version: &str,
    notes: &str,
    asset_path: &str,
    asset_size: i64,
    sha256: &str,
    metadata_json: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let now = crate::views::now_ts();
    sqlx::query(
        "INSERT INTO releases (id, plugin_id, version, notes, asset_path, asset_size, sha256, downloads, created_at, metadata_json)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(&id)
    .bind(plugin_id)
    .bind(version)
    .bind(notes)
    .bind(asset_path)
    .bind(asset_size)
    .bind(sha256)
    .bind(now)
    .bind(metadata_json)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn list_for_plugin(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<Vec<Release>, sqlx::Error> {
    sqlx::query_as::<_, Release>(
        "SELECT id, plugin_id, version, notes, asset_path, asset_size, sha256, downloads, created_at, metadata_json
         FROM releases WHERE plugin_id = ? ORDER BY created_at DESC, rowid DESC",
    )
    .bind(plugin_id)
    .fetch_all(pool)
    .await
}

pub async fn find_asset(
    pool: &SqlitePool,
    owner: &str,
    slug: &str,
    version: &str,
) -> Result<Option<ReleaseAsset>, sqlx::Error> {
    sqlx::query_as::<_, ReleaseAsset>(
        "SELECT r.id, r.plugin_id, COALESCE(o.name, u.username) AS owner, p.slug, r.version, r.asset_path, r.asset_size, r.sha256
         FROM releases r
         JOIN plugins p ON p.id = r.plugin_id
         LEFT JOIN users u ON u.id = p.owner_id
         LEFT JOIN orgs o ON o.id = p.org_id
         WHERE COALESCE(o.name, u.username) = ? AND p.slug = ? AND r.version = ?",
    )
    .bind(owner)
    .bind(slug)
    .bind(version)
    .fetch_optional(pool)
    .await
}

pub async fn increment_downloads(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE releases SET downloads = downloads + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn count_all(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM releases")
        .fetch_one(pool)
        .await?;
    Ok(n)
}
