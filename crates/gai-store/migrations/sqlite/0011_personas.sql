-- Personas are versioned like agents: every saved spec is a new version.
-- The summary columns are copied from the latest spec for list views.
CREATE TABLE personas (
    name           TEXT PRIMARY KEY,
    latest_version INTEGER NOT NULL,
    display_name   TEXT,
    description    TEXT,
    avatar         TEXT,
    labels         TEXT NOT NULL DEFAULT '{}',   -- {"prod": 3, "staging": 4}
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE persona_versions (
    name       TEXT NOT NULL REFERENCES personas(name) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);
