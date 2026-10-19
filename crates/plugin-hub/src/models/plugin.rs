use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct PluginCard {
    pub id: String,
    pub owner: String,
    pub owner_kind: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub tags: String,
    pub downloads: i64,
    pub stars: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct PluginDetail {
    pub id: String,
    pub owner_id: Option<String>,
    pub org_id: Option<String>,
    pub owner: String,
    pub owner_kind: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub readme: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub tags: String,
    pub downloads: i64,
    pub stars: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct PluginFormData {
    pub name: String,
    pub description: String,
    pub readme: String,
    pub homepage: String,
    pub license: String,
    pub tags: String,
}

impl From<&PluginDetail> for PluginFormData {
    fn from(p: &PluginDetail) -> Self {
        Self {
            name: p.name.clone(),
            description: p.description.clone(),
            readme: p.readme.clone(),
            homepage: p.homepage.clone().unwrap_or_default(),
            license: p.license.clone().unwrap_or_default(),
            tags: p.tags.clone(),
        }
    }
}

impl PluginCard {
    pub fn tags_list(&self) -> Vec<String> {
        split_tags(&self.tags)
    }
    pub fn updated_text(&self) -> String {
        crate::views::format_ts(self.updated_at)
    }
    pub fn owner_href(&self) -> String {
        if self.owner_kind == "org" {
            format!("/orgs/{}", self.owner)
        } else {
            format!("/u/{}", self.owner)
        }
    }
}

impl PluginDetail {
    pub fn tags_list(&self) -> Vec<String> {
        split_tags(&self.tags)
    }
    pub fn updated_text(&self) -> String {
        crate::views::format_ts(self.updated_at)
    }
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
    pub fn owner_href(&self) -> String {
        if self.owner_kind == "org" {
            format!("/orgs/{}", self.owner)
        } else {
            format!("/u/{}", self.owner)
        }
    }
}

pub fn normalize_tags(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for tag in raw.split(',') {
        let tag = tag.trim().trim_start_matches('#').to_lowercase();
        if tag.is_empty() || tag.len() > 24 {
            continue;
        }
        if !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            continue;
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
        if out.len() >= 10 {
            break;
        }
    }
    out.join(",")
}

fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub async fn create(
    pool: &SqlitePool,
    owner_id: Option<&str>,
    org_id: Option<&str>,
    slug: &str,
    form: &PluginFormData,
    tags: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let now = crate::views::now_ts();
    sqlx::query(
        "INSERT INTO plugins
         (id, owner_id, org_id, slug, name, description, readme, homepage, license, tags, visibility, downloads, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'public', 0, ?, ?)",
    )
    .bind(&id)
    .bind(owner_id)
    .bind(org_id)
    .bind(slug)
    .bind(&form.name)
    .bind(&form.description)
    .bind(&form.readme)
    .bind(empty_to_none(&form.homepage))
    .bind(empty_to_none(&form.license))
    .bind(tags)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn update(
    pool: &SqlitePool,
    id: &str,
    form: &PluginFormData,
    tags: &str,
) -> Result<(), sqlx::Error> {
    let now = crate::views::now_ts();
    sqlx::query(
        "UPDATE plugins SET name = ?, description = ?, readme = ?, homepage = ?, license = ?, tags = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&form.name)
    .bind(&form.description)
    .bind(&form.readme)
    .bind(empty_to_none(&form.homepage))
    .bind(empty_to_none(&form.license))
    .bind(tags)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

const DETAIL_SELECT: &str = "SELECT p.id, p.owner_id, p.org_id,
        COALESCE(o.name, u.username) AS owner,
        CASE WHEN p.org_id IS NULL THEN 'user' ELSE 'org' END AS owner_kind,
        p.slug, p.name, p.description, p.readme, p.homepage, p.license, p.tags,
        p.downloads,
        (SELECT COUNT(*) FROM stars s WHERE s.plugin_id = p.id) AS stars,
        p.created_at, p.updated_at
 FROM plugins p
 LEFT JOIN users u ON u.id = p.owner_id
 LEFT JOIN orgs o ON o.id = p.org_id";

pub async fn find_detail(
    pool: &SqlitePool,
    owner: &str,
    slug: &str,
) -> Result<Option<PluginDetail>, sqlx::Error> {
    let sql = format!(
        "{DETAIL_SELECT}
         WHERE p.visibility = 'public'
           AND p.slug = ?
           AND (o.name = ? OR u.username = ?)"
    );
    sqlx::query_as::<_, PluginDetail>(&sql)
        .bind(slug)
        .bind(owner)
        .bind(owner)
        .fetch_optional(pool)
        .await
}

pub async fn list(
    pool: &SqlitePool,
    q: &str,
    sort: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<PluginCard>, sqlx::Error> {
    let order = match sort {
        "downloads" => "p.downloads DESC, p.updated_at DESC",
        "stars" => "stars DESC, p.updated_at DESC",
        "name" => "p.name ASC",
        _ => "p.updated_at DESC",
    };
    let like = format!("%{}%", q.trim());
    let sql = format!(
        "SELECT p.id, COALESCE(o.name, u.username) AS owner,
                CASE WHEN p.org_id IS NULL THEN 'user' ELSE 'org' END AS owner_kind,
                p.slug, p.name, p.description, p.tags, p.downloads,
                (SELECT COUNT(*) FROM stars s WHERE s.plugin_id = p.id) AS stars,
                p.updated_at
         FROM plugins p
         LEFT JOIN users u ON u.id = p.owner_id
         LEFT JOIN orgs o ON o.id = p.org_id
         WHERE p.visibility = 'public'
           AND (? = '%%' OR p.name LIKE ? OR p.description LIKE ? OR p.tags LIKE ?
                OR COALESCE(o.name, u.username) LIKE ?)
         ORDER BY {order}
         LIMIT ? OFFSET ?"
    );
    sqlx::query_as::<_, PluginCard>(&sql)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

pub async fn count(pool: &SqlitePool, q: &str) -> Result<i64, sqlx::Error> {
    let like = format!("%{}%", q.trim());
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM plugins p
         LEFT JOIN users u ON u.id = p.owner_id
         LEFT JOIN orgs o ON o.id = p.org_id
         WHERE p.visibility = 'public'
           AND (? = '%%' OR p.name LIKE ? OR p.description LIKE ? OR p.tags LIKE ?
                OR COALESCE(o.name, u.username) LIKE ?)",
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn latest(pool: &SqlitePool, limit: i64) -> Result<Vec<PluginCard>, sqlx::Error> {
    list(pool, "", "updated", limit, 0).await
}

pub async fn top_downloads(pool: &SqlitePool, limit: i64) -> Result<Vec<PluginCard>, sqlx::Error> {
    list(pool, "", "downloads", limit, 0).await
}

pub async fn by_owner(pool: &SqlitePool, owner: &str) -> Result<Vec<PluginCard>, sqlx::Error> {
    list_by_owner(pool, owner, false).await
}

pub async fn by_org(pool: &SqlitePool, org: &str) -> Result<Vec<PluginCard>, sqlx::Error> {
    list_by_owner(pool, org, true).await
}

async fn list_by_owner(
    pool: &SqlitePool,
    owner: &str,
    is_org: bool,
) -> Result<Vec<PluginCard>, sqlx::Error> {
    let filter = if is_org {
        "o.name = ? AND p.org_id IS NOT NULL"
    } else {
        "u.username = ? AND p.owner_id IS NOT NULL"
    };
    let sql = format!(
        "SELECT p.id, COALESCE(o.name, u.username) AS owner,
                CASE WHEN p.org_id IS NULL THEN 'user' ELSE 'org' END AS owner_kind,
                p.slug, p.name, p.description, p.tags, p.downloads,
                (SELECT COUNT(*) FROM stars s WHERE s.plugin_id = p.id) AS stars,
                p.updated_at
         FROM plugins p
         LEFT JOIN users u ON u.id = p.owner_id
         LEFT JOIN orgs o ON o.id = p.org_id
         WHERE {filter} AND p.visibility = 'public'
         ORDER BY p.updated_at DESC"
    );
    sqlx::query_as::<_, PluginCard>(&sql)
        .bind(owner)
        .fetch_all(pool)
        .await
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM plugins WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn increment_downloads(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE plugins SET downloads = downloads + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn count_all(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM plugins")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

fn empty_to_none(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
