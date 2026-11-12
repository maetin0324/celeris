-- ADR 2026-11-12-cos-triage-scope D2: CoS が Celeris の外の操作が要る件を外部 operator 宛てに回す経路 'operator'。
--
-- operator 宛ての件は経路だけを取り、通知（Discord）の行を作らない（notification_id は NULL）。SQLite は CHECK を
-- 変えられないので表を作り直す。行はそのまま写す（消さない）。
CREATE TABLE cos_notification_routes_new (
 item_id TEXT PRIMARY KEY, notification_id TEXT UNIQUE,
 route TEXT NOT NULL CHECK(route IN ('escalation','fallback','operator')),
 source_kind TEXT NOT NULL, source_key TEXT NOT NULL, source_revision TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(source_kind,source_key,source_revision)
);
INSERT INTO cos_notification_routes_new (item_id, notification_id, route, source_kind, source_key,
    source_revision, created_at, updated_at)
SELECT item_id, notification_id, route, source_kind, source_key, source_revision, created_at, updated_at
FROM cos_notification_routes;
DROP TABLE cos_notification_routes;
ALTER TABLE cos_notification_routes_new RENAME TO cos_notification_routes;
