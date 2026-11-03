-- ADR 2026-11-03-cos-digest-persist: the outbox of the `cos_digest` compare shadow.
--
-- Mode `compare` (and the Rust side of `canary`) applies Rust's commands first and runs TS on a
-- separate task afterwards. The apply transaction adds one row per applied run; `json` is the
-- `script_compare_log` record (outcome `unavailable`) to write if the shadow never finishes.
-- The shadow, or the dispatcher's reconcile after a restart, deletes the row and appends exactly
-- one record in one transaction. `script_compare_log` stays append-only.
CREATE TABLE IF NOT EXISTS cos_digest_shadow_outbox (
    pass_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (pass_id, run_id)
);
