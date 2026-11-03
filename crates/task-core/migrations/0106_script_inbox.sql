-- ADR 2026-11-02-ts-control-layer-boundary 付記 S5-2・S5-6・S5-7: 受信箱 `script_control` の元。
--
-- daemon の script-guard（`crates/celeris/src/daemon/script_guard.rs`）が `script_domain_events`・
-- `script_generation_events` の actor `system:script-startup`・`system:script-guard` の行を cursor で読み、
-- 1 行につき 1 件の項目を作る（冪等 key は記録の表と event の id。同じ event で 2 件作らない）。
-- 項目は人の actor の後の記録（同じ領域の mode の変更、同じ generation の遷移、停止の解除）で閉じる。
-- 記録の表（events）は追記専用のまま。この表の行は消さず、閉じた時刻だけを書く。
CREATE TABLE IF NOT EXISTS script_inbox_items (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('domain', 'generation')),
    event_id INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    domain TEXT,
    generation_id TEXT,
    change TEXT,
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    code TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- 元の記録の時刻（UNIX ms）。人の後の記録が閉じるのは、それより前の項目だけ。
    created_unix_ms INTEGER NOT NULL,
    resolved_at TEXT,
    resolved_by TEXT,
    UNIQUE (source, event_id)
);
CREATE INDEX IF NOT EXISTS script_inbox_items_open ON script_inbox_items (resolved_at, created_at);

-- 記録の表ごとの読み終えた id（前へだけ進む）。
CREATE TABLE IF NOT EXISTS script_inbox_cursors (
    source TEXT PRIMARY KEY CHECK (source IN ('domain', 'generation')),
    last_event_id INTEGER NOT NULL CHECK (last_event_id >= 0)
);
