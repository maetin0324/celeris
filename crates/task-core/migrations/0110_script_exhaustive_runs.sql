-- ADR 2026-11-04-lane-rules-exhaustive-evidence D4: lane_rules の全入力を本番の isolate で流した証拠。
--
-- 実行 1 回につき 1 行。live の `script_guard_state`・`script_compare_log`・`events` には触れない。
-- `state` が 'running' でない行の UPDATE・DELETE は trigger が拒む。領域ごとの 'running' は部分 UNIQUE index で 1 行に限る。
CREATE TABLE IF NOT EXISTS script_exhaustive_runs (
    id TEXT PRIMARY KEY,
    domain TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    bundle_sha256 TEXT NOT NULL,
    mode_at_start TEXT NOT NULL CHECK (mode_at_start IN ('compare', 'canary')),
    daemon_instance_id TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    reason TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('running', 'passed', 'failed', 'aborted')),
    stop_code TEXT CHECK (stop_code IS NULL OR stop_code IN (
        'mismatch', 'safety_violation', 'consecutive_errors', 'rate_exceeded',
        'generation_gone', 'wall_limit', 'daemon_stopped', 'daemon_restarted')),
    reference_sha256 TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    calls INTEGER NOT NULL DEFAULT 0,
    matches INTEGER NOT NULL DEFAULT 0,
    mismatches INTEGER NOT NULL DEFAULT 0,
    timeouts INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    safety_violations INTEGER NOT NULL DEFAULT 0,
    uncovered_inputs INTEGER NOT NULL DEFAULT 0,
    latency_p50_us INTEGER NOT NULL DEFAULT 0,
    latency_p95_us INTEGER NOT NULL DEFAULT 0,
    latency_max_us INTEGER NOT NULL DEFAULT 0,
    first_problem_json TEXT
);
CREATE INDEX IF NOT EXISTS script_exhaustive_runs_generation
    ON script_exhaustive_runs (domain, generation_id, started_at);
CREATE UNIQUE INDEX IF NOT EXISTS script_exhaustive_runs_one_running
    ON script_exhaustive_runs (domain) WHERE state = 'running';
CREATE TRIGGER IF NOT EXISTS script_exhaustive_runs_terminal_no_update
BEFORE UPDATE ON script_exhaustive_runs
WHEN OLD.state <> 'running'
BEGIN
    SELECT RAISE(ABORT, 'script_exhaustive_runs: a finished row is immutable');
END;
CREATE TRIGGER IF NOT EXISTS script_exhaustive_runs_terminal_no_delete
BEFORE DELETE ON script_exhaustive_runs
WHEN OLD.state <> 'running'
BEGIN
    SELECT RAISE(ABORT, 'script_exhaustive_runs: a finished row is immutable');
END;
