CREATE TABLE route_features (
    hash TEXT PRIMARY KEY,
    schema TEXT NOT NULL,
    template TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    brief TEXT NOT NULL,
    aux_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE route_options (
    hash TEXT PRIMARY KEY,
    schema TEXT NOT NULL,
    builder TEXT NOT NULL,
    question TEXT NOT NULL,
    options_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE route_decisions (
    decision_id TEXT PRIMARY KEY,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    run_id TEXT,
    features_hash TEXT NOT NULL REFERENCES route_features(hash),
    options_hash TEXT NOT NULL REFERENCES route_options(hash),
    input_json TEXT NOT NULL,
    output_json TEXT NOT NULL,
    judge_json TEXT NOT NULL,
    stage TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_route_decisions_export ON route_decisions(created_at, stage, decision_id);

CREATE TABLE route_outcomes (
    decision_id TEXT PRIMARY KEY REFERENCES route_decisions(decision_id),
    schema TEXT NOT NULL,
    labels_json TEXT NOT NULL,
    recorded_at TEXT NOT NULL
);

CREATE TABLE route_judgments (
    judgment_id TEXT PRIMARY KEY,
    features_hash TEXT NOT NULL REFERENCES route_features(hash),
    options_hash TEXT NOT NULL REFERENCES route_options(hash),
    input_sha256 TEXT NOT NULL,
    judge_json TEXT NOT NULL,
    probs_json TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL
);
