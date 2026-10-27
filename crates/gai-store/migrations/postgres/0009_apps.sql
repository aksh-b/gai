-- Apps: an agent or workflow published as a form and an endpoint, with
-- the hash of its own key (if it has one).
CREATE TABLE apps (
    name       TEXT PRIMARY KEY,
    spec       TEXT NOT NULL,
    key_hash   TEXT,
    key_prefix TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
