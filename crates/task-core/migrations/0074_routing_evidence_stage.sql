CREATE TABLE routing_evidence (
    genre TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    harness_version TEXT NOT NULL,
    source_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    evidence_version TEXT NOT NULL,
    n INTEGER NOT NULL,
    pass INTEGER NOT NULL,
    pass_lower REAL NOT NULL,
    pass_upper REAL NOT NULL,
    wall_median_s REAL NOT NULL,
    usd_list_median REAL NOT NULL,
    quota_per_run_json TEXT NOT NULL,
    timeout_rate REAL NOT NULL,
    PRIMARY KEY (genre, harness_id, harness_version, source_id, model_id, evidence_version)
);
CREATE INDEX idx_routing_evidence_latest ON routing_evidence
    (genre, harness_id, evidence_version DESC);

CREATE TABLE routing_stage (
    genre TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN ('shadow', 'enforce')),
    since TEXT NOT NULL,
    evidence_version TEXT,
    PRIMARY KEY (genre, harness_id)
);
