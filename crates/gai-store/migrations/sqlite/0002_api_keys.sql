-- API keys for the server. Only a SHA-256 hash of each key is stored; keys
-- are long random strings, so a fast hash is enough (no password stretching).
CREATE TABLE api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    prefix       TEXT NOT NULL,            -- first characters, for display
    hash         TEXT NOT NULL UNIQUE,
    scopes       TEXT NOT NULL,            -- ["admin"] | ["run", "read"] ...
    budget_usd   REAL,
    spent_usd    REAL NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at   TEXT
);
