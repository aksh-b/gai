-- Who changed what: one row per request that changed something (agents,
-- keys, settings, MCP servers, approvals, ...).
CREATE TABLE audit_log (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    at       TEXT NOT NULL,
    actor    TEXT NOT NULL,            -- the API key's name, or anonymous
    actor_id TEXT NOT NULL,            -- the key id
    action   TEXT NOT NULL,            -- e.g. "POST /v1/agents"
    target   TEXT NOT NULL,            -- what it touched, e.g. "agents/researcher"
    status   INTEGER NOT NULL,         -- HTTP status of the answer
    detail   TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_at ON audit_log(at);
CREATE INDEX audit_actor ON audit_log(actor, at);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_agent ON sessions(agent, updated_at);
