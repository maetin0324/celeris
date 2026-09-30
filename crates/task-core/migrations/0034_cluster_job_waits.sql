-- Migration 34 (schema_version=34): ADR-0090 D2。
--
-- クラスタ job（PBS / Slurm）の durable wait。worker の run が `result.json` の
-- `{"type": "wait", "kind": "cluster_job", ...}` で終わったとき、daemon が job の終了を待つ 1 件。
-- **events が正本**（`ClusterJobWaitStarted` / `ClusterJobWaitPolled` / `ClusterJobWaitFinished`）で、
-- この表は event と同じトランザクションで書く派生の索引（`task_core::cluster_job::apply_event_tx`）。
-- 例外は `last_polled_at` と、状態の変わらない poll の `last_status`（event を出さない。D3）。
--
--   wait_id        — ULID。
--   task_id / work_unit_id / run_id — 待った run（atomic の run なら work_unit_id は NULL）。
--   cluster        — `[[clusters]] id`。
--   scheduler      — `pbs` | `slurm`。
--   jobs_json      — job id の配列（JSON。worker が書いた順、重複なし）。
--   poll_secs      — poll の間隔（秒。クラスタの `job_wait.poll_secs` 以上に丸めた値）。
--   timeout_secs   — 待ちの上限（秒。クラスタの `job_wait.max_wait_secs` 以下に丸めた値）。
--   summary        — worker の 1 行要約（plain text、最大 500 文字）。
--   checkpoint_json — worker が添えた checkpoint の申告（生の JSON。合成済みの checkpoint は CheckpointSaved）。
--   created_at / deadline / last_polled_at / finished_at — RFC 3339。
--   state          — `waiting` | `satisfied` | `timed_out` | `cancelled`。
--   last_status_json — 直近の poll の job ごとの状態（`ClusterJobStatus` の配列、JSON）。
CREATE TABLE IF NOT EXISTS cluster_job_waits (
    wait_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    work_unit_id TEXT,
    run_id TEXT NOT NULL,
    cluster TEXT NOT NULL,
    scheduler TEXT NOT NULL CHECK (scheduler IN ('pbs', 'slurm')),
    jobs_json TEXT NOT NULL,
    poll_secs INTEGER NOT NULL,
    timeout_secs INTEGER NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    checkpoint_json TEXT,
    created_at TEXT NOT NULL,
    deadline TEXT NOT NULL,
    last_polled_at TEXT,
    finished_at TEXT,
    state TEXT NOT NULL CHECK (state IN ('waiting', 'satisfied', 'timed_out', 'cancelled')),
    last_status_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_cluster_job_waits_task ON cluster_job_waits (task_id, created_at);
CREATE INDEX IF NOT EXISTS idx_cluster_job_waits_state ON cluster_job_waits (state);
