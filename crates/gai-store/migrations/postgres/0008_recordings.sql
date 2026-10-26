-- Recordings: a meeting, call or video and what was made of it
-- (transcript with speakers, notes, summary), filled in once processed.
CREATE TABLE recordings (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    title      TEXT NOT NULL,
    blob_id    TEXT,
    status     TEXT NOT NULL,
    options    TEXT NOT NULL,
    result     TEXT,
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX recordings_created ON recordings (created_at);
