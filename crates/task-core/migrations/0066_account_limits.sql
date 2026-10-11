-- Authoritative settings, independent of events and usage observations.
CREATE TABLE account_limits (
    adapter TEXT NOT NULL,
    account_id TEXT NOT NULL,
    max_runs INTEGER NOT NULL CHECK (max_runs BETWEEN 1 AND 32),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (adapter, account_id)
);
