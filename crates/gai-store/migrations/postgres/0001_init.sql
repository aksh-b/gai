-- The Postgres schema so far: versioned agents, runtime settings and MCP
-- servers (see PostgresStore for what is not ported yet).
CREATE TABLE agents (
    name           TEXT PRIMARY KEY,
    latest_version INTEGER NOT NULL,
    labels         JSONB NOT NULL DEFAULT '{}',   -- {"prod": 3, "staging": 4}
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE agent_versions (
    name       TEXT NOT NULL REFERENCES agents(name) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    spec       JSONB NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE settings (
    name       TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE mcp_servers (
    name       TEXT PRIMARY KEY,
    config     JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE mcp_oauth (
    server      TEXT PRIMARY KEY,
    credentials JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);
