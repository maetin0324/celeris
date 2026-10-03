-- ADR-0140 D2: execute continuation 用の継続セッション（`node_sessions.kind = 'continuation'`）。
--
-- 新しい表は作らず `node_sessions` に列を足す。key は `(task_id, work_unit_id, adapter, account_id)`。
--   task_id       — その WU を持つ Task の id（`continuation` 行だけ非 NULL）。
--   work_unit_id  — WU の key。atomic な Task（WU を持たない）は NULL を「Task 全体で 1 本」として扱う。
--   provider      — account の provider（key には入れない。判断表 #7 で比べるための記録）。
--   cwd           — session を作ったときの WU worktree の path（判断表 #8 で比べるための記録）。
-- 既存の `conversation` / `lead` 行はすべて NULL のまま（意味を変えない）。
-- 「key ごとに現役は高々 1 行」はストアで強制せず、dispatcher が retire → create の順で保つ（ADR-0054 と同じ）。

ALTER TABLE node_sessions ADD COLUMN task_id TEXT;
ALTER TABLE node_sessions ADD COLUMN work_unit_id TEXT;
ALTER TABLE node_sessions ADD COLUMN provider TEXT;
ALTER TABLE node_sessions ADD COLUMN cwd TEXT;

CREATE INDEX IF NOT EXISTS idx_node_sessions_work_unit_active
    ON node_sessions (kind, task_id, work_unit_id, retired_at);
