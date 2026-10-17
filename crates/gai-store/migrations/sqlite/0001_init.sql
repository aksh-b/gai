-- Agents are versioned: every saved spec is a new immutable version.
CREATE TABLE agents (
    name           TEXT PRIMARY KEY,
    latest_version INTEGER NOT NULL,
    labels         TEXT NOT NULL DEFAULT '{}',   -- {"prod": 3, "staging": 4}
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE agent_versions (
    name       TEXT NOT NULL REFERENCES agents(name) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE sessions (
    id         TEXT PRIMARY KEY,
    agent      TEXT,
    title      TEXT,
    user_id    TEXT,
    metadata   TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX sessions_updated ON sessions(updated_at);

-- The conversation, in order. `kind = 'compaction'` rows hold a summary
-- that replaces every message up to `covers_seq` when history is loaded.
CREATE TABLE messages (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    run_id     TEXT,
    kind       TEXT NOT NULL DEFAULT 'message',
    covers_seq INTEGER,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE runs (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    agent         TEXT NOT NULL,
    agent_version INTEGER,
    parent_run_id TEXT,
    status        TEXT NOT NULL,
    input         TEXT NOT NULL,
    output        TEXT,
    error         TEXT,
    usage         TEXT NOT NULL DEFAULT '{}',
    cost          REAL NOT NULL DEFAULT 0,
    steps         INTEGER NOT NULL DEFAULT 0,
    metadata      TEXT NOT NULL DEFAULT '{}',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    finished_at   TEXT
);
CREATE INDEX runs_session ON runs(session_id);
CREATE INDEX runs_status ON runs(status);
CREATE INDEX runs_created ON runs(created_at);

-- The step journal: one row per model call, tool call or approval, written
-- when the step starts and updated when it ends.
CREATE TABLE run_steps (
    run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT,
    status      TEXT NOT NULL,
    input       TEXT,
    output      TEXT,
    usage       TEXT,
    cost        REAL,
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    PRIMARY KEY (run_id, seq)
);

-- Every event a run emitted, for replaying streams and drawing traces.
CREATE TABLE run_events (
    run_id     TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    event      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE usage_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id     TEXT,
    model      TEXT NOT NULL,
    usage      TEXT NOT NULL,
    cost       REAL,
    created_at TEXT NOT NULL
);
CREATE INDEX usage_created ON usage_log(created_at);
