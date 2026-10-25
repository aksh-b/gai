-- Organizations, users, memberships and teams. Every organization but the
-- default one keeps its data in a database of its own; this directory and
-- all API keys live in the main database.
CREATE TABLE orgs (
    id         TEXT PRIMARY KEY,
    slug       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    settings   TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL
);
INSERT INTO orgs (id, slug, name, created_at) VALUES ('default', 'default', 'Default', now());

CREATE TABLE users (
    id           TEXT PRIMARY KEY,
    email        TEXT NOT NULL,
    name         TEXT,
    subject      TEXT UNIQUE,              -- issuer|sub from the identity provider
    disabled     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ
);

CREATE TABLE memberships (
    org_id     TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (org_id, user_id)
);

CREATE TABLE teams (
    id         TEXT PRIMARY KEY,
    org_id     TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE (org_id, name)
);

CREATE TABLE team_members (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (team_id, user_id)
);

CREATE TABLE resource_owners (
    org_id  TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    kind    TEXT NOT NULL,
    name    TEXT NOT NULL,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    PRIMARY KEY (org_id, kind, name)
);

CREATE TABLE logins (
    hash       TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

ALTER TABLE api_keys ADD COLUMN org_id TEXT NOT NULL DEFAULT 'default';
ALTER TABLE api_keys ADD COLUMN team_id TEXT;
ALTER TABLE api_keys ADD COLUMN user_id TEXT;
CREATE INDEX api_keys_org ON api_keys (org_id);
CREATE UNIQUE INDEX users_email ON users (lower(email));
