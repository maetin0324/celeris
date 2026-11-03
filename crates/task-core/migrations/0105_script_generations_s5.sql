-- ADR 2026-11-02-ts-control-layer-boundary 付記 S5-1・S5-3・S5-7。
--
-- `script_generations.state` を loaded / active / draining / retired / invalid に改める（S2-4 の `rejected` を消す）。
--   - 旧 `rejected` → `invalid`。
--   - mode の行が指す旧 `loaded` → `active`。
--   - どの mode の行も指さない旧 `loaded` → `retired`（再起動で isolate は残らない。S5-1 の登録で `loaded` に戻る）。
-- `script_domain_modes.canary_bps` — `canary` の比率（1〜9999）。`canary` 以外は NULL。
-- `script_generation_events` — generation の状態遷移の記録（追記専用）。
-- `script_guard_state` — 停止基準の計数（(domain, generation_id) ごと。領域 `*` の行は全体の停止）。
-- checkpoint の表は作らない（S5-5）。
--
-- `script_domain_modes.generation_id` が親を参照する（bundled SQLite の foreign_keys は on）。親を作り直す間だけ
-- 制約を commit まで遅らせ、同じ名前の親へ行を入れ直して違反を解消する（新しい名前で作って RENAME すると解消しない）。
PRAGMA defer_foreign_keys = ON;

CREATE TABLE script_generations_s5_copy AS
SELECT id, bundle_sha256, host_api, schema_sha256, manifest_json,
    CASE
        WHEN state = 'rejected' THEN 'invalid'
        WHEN id IN (SELECT generation_id FROM script_domain_modes WHERE generation_id IS NOT NULL) THEN 'active'
        ELSE 'retired'
    END AS state,
    created_at, updated_at
FROM script_generations;

DROP TABLE script_generations;

CREATE TABLE script_generations (
    id TEXT PRIMARY KEY,
    bundle_sha256 TEXT NOT NULL UNIQUE,
    host_api TEXT NOT NULL,
    schema_sha256 TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('loaded', 'active', 'draining', 'retired', 'invalid')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO script_generations
    (id, bundle_sha256, host_api, schema_sha256, manifest_json, state, created_at, updated_at)
SELECT id, bundle_sha256, host_api, schema_sha256, manifest_json, state, created_at, updated_at
FROM script_generations_s5_copy;

DROP TABLE script_generations_s5_copy;

-- CHECK は NULL で通るので `canary_bps IS NOT NULL` を明示する。
ALTER TABLE script_domain_modes ADD COLUMN canary_bps INTEGER CHECK (
    CASE WHEN mode = 'canary'
        THEN canary_bps IS NOT NULL AND canary_bps BETWEEN 1 AND 9999
        ELSE canary_bps IS NULL
    END
);

CREATE TABLE IF NOT EXISTS script_generation_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS script_generation_events_generation ON script_generation_events (generation_id, id);
CREATE TRIGGER IF NOT EXISTS script_generation_events_no_update
BEFORE UPDATE ON script_generation_events
BEGIN
    SELECT RAISE(ABORT, 'script_generation_events is append-only');
END;
CREATE TRIGGER IF NOT EXISTS script_generation_events_no_delete
BEFORE DELETE ON script_generation_events
BEGIN
    SELECT RAISE(ABORT, 'script_generation_events is append-only');
END;

-- 移行で変えた状態も記録に残す（旧 `rejected` の語は残さないので from_state は NULL）。
INSERT INTO script_generation_events (ts, generation_id, json)
SELECT updated_at, id, json_object(
    'type', CASE state
        WHEN 'invalid' THEN 'script_generation_invalidated'
        WHEN 'active' THEN 'script_generation_activated'
        ELSE 'script_generation_retired'
    END,
    'generation_id', id,
    'bundle_sha256', bundle_sha256,
    'from_state', CASE state WHEN 'invalid' THEN NULL ELSE 'loaded' END,
    'to_state', state,
    'actor', 'system:migration',
    'reason', 'migration 0105',
    'cancelled_in_flight', CASE state WHEN 'retired' THEN 0 ELSE NULL END
)
FROM script_generations ORDER BY created_at, id;

CREATE TABLE IF NOT EXISTS script_guard_state (
    domain TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    "window" TEXT NOT NULL DEFAULT '',
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    timeouts_10min TEXT NOT NULL DEFAULT '[]',
    clean_streak INTEGER NOT NULL DEFAULT 0,
    ts_side_clean_streak INTEGER NOT NULL DEFAULT 0,
    calls INTEGER NOT NULL DEFAULT 0,
    safety_violations INTEGER NOT NULL DEFAULT 0,
    halted INTEGER NOT NULL DEFAULT 0 CHECK (halted IN (0, 1)),
    halt_reason TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (domain, generation_id)
);
