-- plugin-hub 初始 schema（SQLite）
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS oauth_accounts (
    user_id          TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider         TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    login            TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (provider, provider_user_id)
);

CREATE TABLE IF NOT EXISTS orgs (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    owner_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS org_members (
    org_id     TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL DEFAULT 'member',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, user_id)
);

CREATE TABLE IF NOT EXISTS teams (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    UNIQUE (org_id, name)
);

CREATE TABLE IF NOT EXISTS team_members (
    team_id    TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL DEFAULT 'member',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (team_id, user_id)
);

CREATE TABLE IF NOT EXISTS plugins (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT REFERENCES users(id) ON DELETE CASCADE,
    org_id      TEXT REFERENCES orgs(id) ON DELETE CASCADE,
    slug        TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    readme      TEXT NOT NULL DEFAULT '',
    homepage    TEXT,
    license     TEXT,
    tags        TEXT NOT NULL DEFAULT '',
    visibility  TEXT NOT NULL DEFAULT 'public',
    downloads   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    CHECK ((owner_id IS NOT NULL AND org_id IS NULL) OR (owner_id IS NULL AND org_id IS NOT NULL))
);

-- 用户/组织各自的 slug 唯一；SQLite 的部分索引正好表达“二者互斥”。
CREATE UNIQUE INDEX IF NOT EXISTS idx_plugins_user_slug
    ON plugins(owner_id, slug) WHERE owner_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_plugins_org_slug
    ON plugins(org_id, slug) WHERE org_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_plugins_owner ON plugins(owner_id);
CREATE INDEX IF NOT EXISTS idx_plugins_org ON plugins(org_id);
CREATE INDEX IF NOT EXISTS idx_plugins_updated ON plugins(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_plugins_downloads ON plugins(downloads DESC);

CREATE TABLE IF NOT EXISTS releases (
    id          TEXT PRIMARY KEY,
    plugin_id   TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
    version     TEXT NOT NULL,
    notes       TEXT NOT NULL DEFAULT '',
    asset_path  TEXT NOT NULL,
    asset_size  INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    downloads   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    UNIQUE(plugin_id, version)
);

CREATE TABLE IF NOT EXISTS stars (
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plugin_id  TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, plugin_id)
);

CREATE TABLE IF NOT EXISTS api_tokens (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    prefix       TEXT NOT NULL,
    scopes       TEXT NOT NULL DEFAULT 'read',
    last_used_at INTEGER,
    created_at   INTEGER NOT NULL,
    revoked_at   INTEGER
);

CREATE TABLE IF NOT EXISTS webhooks (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    url        TEXT NOT NULL,
    secret     TEXT NOT NULL,
    events     TEXT NOT NULL DEFAULT '*',
    active     INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id          TEXT PRIMARY KEY,
    webhook_id  TEXT NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    event       TEXT NOT NULL,
    status_code INTEGER,
    ok          INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_releases_plugin ON releases(plugin_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_stars_plugin ON stars(plugin_id);
CREATE INDEX IF NOT EXISTS idx_org_members_user ON org_members(user_id);
CREATE INDEX IF NOT EXISTS idx_team_members_user ON team_members(user_id);
CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries ON webhook_deliveries(webhook_id, created_at DESC);