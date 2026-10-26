-- Dashboards: charts with their queries, and the results of the last
-- refresh (which alerts compare with to fire once, not every time).
CREATE TABLE dashboards (
    name         TEXT PRIMARY KEY,
    spec         TEXT NOT NULL,
    results      TEXT,
    refreshed_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
