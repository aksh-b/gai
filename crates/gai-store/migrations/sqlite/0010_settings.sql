-- Settings changed at runtime (from the dashboard), by name: the default
-- model, and provider API keys as `provider_key:<provider>`.
CREATE TABLE settings (
    name       TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
