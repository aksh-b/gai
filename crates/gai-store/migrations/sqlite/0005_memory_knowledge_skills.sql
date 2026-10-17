-- Memory blocks: named, size-limited text an agent always sees and can
-- edit. `scope` is `agent:<name>`, `user:<id>` or `shared:<name>`.
CREATE TABLE memory_blocks (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,
    label       TEXT NOT NULL,
    description TEXT,
    value       TEXT NOT NULL,
    char_limit  INTEGER NOT NULL,
    read_only   INTEGER NOT NULL DEFAULT 0,
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (scope, label)
);

-- Every value a block has had, with who changed it.
CREATE TABLE memory_block_history (
    block_id   TEXT NOT NULL REFERENCES memory_blocks(id) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    value      TEXT NOT NULL,
    op         TEXT NOT NULL,
    actor      TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (block_id, version)
);

-- Knowledge collections. Every chunk in a collection is embedded with the
-- collection's model, so queries can be compared with it.
CREATE TABLE collections (
    name            TEXT PRIMARY KEY,
    description     TEXT,
    embedding_model TEXT NOT NULL,
    dimensions      INTEGER,
    chunk_tokens    INTEGER NOT NULL,
    chunk_overlap   INTEGER NOT NULL,
    metadata        TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE documents (
    id           TEXT PRIMARY KEY,
    collection   TEXT NOT NULL REFERENCES collections(name) ON DELETE CASCADE,
    source       TEXT NOT NULL,
    title        TEXT,
    content_hash TEXT NOT NULL,
    chars        INTEGER NOT NULL,
    chunks       INTEGER NOT NULL,
    metadata     TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    UNIQUE (collection, source)
);
CREATE INDEX documents_collection ON documents(collection, created_at);

-- Embeddings are little-endian f32 blobs.
CREATE TABLE chunks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    collection  TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    text        TEXT NOT NULL,
    embedding   BLOB,
    metadata    TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX chunks_collection ON chunks(collection);
CREATE INDEX chunks_document ON chunks(document_id, seq);

-- Full-text index over chunks, for keyword and hybrid search.
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    text,
    content = 'chunks',
    content_rowid = 'id',
    tokenize = 'porter unicode61'
);
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts (chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

-- Agent Skills uploaded through the API (skills in folders are read from
-- disk). `body` is the whole SKILL.md; `files` maps relative paths to text.
CREATE TABLE skills (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    version     INTEGER NOT NULL,
    body        TEXT NOT NULL,
    files       TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
