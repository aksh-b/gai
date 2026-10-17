-- MCP servers added at runtime (servers in gai.toml are not stored).
CREATE TABLE mcp_servers (
    name       TEXT PRIMARY KEY,
    config     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
