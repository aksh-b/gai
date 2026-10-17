-- Triggers start runs on their own: on a schedule, from a webhook, or when
-- files change. `spec` holds the trigger's definition as JSON.
CREATE TABLE triggers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    kind          TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    spec          TEXT NOT NULL,
    secret        TEXT,
    next_fire_at  TEXT,
    last_fired_at TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- What each trigger did: the runs it started, and firings it skipped.
CREATE TABLE trigger_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_id TEXT NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
    fired_at   TEXT NOT NULL,
    reason     TEXT NOT NULL,
    payload    TEXT,
    run_id     TEXT,
    status     TEXT NOT NULL,
    error      TEXT
);
CREATE INDEX trigger_events_trigger ON trigger_events(trigger_id, id);
