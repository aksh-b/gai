-- Dashboards: charts with their queries, and the results of the last
-- refresh (which alerts compare with to fire once, not every time).
CREATE TABLE dashboards (
    name         TEXT PRIMARY KEY,
    spec         TEXT NOT NULL,
    results      TEXT,
    refreshed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);
