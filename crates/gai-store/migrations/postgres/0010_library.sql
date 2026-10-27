-- The library: named, versioned JSON items of a kind (prompt templates,
-- marketplace listings), labelled like agents and rated by people.
CREATE TABLE library_items (
    kind           TEXT NOT NULL,
    name           TEXT NOT NULL,
    latest_version BIGINT NOT NULL,
    title          TEXT,
    description    TEXT,
    labels         TEXT NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (kind, name)
);

CREATE TABLE library_versions (
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    version    BIGINT NOT NULL,
    spec       TEXT NOT NULL,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (kind, name, version),
    FOREIGN KEY (kind, name) REFERENCES library_items(kind, name) ON DELETE CASCADE
);

-- One rating per person (or key) per item; rating again replaces it.
CREATE TABLE library_ratings (
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    rater      TEXT NOT NULL,
    stars      BIGINT NOT NULL,
    comment    TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (kind, name, rater),
    FOREIGN KEY (kind, name) REFERENCES library_items(kind, name) ON DELETE CASCADE
);

-- Background work (e.g. prompt optimization) and what it made.
CREATE TABLE jobs (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    status     TEXT NOT NULL,
    input      TEXT NOT NULL,
    result     TEXT,
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX jobs_kind_created ON jobs (kind, created_at);
