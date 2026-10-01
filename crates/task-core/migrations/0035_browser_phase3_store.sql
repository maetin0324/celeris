-- ADR-0099..0083: Phase 3 browser persistence. All text in live events is scrubbed before insertion.
CREATE TABLE browser_live_events (
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    event_seq INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('status','tabs','url','console')),
    body_json TEXT NOT NULL,
    PRIMARY KEY (task_id, run_id, session_id, event_seq)
);
CREATE INDEX idx_browser_live_events_run ON browser_live_events(task_id, run_id, session_id, event_seq);

CREATE TABLE browser_control_state (
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    version INTEGER NOT NULL,
    lease_holder TEXT,
    lease_expires_at INTEGER,
    state_json TEXT NOT NULL,
    PRIMARY KEY (task_id, run_id, session_id)
);
CREATE TABLE browser_control_actions (
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    command_json TEXT NOT NULL,
    outcome_json TEXT NOT NULL,
    PRIMARY KEY (task_id, run_id, session_id, idempotency_key)
);

CREATE TABLE browser_identities (
    identity_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    key_label TEXT NOT NULL,
    generation INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    demand_confirmed_by TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active','revoked','deleted')),
    tombstone INTEGER NOT NULL DEFAULT 0 CHECK (tombstone IN (0,1)),
    sealed_blob BLOB,
    CHECK (tombstone = 0 OR sealed_blob IS NULL)
);
CREATE INDEX idx_browser_identities_project_origin ON browser_identities(project_id, origin);
