-- Databases agents query (read-only unless a connection says otherwise),
-- with their last introspected schema. Passwords and tokens are never
-- stored here: specs name the environment variable or secret holding them.
CREATE TABLE data_connections (
    name          TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    spec          TEXT NOT NULL,
    schema        TEXT,
    schema_at     TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL
);
