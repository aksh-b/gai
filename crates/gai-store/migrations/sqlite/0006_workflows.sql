-- Workflows are versioned like agents: every saved spec is a new version.
CREATE TABLE workflows (
    name           TEXT PRIMARY KEY,
    latest_version INTEGER NOT NULL,
    description    TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE workflow_versions (
    name       TEXT NOT NULL REFERENCES workflows(name) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);

-- Child runs are looked up by parent.
CREATE INDEX runs_parent ON runs(parent_run_id);
