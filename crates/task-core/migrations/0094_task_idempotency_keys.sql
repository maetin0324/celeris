-- Migration 94（schema_version=94）: ADR 2026-10-27-task-idempotency-keys。
--
-- `task_idempotency_keys` — `POST /tasks` と `POST /tasks/{id}/retry` の冪等キー。キーは 1 つの名前空間で、
-- `request_hash`（method・path・キーを除いた本文の正規化 JSON の sha256）が一致する再送は `task_id` の結果を返し、
-- 違えば 409 `idempotency_key_conflict`。行は task と同じ transaction で書き、消さない。
-- `result_json` は retry の応答（`RetryResult`）。作成は task を読み直すので NULL。
CREATE TABLE task_idempotency_keys (
    idempotency_key TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    task_id TEXT NOT NULL,
    result_json TEXT,
    created_at TEXT NOT NULL
);
