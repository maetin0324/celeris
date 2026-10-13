CREATE TABLE routing_evidence (
    genre TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    harness_version TEXT NOT NULL,
    source_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    evidence_version TEXT NOT NULL,
    n INTEGER NOT NULL CHECK (n >= 0),
    pass INTEGER NOT NULL CHECK (pass >= 0 AND pass <= n),
    pass_lower REAL NOT NULL,
    pass_upper REAL NOT NULL,
    wall_median_s REAL,
    usd_list_median REAL,
    quota_per_run_json TEXT NOT NULL,
    timeout_rate REAL NOT NULL,
    low_confidence INTEGER NOT NULL CHECK (low_confidence IN (0, 1)),
    PRIMARY KEY (genre, harness_id, harness_version, source_id, model_id, evidence_version)
);
CREATE INDEX idx_routing_evidence_lookup
    ON routing_evidence(genre, harness_id, harness_version, source_id, model_id);
