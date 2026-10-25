CREATE TABLE sources (
    name         TEXT PRIMARY KEY,
    collection   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    spec         TEXT NOT NULL,
    state        TEXT NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'new',
    error        TEXT,
    stats        TEXT NOT NULL DEFAULT '{}',
    last_sync_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);
