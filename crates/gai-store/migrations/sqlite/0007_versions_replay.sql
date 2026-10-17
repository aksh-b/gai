-- Workflows get labels like agents: {"prod": 3, "staging": 4}.
ALTER TABLE workflows ADD COLUMN labels TEXT NOT NULL DEFAULT '{}';

-- Forks (replays) of a run are looked up by the run they came from.
CREATE INDEX runs_fork_of ON runs(json_extract(metadata, '$.fork.run_id'));
