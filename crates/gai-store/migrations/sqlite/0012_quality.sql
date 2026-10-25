-- Ratings people give replies: one per run and user, changeable.
CREATE TABLE feedback (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id    TEXT NOT NULL,
    agent         TEXT NOT NULL,
    agent_version INTEGER,
    user_id       TEXT NOT NULL DEFAULT '',
    rating        INTEGER NOT NULL,          -- 1 good, -1 bad
    comment       TEXT,
    tags          TEXT NOT NULL DEFAULT '[]',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (run_id, user_id)
);
CREATE INDEX feedback_agent ON feedback(agent, created_at);

-- Eval sets are versioned like agents.
CREATE TABLE eval_sets (
    name           TEXT PRIMARY KEY,
    latest_version INTEGER NOT NULL,
    description    TEXT,
    cases          INTEGER NOT NULL DEFAULT 0,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE eval_set_versions (
    name       TEXT NOT NULL REFERENCES eval_sets(name) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);

-- One evaluation: an eval set version against one target (agent version
-- and model), with its summary once finished.
CREATE TABLE eval_runs (
    id          TEXT PRIMARY KEY,
    eval_set    TEXT NOT NULL,
    eval_version INTEGER NOT NULL,
    target      TEXT NOT NULL,               -- {"agent", "agent_version", "model"}
    status      TEXT NOT NULL,               -- running | completed | failed
    summary     TEXT NOT NULL DEFAULT '{}',
    error       TEXT,
    created_at  TEXT NOT NULL,
    finished_at TEXT
);
CREATE INDEX eval_runs_set ON eval_runs(eval_set, created_at);

CREATE TABLE eval_results (
    eval_run_id TEXT NOT NULL REFERENCES eval_runs(id) ON DELETE CASCADE,
    case_id     TEXT NOT NULL,
    run_id      TEXT,
    output      TEXT,
    scores      TEXT NOT NULL DEFAULT '[]',
    score       REAL NOT NULL,
    passed      INTEGER NOT NULL,
    cost        REAL NOT NULL DEFAULT 0,
    latency_ms  INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    PRIMARY KEY (eval_run_id, case_id)
);
