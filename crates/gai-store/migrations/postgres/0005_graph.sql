-- A knowledge graph per collection: entities found in chunks, the
-- relations between them, and which chunks mention each. Mentions,
-- relations and the processed marks go with their chunk.
CREATE TABLE graph_nodes (
    id          BIGSERIAL PRIMARY KEY,
    collection  TEXT NOT NULL,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'other',
    description TEXT,
    UNIQUE (collection, key)
);

CREATE TABLE graph_mentions (
    node_id  BIGINT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    chunk_id BIGINT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    PRIMARY KEY (node_id, chunk_id)
);
CREATE INDEX graph_mentions_chunk ON graph_mentions(chunk_id);

CREATE TABLE graph_edges (
    id         BIGSERIAL PRIMARY KEY,
    collection TEXT NOT NULL,
    source_id  BIGINT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    target_id  BIGINT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    relation   TEXT NOT NULL,
    chunk_id   BIGINT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    UNIQUE (source_id, target_id, relation, chunk_id)
);
CREATE INDEX graph_edges_source ON graph_edges(source_id);
CREATE INDEX graph_edges_target ON graph_edges(target_id);

CREATE TABLE graph_done (
    chunk_id BIGINT PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE
);
