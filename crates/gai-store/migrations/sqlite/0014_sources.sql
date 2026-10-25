-- Outside sources (folders, sites, GitHub, Confluence, Notion) synced
-- into knowledge collections. `state` maps each item seen to its version,
-- so a sync only fetches what changed.
CREATE TABLE sources (
    name         TEXT PRIMARY KEY,
    collection   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    spec         TEXT NOT NULL,
    state        TEXT NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'new',
    error        TEXT,
    stats        TEXT NOT NULL DEFAULT '{}',
    last_sync_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
