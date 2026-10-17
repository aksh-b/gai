-- Media (images, audio, uploads) kept with its metadata. Not tied to runs
-- by a foreign key: a blob outlives the run that made it.
CREATE TABLE blobs (
    id         TEXT PRIMARY KEY,
    media_type TEXT NOT NULL,
    size       INTEGER NOT NULL,
    sha256     TEXT NOT NULL,
    name       TEXT,
    run_id     TEXT,
    source     TEXT,
    metadata   TEXT NOT NULL DEFAULT '{}',
    data       BLOB NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX blobs_run ON blobs(run_id);
CREATE INDEX blobs_created ON blobs(created_at);
