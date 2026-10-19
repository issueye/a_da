use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Org {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub owner_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrgMember {
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct Team {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub description: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TeamMember {
    pub user_id: String,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct OrgChoice {
    pub id: String,
    pub name: String,
}

impl Org {
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
}

impl Team {
    pub fn created_text(&self) -> String {
        crate::views::format_ts(self.created_at)
    }
}

pub fn can_manage(role: Option<&str>) -> bool {
    matches!(role, Some("owner") | Some("admin"))
}

pub async fn create(
    pool: &SqlitePool,
    name: &str,
    display_name: &str,
    description: &str,
    owner_id: &str,
) -> Result<Org, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let now = crate::views::now_ts();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO orgs (id, name, display_name, description, owner_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(display_name)
    .bind(description)
    .bind(owner_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, created_at) VALUES (?, ?, 'owner', ?)",
    )
    .bind(&id)
    .bind(owner_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Org {
        id,
        name: name.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        owner_id: owner_id.to_string(),
        created_at: now,
    })
}

pub async fn find_by_name(pool: &SqlitePool, name: &str) -> Result<Option<Org>, sqlx::Error> {
    sqlx::query_as::<_, Org>(
        "SELECT id, name, display_name, description, owner_id, created_at FROM orgs WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Org>, sqlx::Error> {
    sqlx::query_as::<_, Org>(
        "SELECT id, name, display_name, description, owner_id, created_at
         FROM orgs ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<OrgChoice>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT o.id, o.name FROM orgs o
         JOIN org_members m ON m.org_id = o.id
         WHERE m.user_id = ? ORDER BY o.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(id, name)| OrgChoice { id, name })
            .collect()
    })
}

pub async fn owner_id(pool: &SqlitePool, org_id: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT owner_id FROM orgs WHERE id = ?")
        .bind(org_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn member_role(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT role FROM org_members WHERE org_id = ? AND user_id = ?")
            .bind(org_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

pub async fn members(pool: &SqlitePool, org_id: &str) -> Result<Vec<OrgMember>, sqlx::Error> {
    sqlx::query_as::<_, OrgMember>(
        "SELECT m.user_id, u.username, m.role, m.created_at
         FROM org_members m JOIN users u ON u.id = m.user_id
         WHERE m.org_id = ?
         ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, u.username",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn add_member(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(org_id, user_id) DO UPDATE SET role = excluded.role",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(role)
    .bind(crate::views::now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_member(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM org_members WHERE org_id = ? AND user_id = ?")
        .bind(org_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_team(
    pool: &SqlitePool,
    org_id: &str,
    name: &str,
    description: &str,
) -> Result<Team, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let now = crate::views::now_ts();
    sqlx::query(
        "INSERT INTO teams (id, org_id, name, description, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(org_id)
    .bind(name)
    .bind(description)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Team {
        id,
        org_id: org_id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        created_at: now,
    })
}

pub async fn teams(pool: &SqlitePool, org_id: &str) -> Result<Vec<Team>, sqlx::Error> {
    sqlx::query_as::<_, Team>(
        "SELECT id, org_id, name, description, created_at FROM teams WHERE org_id = ? ORDER BY name",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn team_members(
    pool: &SqlitePool,
    team_id: &str,
) -> Result<Vec<TeamMember>, sqlx::Error> {
    sqlx::query_as::<_, TeamMember>(
        "SELECT tm.user_id, u.username, tm.role
         FROM team_members tm JOIN users u ON u.id = tm.user_id
         WHERE tm.team_id = ? ORDER BY u.username",
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
}

pub async fn add_team_member(
    pool: &SqlitePool,
    team_id: &str,
    user_id: &str,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(team_id, user_id) DO UPDATE SET role = excluded.role",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(role)
    .bind(crate::views::now_ts())
    .execute(pool)
    .await?;
    Ok(())
}
