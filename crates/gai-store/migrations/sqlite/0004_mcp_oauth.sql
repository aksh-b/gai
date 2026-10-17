-- OAuth tokens for MCP servers that require sign-in, by server name.
CREATE TABLE mcp_oauth (
    server      TEXT PRIMARY KEY,
    credentials TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
