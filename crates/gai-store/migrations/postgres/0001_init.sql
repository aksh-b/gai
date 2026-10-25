-- The whole gai schema on Postgres. It mirrors migrations/sqlite (JSON is
-- kept as text, so both backends share their code shape); timestamps are
-- TIMESTAMPTZ, full-text search is a generated tsvector.

CREATE TABLE agents (
    name           TEXT PRIMARY KEY,
    latest_version BIGINT NOT NULL,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE agent_versions (
    name       TEXT NOT NULL REFERENCES agents(name) ON DELETE CASCADE,
    version    BIGINT NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE sessions (
    id         TEXT PRIMARY KEY,
    agent      TEXT,
    title      TEXT,
    user_id    TEXT,
    metadata   TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_updated ON sessions(updated_at);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_agent ON sessions(agent, updated_at);

CREATE TABLE messages (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,
    run_id     TEXT,
    kind       TEXT NOT NULL DEFAULT 'message',
    covers_seq BIGINT,
    body       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE runs (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    agent         TEXT NOT NULL,
    agent_version BIGINT,
    parent_run_id TEXT,
    status        TEXT NOT NULL,
    input         TEXT NOT NULL,
    output        TEXT,
    error         TEXT,
    usage         TEXT NOT NULL DEFAULT '{}',
    cost          DOUBLE PRECISION NOT NULL DEFAULT 0,
    steps         BIGINT NOT NULL DEFAULT 0,
    metadata      TEXT NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL,
    finished_at   TIMESTAMPTZ
);
CREATE INDEX runs_session ON runs(session_id);
CREATE INDEX runs_status ON runs(status);
CREATE INDEX runs_created ON runs(created_at);
CREATE INDEX runs_parent ON runs(parent_run_id);
CREATE INDEX runs_fork_of ON runs(((metadata::jsonb) #>> '{fork,run_id}'));

CREATE TABLE run_steps (
    run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq         BIGINT NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT,
    status      TEXT NOT NULL,
    input       TEXT,
    output      TEXT,
    usage       TEXT,
    cost        DOUBLE PRECISION,
    started_at  TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE run_events (
    run_id     TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,
    event      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE usage_log (
    id         BIGSERIAL PRIMARY KEY,
    run_id     TEXT,
    model      TEXT NOT NULL,
    usage      TEXT NOT NULL,
    cost       DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX usage_created ON usage_log(created_at);

CREATE TABLE api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    prefix       TEXT NOT NULL,
    hash         TEXT NOT NULL UNIQUE,
    scopes       TEXT NOT NULL,
    budget_usd   DOUBLE PRECISION,
    spent_usd    DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE TABLE mcp_servers (
    name       TEXT PRIMARY KEY,
    config     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE mcp_oauth (
    server      TEXT PRIMARY KEY,
    credentials TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);

CREATE TABLE memory_blocks (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,
    label       TEXT NOT NULL,
    description TEXT,
    value       TEXT NOT NULL,
    char_limit  BIGINT NOT NULL,
    read_only   BOOLEAN NOT NULL DEFAULT FALSE,
    version     BIGINT NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL,
    UNIQUE (scope, label)
);

CREATE TABLE memory_block_history (
    block_id   TEXT NOT NULL REFERENCES memory_blocks(id) ON DELETE CASCADE,
    version    BIGINT NOT NULL,
    value      TEXT NOT NULL,
    op         TEXT NOT NULL,
    actor      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (block_id, version)
);

CREATE TABLE collections (
    name            TEXT PRIMARY KEY,
    description     TEXT,
    embedding_model TEXT NOT NULL,
    dimensions      BIGINT,
    chunk_tokens    BIGINT NOT NULL,
    chunk_overlap   BIGINT NOT NULL,
    metadata        TEXT NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL
);

CREATE TABLE documents (
    id           TEXT PRIMARY KEY,
    collection   TEXT NOT NULL REFERENCES collections(name) ON DELETE CASCADE,
    source       TEXT NOT NULL,
    title        TEXT,
    content_hash TEXT NOT NULL,
    chars        BIGINT NOT NULL,
    chunks       BIGINT NOT NULL,
    metadata     TEXT NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL,
    UNIQUE (collection, source)
);
CREATE INDEX documents_collection ON documents(collection, created_at);

CREATE TABLE chunks (
    id          BIGSERIAL PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    collection  TEXT NOT NULL,
    seq         BIGINT NOT NULL,
    text        TEXT NOT NULL,
    embedding   BYTEA,
    metadata    TEXT NOT NULL DEFAULT '{}',
    tsv         TSVECTOR GENERATED ALWAYS AS (to_tsvector('english', text)) STORED
);
CREATE INDEX chunks_collection ON chunks(collection);
CREATE INDEX chunks_document ON chunks(document_id, seq);
CREATE INDEX chunks_tsv ON chunks USING GIN (tsv);

CREATE TABLE skills (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    version     BIGINT NOT NULL,
    body        TEXT NOT NULL,
    files       TEXT NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);

CREATE TABLE workflows (
    name           TEXT PRIMARY KEY,
    latest_version BIGINT NOT NULL,
    description    TEXT,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE workflow_versions (
    name       TEXT NOT NULL REFERENCES workflows(name) ON DELETE CASCADE,
    version    BIGINT NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE blobs (
    id         TEXT PRIMARY KEY,
    media_type TEXT NOT NULL,
    size       BIGINT NOT NULL,
    sha256     TEXT NOT NULL,
    name       TEXT,
    run_id     TEXT,
    source     TEXT,
    metadata   TEXT NOT NULL DEFAULT '{}',
    data       BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX blobs_run ON blobs(run_id);
CREATE INDEX blobs_created ON blobs(created_at);

CREATE TABLE triggers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    kind          TEXT NOT NULL,
    enabled       BIGINT NOT NULL DEFAULT 1,
    spec          TEXT NOT NULL,
    secret        TEXT,
    next_fire_at  TIMESTAMPTZ,
    last_fired_at TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL
);

CREATE TABLE trigger_events (
    id         BIGSERIAL PRIMARY KEY,
    trigger_id TEXT NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
    fired_at   TIMESTAMPTZ NOT NULL,
    reason     TEXT NOT NULL,
    payload    TEXT,
    run_id     TEXT,
    status     TEXT NOT NULL,
    error      TEXT
);
CREATE INDEX trigger_events_trigger ON trigger_events(trigger_id, id);

CREATE TABLE settings (
    name       TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE personas (
    name           TEXT PRIMARY KEY,
    latest_version BIGINT NOT NULL,
    display_name   TEXT,
    description    TEXT,
    avatar         TEXT,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE persona_versions (
    name       TEXT NOT NULL REFERENCES personas(name) ON DELETE CASCADE,
    version    BIGINT NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE feedback (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id    TEXT NOT NULL,
    agent         TEXT NOT NULL,
    agent_version BIGINT,
    user_id       TEXT NOT NULL DEFAULT '',
    rating        BIGINT NOT NULL,
    comment       TEXT,
    tags          TEXT NOT NULL DEFAULT '[]',
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL,
    UNIQUE (run_id, user_id)
);
CREATE INDEX feedback_agent ON feedback(agent, created_at);

CREATE TABLE eval_sets (
    name           TEXT PRIMARY KEY,
    latest_version BIGINT NOT NULL,
    description    TEXT,
    cases          BIGINT NOT NULL DEFAULT 0,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE eval_set_versions (
    name       TEXT NOT NULL REFERENCES eval_sets(name) ON DELETE CASCADE,
    version    BIGINT NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE eval_runs (
    id           TEXT PRIMARY KEY,
    eval_set     TEXT NOT NULL,
    eval_version BIGINT NOT NULL,
    target       TEXT NOT NULL,
    status       TEXT NOT NULL,
    summary      TEXT NOT NULL DEFAULT '{}',
    error        TEXT,
    created_at   TIMESTAMPTZ NOT NULL,
    finished_at  TIMESTAMPTZ
);
CREATE INDEX eval_runs_set ON eval_runs(eval_set, created_at);

CREATE TABLE eval_results (
    ord         BIGSERIAL,
    eval_run_id TEXT NOT NULL REFERENCES eval_runs(id) ON DELETE CASCADE,
    case_id     TEXT NOT NULL,
    run_id      TEXT,
    output      TEXT,
    scores      TEXT NOT NULL DEFAULT '[]',
    score       DOUBLE PRECISION NOT NULL,
    passed      BIGINT NOT NULL,
    cost        DOUBLE PRECISION NOT NULL DEFAULT 0,
    latency_ms  BIGINT NOT NULL DEFAULT 0,
    error       TEXT,
    PRIMARY KEY (eval_run_id, case_id)
);

CREATE TABLE audit_log (
    id       BIGSERIAL PRIMARY KEY,
    at       TIMESTAMPTZ NOT NULL,
    actor    TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    action   TEXT NOT NULL,
    target   TEXT NOT NULL,
    status   BIGINT NOT NULL,
    detail   TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_at ON audit_log(at);
CREATE INDEX audit_actor ON audit_log(actor, at);
