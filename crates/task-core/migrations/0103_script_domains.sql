-- Migration 103（schema_version=103）: ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4。
--
-- `script_generations` — 検証した TS bundle の世代（同じ bundle_sha256 は 1 行）。
-- `script_domain_modes` — 領域ごとの mode（rust → compare → canary → ts）。更新は version の CAS だけ。
-- `script_domain_events` — mode の変更の記録（追記専用。特定の task に属さないので events に書かない）。
-- 旧 routing_shadow の表・event は使わない。
CREATE TABLE IF NOT EXISTS script_generations (
    id TEXT PRIMARY KEY,
    bundle_sha256 TEXT NOT NULL UNIQUE,
    host_api TEXT NOT NULL,
    schema_sha256 TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('loaded', 'rejected')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS script_domain_modes (
    domain TEXT PRIMARY KEY,
    mode TEXT NOT NULL CHECK (mode IN ('rust', 'compare', 'canary', 'ts')),
    generation_id TEXT REFERENCES script_generations(id),
    version INTEGER NOT NULL CHECK (version >= 0),
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL,
    CHECK (mode = 'rust' OR generation_id IS NOT NULL)
);

INSERT OR IGNORE INTO script_domain_modes (domain, mode, generation_id, version, updated_at, updated_by)
VALUES
    ('lane_rules', 'rust', NULL, 0, '1970-01-01T00:00:00Z', 'system:migration'),
    ('route_priority', 'rust', NULL, 0, '1970-01-01T00:00:00Z', 'system:migration');

CREATE TABLE IF NOT EXISTS script_domain_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    domain TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS script_domain_events_domain ON script_domain_events (domain, id);
CREATE TRIGGER IF NOT EXISTS script_domain_events_no_update
BEFORE UPDATE ON script_domain_events
BEGIN
    SELECT RAISE(ABORT, 'script_domain_events is append-only');
END;
CREATE TRIGGER IF NOT EXISTS script_domain_events_no_delete
BEFORE DELETE ON script_domain_events
BEGIN
    SELECT RAISE(ABORT, 'script_domain_events is append-only');
END;
